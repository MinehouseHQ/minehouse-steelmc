//! «Терминал оплаты», what it does (the Fabric `Terminal` and `StockGuard`).
//!
//! A terminal that is not locked is set up by whoever clicks it: the account, till or
//! vending, the goods and the price. «Сохранить и закрепить» locks it; only breaking the
//! block undoes that, and the site hears of both.
//!
//! - **Till**: the owner of the account names an amount, the buyer pays it with a card.
//! - **Vending**: the goods lie in the chest or barrel under the terminal and are sold
//!   without the seller. The goods leave the chest into the journal before the money moves,
//!   so a crash or a lost answer neither loses them nor hands them out unpaid.
//!
//! The stock of a locked vending terminal is open to everyone, but only the owner of the
//! account (and admins) may break it, and hoppers don't pull from it.

use std::{
    borrow::Cow,
    collections::BTreeSet,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use glam::DVec3;
use serde_json::{Value, json};
use steel_core::{
    behavior::blocks::{chest_container, connected_direction},
    block_entity::entities::HopperBlockEntity,
    entity::Entity as _,
    inventory::{
        container::{Container as _, SimpleContainer},
        lock::{ContainerId, ContainerLockGuard, ContainerRef},
        menu::{MenuBuilder, SectionKind},
    },
    permission::{PermissionExpr, PermissionKey},
    player::Player,
    server::Server,
    world::{World, block_guard::BlockGuard},
};
use steel_registry::{
    REGISTRY, RegistryExt as _,
    blocks::{
        block_state_ext::BlockStateExt as _,
        properties::{BlockStateProperties, ChestType},
    },
    data_components::vanilla_components::{CUSTOM_NAME, ITEM_NAME},
    item_stack::ItemStack,
    vanilla_blocks, vanilla_items, vanilla_menu_types,
};
use steel_utils::{
    BlockPos, Identifier,
    locks::{IntoShared as _, Shared, SyncMutex},
};
use text_components::{
    Modifier as _, TextComponent, format::Color, resolving::NoResolutor,
    translation::TranslatedMessage,
};
use uuid::Uuid;

use super::{
    Bank, ar,
    atm::{AMOUNT_BACK, AMOUNT_OK, AMOUNT_SHOWN, Buttons, ButtonsKind, ROWS, STEPS, near},
    bank, body, card, error, gui, is_offline,
    journal::{self, Entry, VENDING as VENDING_ENTRY},
    ok, online, ores,
    terminal_block::{self, is_terminal},
    terminals::{TILL, Terminal, VENDING},
};

/// Who may break the stock of someone else's vending terminal.
const ADMIN: &str = "minehouse.bank.admin";
/// The most a till or a vending terminal may ask for, АР.
const MAX_PRICE: i64 = 100_000;
/// A vending purchase the site never answered is asked about after this long, ms: an
/// in-flight payment gets time to finish before it is taken for one that never happened.
const VENDING_RETRY_MS: u64 = 120_000;
/// Labels are kept right for terminals this close to a player, blocks.
const LABEL_DISTANCE: i32 = 24;

const SETUP_ACCOUNT: usize = 10;
const SETUP_MODE: usize = 12;
const SETUP_SAMPLE: usize = 14;
const SETUP_LOCK: usize = 16;
const SETUP_PRICE: usize = 21;
const SETUP_CLOSE: usize = 26;
/// The price buttons by slot.
const PRICE_STEPS: [(usize, i64); 4] = [(19, -10), (20, -1), (22, 1), (23, 10)];

const NUMBER_BACK: usize = 1;
const NUMBER_OK: usize = 2;

const PAY_INFO: usize = 0;
const PAY_OK: usize = 2;
const PAY_CARD: usize = 3;
const PAY_CANCEL: usize = 4;

fn is_admin(player: &Player) -> bool {
    PermissionKey::parse(ADMIN)
        .ok()
        .map(PermissionExpr::key)
        .is_some_and(|permission| player.has_permission(&permission))
}

// ---- names -----------------------------------------------------------------------------------

/// `diamond_sword` as `Diamond Sword`: what the site is told when the item has no name of
/// its own (the server has no language files).
fn title_case(path: &str) -> String {
    path.split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut letters = word.chars();
            letters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(letters).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The name of the goods for a player: the anvil name if the item was renamed, otherwise the
/// item's own name, which the client shows in its language.
pub(super) fn item_name(stack: &ItemStack) -> TextComponent {
    if let Some(custom) = stack.get(CUSTOM_NAME) {
        let mut named = TextComponent::plain("").add_child(custom.clone());
        named.format = named.format.italic(true);
        return named;
    }
    if let Some(name) = stack.get(ITEM_NAME) {
        return name.clone();
    }
    let key = &stack.item().key;
    let group = if REGISTRY.blocks.by_key(key).is_some() {
        "block"
    } else {
        "item"
    };
    TranslatedMessage {
        key: Cow::Owned(format!("{group}.{}.{}", key.namespace, key.path)),
        fallback: Some(Cow::Owned(title_case(&key.path))),
        args: None,
    }
    .component()
}

/// Plain text of [`item_name`] for the site (history, the list of terminals).
pub(super) fn item_text(stack: &ItemStack) -> String {
    if let Some(name) = stack.get(CUSTOM_NAME) {
        return name.to_plain(&NoResolutor);
    }
    // A vanilla item is named by a key of the client's language files; ours by plain text.
    let own = stack
        .get(ITEM_NAME)
        .map(|name| name.to_plain(&NoResolutor))
        .filter(|name| !name.starts_with("[Translation: "));
    own.unwrap_or_else(|| title_case(&stack.item().key.path))
}

/// «покупку», «покупки», «покупок» for `n`.
fn purchases(n: i32) -> &'static str {
    let (ones, hundred) = (n % 10, n % 100);
    if ones == 1 && hundred != 11 {
        "покупку"
    } else if (2..=4).contains(&ones) && !(12..=14).contains(&hundred) {
        "покупки"
    } else {
        "покупок"
    }
}

// ---- stock chest -----------------------------------------------------------------------------

/// The chest or barrel right under the terminal at `terminal`: both halves of a double chest.
fn stock(world: &Arc<World>, terminal: BlockPos) -> Option<Vec<ContainerRef>> {
    let below = terminal.below();
    let state = world.get_block_state(below);
    if let Some(chest) = chest_container(state, world, below, true) {
        return (!chest.is_empty()).then_some(chest);
    }
    if state.get_block() == &vanilla_blocks::BARREL {
        return HopperBlockEntity::get_container_at(world, below);
    }
    None
}

/// The slots of `containers`, in order.
fn read(guard: &ContainerLockGuard, ids: &[ContainerId]) -> Vec<ItemStack> {
    ids.iter()
        .filter_map(|&id| guard.get(id))
        .flat_map(|container| container.items().to_vec())
        .collect()
}

/// Puts `items` back where [`read`] took them from.
fn write(guard: &mut ContainerLockGuard, ids: &[ContainerId], items: Vec<ItemStack>) {
    let mut items = items.into_iter();
    for &id in ids {
        let Some(container) = guard.get_mut(id) else {
            continue;
        };
        for slot in 0..container.get_container_size() {
            if let Some(item) = items.next()
                && !ItemStack::matches(container.get_item(slot), &item)
            {
                container.set_item(slot, item);
            }
        }
        container.set_changed();
    }
}

/// Runs `change` on the slots of `containers` and writes back what it changed.
fn with_slots<R>(containers: &[ContainerRef], change: impl FnOnce(&mut Vec<ItemStack>) -> R) -> R {
    let ids: Vec<ContainerId> = containers.iter().map(ContainerRef::container_id).collect();
    let mut guard = ContainerLockGuard::lock_all(containers);
    let mut items = read(&guard, &ids);
    let result = change(&mut items);
    write(&mut guard, &ids, items);
    result
}

/// How many items like `sample` there are in `items`.
fn count_like(items: &[ItemStack], sample: &ItemStack) -> i32 {
    items
        .iter()
        .filter(|item| ItemStack::is_same_item_same_components(item, sample))
        .map(ItemStack::count)
        .sum()
}

/// Takes `count` items like `sample`, or nothing if there aren't enough.
fn take(items: &mut [ItemStack], sample: &ItemStack, count: i32) -> Option<Vec<ItemStack>> {
    if count <= 0 || count_like(items, sample) < count {
        return None;
    }
    let mut taken = Vec::new();
    let mut left = count;
    for item in items.iter_mut() {
        if left == 0 {
            break;
        }
        if !ItemStack::is_same_item_same_components(item, sample) {
            continue;
        }
        let part = left.min(item.count());
        taken.push(item.split(part));
        left -= part;
    }
    Some(taken)
}

/// Puts `stack` into `items`: onto stacks of the same first, then into empty slots. Returns
/// what did not fit.
fn insert(items: &mut [ItemStack], mut stack: ItemStack) -> ItemStack {
    for item in items.iter_mut() {
        if stack.is_empty() {
            break;
        }
        if ItemStack::is_same_item_same_components(item, &stack)
            && item.count() < item.max_stack_size()
        {
            let part = stack.count().min(item.max_stack_size() - item.count());
            item.grow(part);
            stack.shrink(part);
        }
    }
    for item in items.iter_mut() {
        if stack.is_empty() {
            break;
        }
        if item.is_empty() {
            *item = stack;
            stack = ItemStack::empty();
        }
    }
    stack
}

/// How many sales the stock of the terminal at `pos` is good for.
fn stock_count(world: &Arc<World>, pos: BlockPos, terminal: &Terminal) -> i32 {
    let (Some(stock), Some(sample)) = (stock(world, pos), terminal.sample_stack()) else {
        return 0;
    };
    if terminal.count <= 0 {
        return 0;
    }
    with_slots(&stock, |items| count_like(items, &sample)) / terminal.count
}

fn vending_above(bank: &Bank, world: &World, pos: BlockPos) -> Option<Terminal> {
    let above = pos.above();
    if !is_terminal(world, above) {
        return None;
    }
    bank.terminals
        .get(&world.key, above)
        .filter(|terminal| terminal.locked && terminal.vending())
}

/// The locked vending terminal that guards the chest at `pos` (either half of a double
/// chest).
fn guarding(bank: &Bank, world: &World, pos: BlockPos) -> Option<Terminal> {
    if let Some(terminal) = vending_above(bank, world, pos) {
        return Some(terminal);
    }
    let state = world.get_block_state(pos);
    let block = state.get_block();
    let double = (block == &vanilla_blocks::CHEST || block == &vanilla_blocks::TRAPPED_CHEST)
        && state.get_value(&BlockStateProperties::CHEST_TYPE) != ChestType::Single;
    if double {
        return vending_above(bank, world, pos.relative(connected_direction(state)));
    }
    None
}

/// Keeps the stock of vending terminals from other players' pickaxes and from hoppers.
pub struct StockGuard;

impl BlockGuard for StockGuard {
    fn keeps_from_breaking(&self, player: &Player, world: &Arc<World>, pos: BlockPos) -> bool {
        let Some(bank) = bank() else {
            return false;
        };
        let Some(terminal) = guarding(bank, world, pos) else {
            return false;
        };
        let owner = bank.account_number(player.gameprofile.id()).as_deref()
            == Some(terminal.account.as_str());
        if owner || is_admin(player) {
            return false;
        }
        gui::error(player, &format!("Это склад автомата {}.", terminal.owner));
        true
    }

    fn keeps_from_hoppers(&self, world: &Arc<World>, pos: BlockPos) -> bool {
        bank().is_some_and(|bank| guarding(bank, world, pos).is_some())
    }
}

// ---- label -----------------------------------------------------------------------------------

/// What the label above a terminal says, and a plain line that changes whenever it does.
fn label(terminal: &Terminal, left: i32) -> (TextComponent, String) {
    if !terminal.locked {
        let text = TextComponent::plain("Терминал\n")
            .color(Color::Gray)
            .add_child(TextComponent::plain("не закреплён").color(Color::DarkGray));
        return (text, "unlocked".to_owned());
    }
    if terminal.vending() {
        let sample = terminal.sample_stack();
        let name = sample
            .as_ref()
            .map_or_else(|| TextComponent::plain("?"), item_name);
        let goods = TextComponent::plain("")
            .color(Color::White)
            .add_child(name)
            .add_child(TextComponent::plain(format!(" × {}\n", terminal.count)));
        let mut text = TextComponent::plain("")
            .add_child(goods)
            .add_child(TextComponent::plain(ar(terminal.price)).color(Color::Aqua));
        if left == 0 {
            text = text.add_child(TextComponent::plain(" · нет в наличии").color(Color::Red));
        }
        let what = sample.as_ref().map_or_default(item_text);
        let line = format!(
            "vending {what} {} {} {}",
            terminal.count,
            terminal.price,
            left == 0
        );
        return (text, line);
    }
    let invoice = terminal.active_invoice().map(|invoice| invoice.amount);
    let second = invoice.map_or_else(
        || TextComponent::plain("ждёт счёта").color(Color::DarkGray),
        |amount| TextComponent::plain(format!("К оплате: {}", ar(amount))).color(Color::Aqua),
    );
    let text = TextComponent::plain("")
        .add_child(
            TextComponent::plain(format!("Касса · {}\n", terminal.owner)).color(Color::White),
        )
        .add_child(second);
    (text, format!("till {} {invoice:?}", terminal.owner))
}

/// Has the label above the terminal at `pos` say what the terminal is set to now.
pub(super) fn relabel(bank: &Bank, world: &Arc<World>, pos: BlockPos) {
    let Some(terminal) = bank.terminals.get(&world.key, pos) else {
        return;
    };
    let left = if terminal.locked && terminal.vending() {
        stock_count(world, pos, &terminal)
    } else {
        0
    };
    let (text, line) = label(&terminal, left);
    if terminal.label.as_deref() == Some(line.as_str()) && terminal_block::has_label(world, pos) {
        return;
    }
    terminal_block::show_label(world, pos, text);
    bank.terminals
        .change(&world.key, pos, |terminal| terminal.label = Some(line));
}

/// Every half second: the labels of the terminals near players follow the stock and the
/// invoices.
pub(super) fn tick_labels(bank: &Bank, server: &Server) {
    let mut seen: BTreeSet<(String, i32, i32, i32)> = BTreeSet::new();
    for player in server.get_players() {
        let world = player.get_world();
        let at = player.position().floor();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a player's coordinates fit i32"
        )]
        let (x, y, z) = (at.x as i32, at.y as i32, at.z as i32);
        for pos in bank.terminals.places(&world.key) {
            let close = (pos.x() - x).abs() <= LABEL_DISTANCE
                && (pos.y() - y).abs() <= LABEL_DISTANCE
                && (pos.z() - z).abs() <= LABEL_DISTANCE;
            if !close || !seen.insert((world.key.to_string(), pos.x(), pos.y(), pos.z())) {
                continue;
            }
            if is_terminal(&world, pos) {
                relabel(bank, &world, pos);
            }
        }
    }
}

// ---- use -------------------------------------------------------------------------------------

/// A right click on the terminal at `pos`.
pub fn open(player: &Player, pos: BlockPos) {
    let Some(bank) = bank() else {
        gui::error(player, "Банк пока закрыт.");
        return;
    };
    let world = player.get_world();
    // A terminal of a world that came from the Fabric server is set up anew.
    bank.terminals.ensure(&world.key, pos);
    let uuid = player.gameprofile.id();
    my_number(bank, uuid, move |server, number| {
        if let Some(player) = online(server, uuid) {
            route(bank, &player, world, pos, number);
        }
    });
}

/// The player's account number from the cache, or asked from the site; None: no account.
fn my_number(
    bank: &'static Arc<Bank>,
    player: Uuid,
    then: impl FnOnce(&Arc<Server>, Option<String>) + Send + 'static,
) {
    if let Some(number) = bank.account_number(player) {
        bank.later(move |server| then(server, Some(number)));
        return;
    }
    bank.account(player, None, None, move |server, answer| {
        if !ok(&answer) && is_offline(&answer) {
            if let Some(player) = online(server, player) {
                gui::error(&player, &gui::error_text("offline"));
            }
            return;
        }
        then(server, bank.account_number(player));
    });
}

fn route(
    bank: &'static Arc<Bank>,
    player: &Arc<Player>,
    world: Arc<World>,
    pos: BlockPos,
    number: Option<String>,
) {
    if !is_terminal(&world, pos) || !near(bank, player, pos) {
        return;
    }
    let Some(terminal) = bank.terminals.get(&world.key, pos) else {
        return;
    };
    if !terminal.locked {
        setup_window(bank, player, world, pos, number.as_deref());
        return;
    }
    let owner = number.as_deref() == Some(terminal.account.as_str());
    if terminal.vending() {
        if owner {
            tell_owner(player, &world, pos, &terminal);
        } else {
            pay_window(bank, player, world, pos, &terminal);
        }
    } else if owner {
        invoice_window(bank, player, world, pos);
    } else if terminal.active_invoice().is_some() {
        pay_window(bank, player, world, pos, &terminal);
    } else {
        gui::error(player, "Продавец ещё не выставил счёт.");
    }
}

/// The owner of a vending terminal is told what it sells and how much is left.
fn tell_owner(player: &Player, world: &Arc<World>, pos: BlockPos, terminal: &Terminal) {
    let left = stock_count(world, pos, terminal);
    let name = terminal
        .sample_stack()
        .as_ref()
        .map_or_else(|| TextComponent::plain("?"), item_name);
    let stock = if left > 0 {
        format!("Товара в сундуке хватит на {left} {}.", purchases(left))
    } else {
        "Товар закончился, положи его в сундук под терминалом.".to_owned()
    };
    player.send_message(
        &TextComponent::plain("Это твой автомат: ")
            .color(Color::Yellow)
            .add_child(name)
            .add_child(TextComponent::plain(format!(
                " × {} за {}. {stock}",
                terminal.count,
                ar(terminal.price)
            ))),
    );
    player.send_message(
        &TextComponent::plain("Чтобы поменять товар или цену, сломай терминал и поставь заново.")
            .color(Color::Gray),
    );
}

// ---- setup -----------------------------------------------------------------------------------

/// The button of the account the money goes to.
fn account_button(terminal: &Terminal) -> ItemStack {
    if terminal.account.is_empty() {
        gui::button(
            &vanilla_items::NAME_TAG,
            "Счёт не выбран",
            &[
                "Нажми и введи номер MH-1234-5678",
                "Нажми, чтобы ввести другой номер",
            ],
        )
    } else {
        gui::button(
            &vanilla_items::NAME_TAG,
            &format!("Счёт {}", terminal.account),
            &[
                &format!("Владелец: {}", terminal.owner),
                "Нажми, чтобы ввести другой номер",
            ],
        )
    }
}

/// The button that switches between till and vending.
fn mode_button(vending: bool) -> ItemStack {
    if vending {
        gui::button(
            &vanilla_items::CHEST,
            "Режим: автомат",
            &[
                "Товар в сундуке под терминалом,",
                "продаётся и без продавца.",
                "Нажми, чтобы сменить",
            ],
        )
    } else {
        gui::button(
            &vanilla_items::PAPER,
            "Режим: касса",
            &[
                "Продавец выставляет сумму,",
                "покупатель платит картой.",
                "Нажми, чтобы сменить",
            ],
        )
    }
}

/// The window that sets a terminal up.
struct Setup {
    bank: &'static Arc<Bank>,
    player: Uuid,
    world: Arc<World>,
    pos: BlockPos,
    buttons: Shared<SimpleContainer>,
    /// A call is in flight; the window is to be closed.
    state: SyncMutex<(bool, bool)>,
}

impl Setup {
    fn terminal(&self) -> Option<Terminal> {
        self.bank.terminals.get(&self.world.key, self.pos)
    }

    fn change(&self, change: impl FnOnce(&mut Terminal)) {
        self.bank
            .terminals
            .change(&self.world.key, self.pos, change);
    }

    /// What the window shows for `terminal`, by slot.
    fn layout(terminal: &Terminal) -> Vec<(usize, ItemStack)> {
        let vending = terminal.vending();
        let (account, mode) = (account_button(terminal), mode_button(vending));
        let mut layout = vec![(SETUP_ACCOUNT, account), (SETUP_MODE, mode)];
        if vending {
            let sample = terminal.sample_stack().map_or_else(
                || {
                    gui::button(
                        &vanilla_items::ITEM_FRAME,
                        "Товар не выбран",
                        &[
                            "Возьми товар в руку и нажми.",
                            "Сколько в руке — столько за одну покупку.",
                        ],
                    )
                },
                |sample| {
                    shown_goods(
                        &sample,
                        terminal.count,
                        vec![
                            gui::line(
                                &format!("За одну покупку: {} шт.", terminal.count),
                                Color::Gray,
                            ),
                            gui::line(
                                "Нажми с другим товаром в руке, чтобы заменить",
                                Color::DarkGray,
                            ),
                        ],
                    )
                },
            );
            layout.push((SETUP_SAMPLE, sample));
            for (slot, delta) in PRICE_STEPS {
                let (item, sign) = if delta < 0 {
                    (&vanilla_items::RED_STAINED_GLASS_PANE, "−")
                } else {
                    (&vanilla_items::LIME_STAINED_GLASS_PANE, "+")
                };
                let mut button = gui::button(item, &format!("{sign}{} АР", delta.abs()), &[]);
                button.set_count(i32::try_from(delta.abs()).unwrap_or(1));
                layout.push((slot, button));
            }
            let mut price = gui::button(
                &vanilla_items::DIAMOND_ORE,
                &format!("Цена: {}", ar(terminal.price)),
                &["За одну покупку"],
            );
            price.set_count(i32::try_from(terminal.price.clamp(1, 64)).unwrap_or(1));
            layout.push((SETUP_PRICE, price));
        }
        let to = if terminal.account.is_empty() {
            "?"
        } else {
            terminal.account.as_str()
        };
        layout.push((
            SETUP_LOCK,
            gui::button(
                &vanilla_items::TRIPWIRE_HOOK,
                "Сохранить и закрепить",
                &[
                    &format!("Деньги пойдут на счёт {to}."),
                    "Снять замок можно только сломав терминал.",
                ],
            ),
        ));
        layout.push((
            SETUP_CLOSE,
            gui::button(&vanilla_items::BARRIER, "Закрыть", &[]),
        ));
        layout
    }

    fn draw(&self) {
        let Some(terminal) = self.terminal() else {
            return;
        };
        let mut buttons = self.buttons.lock();
        for slot in 0..ROWS {
            buttons.set_item(slot, ItemStack::empty());
        }
        for (slot, stack) in Self::layout(&terminal) {
            buttons.set_item(slot, stack);
        }
    }

    fn act(self: &Arc<Self>, server: &Server, slot: usize) {
        let Some(player) = online(server, self.player) else {
            return;
        };
        let Some(terminal) = self.terminal() else {
            return;
        };
        if terminal.locked || self.state.lock().0 {
            return;
        }
        if slot == SETUP_ACCOUNT {
            gui::click(&player);
            self.state.lock().1 = true;
            number_window(
                self.bank,
                &player,
                Arc::clone(&self.world),
                self.pos,
                &terminal.account,
            );
        } else if slot == SETUP_MODE {
            gui::click(&player);
            self.change(|terminal| {
                let next = if terminal.vending() { TILL } else { VENDING };
                next.clone_into(&mut terminal.mode);
            });
            self.draw();
        } else if slot == SETUP_SAMPLE && terminal.vending() {
            self.sample(&player);
        } else if let Some((_, delta)) = PRICE_STEPS.iter().find(|(at, _)| *at == slot)
            && terminal.vending()
        {
            gui::click(&player);
            self.change(|terminal| terminal.price = (terminal.price + delta).clamp(0, MAX_PRICE));
            self.draw();
        } else if slot == SETUP_LOCK {
            self.lock(&player, &terminal);
        } else if slot == SETUP_CLOSE {
            self.state.lock().1 = true;
        }
    }

    /// What the player holds becomes the goods: one sale hands out as many as are in the hand.
    fn sample(&self, player: &Player) {
        let hand = player.inventory.lock().get_selected_item().clone();
        if hand.is_empty() {
            gui::error(player, "Возьми товар в руку.");
            return;
        }
        let Some(sample) = journal::encode(&hand.copy_with_count(1)) else {
            gui::error(player, "Этот предмет нельзя продавать через автомат.");
            return;
        };
        gui::click(player);
        self.change(|terminal| {
            terminal.sample = Some(sample);
            terminal.count = hand.count();
        });
        self.draw();
    }

    fn lock(self: &Arc<Self>, player: &Player, terminal: &Terminal) {
        if terminal.account.is_empty() {
            gui::error(player, "Сначала выбери счёт.");
            return;
        }
        let mut body = json!({
            "terminal_id": terminal.id.to_string(),
            "action": "lock",
            "to_number": terminal.account,
            "mode": terminal.mode,
        });
        place(&mut body, &self.world, self.pos);
        if terminal.vending() {
            let Some(sample) = terminal.sample_stack().filter(|_| terminal.price > 0) else {
                gui::error(player, "Для автомата нужны товар и цена.");
                return;
            };
            if stock(&self.world, self.pos).is_none() {
                gui::error(
                    player,
                    "Автомату нужен сундук прямо под терминалом: там лежит товар.",
                );
                return;
            }
            body["item_name"] = json!(item_text(&sample));
            body["item_count"] = json!(terminal.count);
            body["price"] = json!(terminal.price);
        }
        self.state.lock().0 = true;
        gui::click(player);
        let window = Arc::clone(self);
        self.bank.call("terminal", &body, move |server, answer| {
            window.state.lock().0 = false;
            let Some(terminal) = window.terminal() else {
                return;
            };
            let player = online(server, window.player);
            if !ok(&answer) {
                if let Some(player) = player {
                    gui::error(&player, &gui::error_text(error(&answer)));
                }
                return;
            }
            window.change(|terminal| terminal.locked = true);
            terminal_block::light_up(&window.world, window.pos);
            relabel(window.bank, &window.world, window.pos);
            log::info!(
                "Bank: {} locked terminal {} at {:?} to {} ({})",
                player
                    .as_ref()
                    .map_or("?", |player| player.gameprofile.name()),
                terminal.id,
                window.pos,
                terminal.account,
                terminal.mode
            );
            if let Some(player) = player {
                gui::ding(&player);
                gui::info(
                    &player,
                    &format!("Терминал закреплён за счётом {}.", terminal.account),
                );
            }
            window.state.lock().1 = true;
        });
    }
}

impl Buttons for Setup {
    fn closing(&self) -> bool {
        self.state.lock().1
            || !is_terminal(&self.world, self.pos)
            || self.terminal().is_none_or(|terminal| terminal.locked)
    }

    fn pos(&self) -> Option<BlockPos> {
        Some(self.pos)
    }

    fn bank(&self) -> &Bank {
        self.bank
    }

    fn clicked(self: Arc<Self>, slot: usize) {
        let bank = self.bank;
        bank.later(move |server| self.act(server, slot));
    }
}

/// The goods as a button: the item itself, as many as one sale hands out (up to a stack).
fn shown_goods(sample: &ItemStack, count: i32, lore: Vec<TextComponent>) -> ItemStack {
    let mut shown = sample.copy_with_count(count.clamp(1, sample.max_stack_size()));
    gui::add_lore(&mut shown, lore);
    shown
}

fn setup_window(
    bank: &'static Arc<Bank>,
    player: &Arc<Player>,
    world: Arc<World>,
    pos: BlockPos,
    number: Option<&str>,
) {
    // Whoever sets a fresh terminal up first is offered their own account.
    if let Some(number) = number {
        bank.terminals.change(&world.key, pos, |terminal| {
            if terminal.account.is_empty() {
                number.clone_into(&mut terminal.account);
                player.gameprofile.name().clone_into(&mut terminal.owner);
            }
        });
    }
    let window = Arc::new(Setup {
        bank,
        player: player.gameprofile.id(),
        world,
        pos,
        buttons: SimpleContainer::new(ROWS).into_shared(),
        state: SyncMutex::new((false, false)),
    });
    window.draw();
    let viewer = Arc::clone(player);
    player.open_menu(gui::title(gui::SETUP, "Терминал"), move |context| {
        let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X3, context.container_id);
        builder.section_all_with(ContainerRef::from(&window.buttons), SectionKind::Display);
        builder.player_inventory(&viewer.inventory);
        builder.build(ButtonsKind(window))
    });
}

/// The account number of a terminal, typed in an anvil window.
struct NumberInput {
    bank: &'static Arc<Bank>,
    player: Uuid,
    world: Arc<World>,
    pos: BlockPos,
    /// The number typed so far, whether a call is in flight, whether the window is to close.
    state: SyncMutex<(String, bool, bool)>,
}

impl NumberInput {
    fn back(&self, player: &Arc<Player>) {
        self.state.lock().2 = true;
        if self
            .bank
            .terminals
            .get(&self.world.key, self.pos)
            .is_some_and(|terminal| !terminal.locked)
        {
            setup_window(self.bank, player, Arc::clone(&self.world), self.pos, None);
        }
    }

    fn act(self: &Arc<Self>, server: &Server, slot: usize) {
        let Some(player) = online(server, self.player) else {
            return;
        };
        if slot == NUMBER_BACK {
            gui::click(&player);
            self.back(&player);
        } else if slot == NUMBER_OK {
            let (typed, busy, _) = self.state.lock().clone();
            if busy {
                return;
            }
            self.state.lock().1 = true;
            gui::click(&player);
            let window = Arc::clone(self);
            let body = json!({"number": typed});
            self.bank.call("resolve", &body, move |server, answer| {
                window.state.lock().1 = false;
                let Some(player) = online(server, window.player) else {
                    return;
                };
                if !ok(&answer) {
                    gui::error(&player, &gui::error_text(error(&answer)));
                    return;
                }
                let text = |key: &str| {
                    answer
                        .get(key)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned()
                };
                let (number, owner) = (text("number"), text("owner"));
                window
                    .bank
                    .terminals
                    .change(&window.world.key, window.pos, |terminal| {
                        if !terminal.locked {
                            terminal.account = number;
                            terminal.owner = owner;
                        }
                    });
                window.back(&player);
            });
        }
    }
}

impl Buttons for NumberInput {
    fn closing(&self) -> bool {
        self.state.lock().2 || !is_terminal(&self.world, self.pos)
    }

    fn pos(&self) -> Option<BlockPos> {
        Some(self.pos)
    }

    fn bank(&self) -> &Bank {
        self.bank
    }

    fn clicked(self: Arc<Self>, slot: usize) {
        let bank = self.bank;
        bank.later(move |server| self.act(server, slot));
    }

    fn renamed(&self, name: String) {
        self.state.lock().0 = name;
    }
}

fn number_window(
    bank: &'static Arc<Bank>,
    player: &Arc<Player>,
    world: Arc<World>,
    pos: BlockPos,
    current: &str,
) {
    let start = if current.is_empty() { "MH-" } else { current };
    let window = Arc::new(NumberInput {
        bank,
        player: player.gameprofile.id(),
        world,
        pos,
        state: SyncMutex::new((start.to_owned(), false, false)),
    });
    // The text field of an anvil shows the name of the item in the first slot.
    let mut typed = ItemStack::new(&vanilla_items::PAPER);
    typed.set(CUSTOM_NAME, TextComponent::plain(start.to_owned()));
    let mut buttons = SimpleContainer::new(3);
    buttons.set_item(0, typed);
    buttons.set_item(
        NUMBER_BACK,
        gui::button(&vanilla_items::ARROW, "Назад", &[]),
    );
    buttons.set_item(
        NUMBER_OK,
        gui::button(
            &vanilla_items::LIME_DYE,
            "Готово",
            &["Номер вида MH-1234-5678"],
        ),
    );
    let buttons = buttons.into_shared();
    let viewer = Arc::clone(player);
    player.open_menu(
        TextComponent::plain("Номер счёта"),
        move |context| {
            let mut builder = MenuBuilder::new(&vanilla_menu_types::ANVIL, context.container_id);
            builder.section_all_with(ContainerRef::from(&buttons), SectionKind::Display);
            builder.player_inventory(&viewer.inventory);
            // The level cost an anvil window shows.
            builder.data_slot(0);
            builder.build(ButtonsKind(window))
        },
    );
}

// ---- the till's invoice ----------------------------------------------------------------------

/// The amount picker of a till: what the next buyer is asked for.
struct InvoiceAmount {
    bank: &'static Arc<Bank>,
    player: Uuid,
    world: Arc<World>,
    pos: BlockPos,
    buttons: Shared<SimpleContainer>,
    /// The amount; the window is to be closed.
    state: SyncMutex<(i64, bool)>,
}

impl InvoiceAmount {
    fn draw(&self) {
        let amount = self.state.lock().0;
        let mut buttons = self.buttons.lock();
        for (slot, delta) in STEPS {
            let (item, sign) = if delta < 0 {
                (&vanilla_items::RED_STAINED_GLASS_PANE, "−")
            } else {
                (&vanilla_items::LIME_STAINED_GLASS_PANE, "+")
            };
            let mut button = gui::button(item, &format!("{sign}{}", delta.abs()), &[]);
            button.set_count(delta.abs());
            buttons.set_item(slot, button);
        }
        let mut shown = gui::button(
            &vanilla_items::DEEPSLATE_DIAMOND_ORE,
            &ar(amount),
            &["Сумма счёта для покупателя"],
        );
        shown.set_count(i32::try_from(amount.clamp(1, 64)).unwrap_or(1));
        buttons.set_item(AMOUNT_SHOWN, shown);
        buttons.set_item(
            AMOUNT_OK,
            gui::button(
                &vanilla_items::LIME_DYE,
                &format!("Выставить {}", ar(amount)),
                &[],
            ),
        );
        buttons.set_item(
            AMOUNT_BACK,
            gui::button(&vanilla_items::ARROW, "Назад", &[]),
        );
    }

    fn act(&self, server: &Server, slot: usize) {
        let Some(player) = online(server, self.player) else {
            return;
        };
        if let Some((_, delta)) = STEPS.iter().find(|(at, _)| *at == slot) {
            gui::click(&player);
            {
                let mut state = self.state.lock();
                state.0 = (state.0 + i64::from(*delta)).clamp(1, MAX_PRICE);
            }
            self.draw();
        } else if slot == AMOUNT_BACK {
            gui::click(&player);
            self.state.lock().1 = true;
        } else if slot == AMOUNT_OK {
            let amount = self.state.lock().0;
            let seconds = self.bank.config.invoice_seconds;
            let set = self
                .bank
                .terminals
                .change(&self.world.key, self.pos, |terminal| {
                    terminal.set_invoice(amount, seconds);
                });
            if set.is_some() {
                relabel(self.bank, &self.world, self.pos);
                gui::info(
                    &player,
                    &format!(
                        "Счёт на {} выставлен. Покупатель платит ПКМ по терминалу.",
                        ar(amount)
                    ),
                );
            }
            self.state.lock().1 = true;
        }
    }
}

impl Buttons for InvoiceAmount {
    fn closing(&self) -> bool {
        self.state.lock().1 || !is_terminal(&self.world, self.pos)
    }

    fn pos(&self) -> Option<BlockPos> {
        Some(self.pos)
    }

    fn bank(&self) -> &Bank {
        self.bank
    }

    fn clicked(self: Arc<Self>, slot: usize) {
        let bank = self.bank;
        bank.later(move |server| self.act(server, slot));
    }
}

fn invoice_window(
    bank: &'static Arc<Bank>,
    player: &Arc<Player>,
    world: Arc<World>,
    pos: BlockPos,
) {
    let window = Arc::new(InvoiceAmount {
        bank,
        player: player.gameprofile.id(),
        world,
        pos,
        buttons: SimpleContainer::new(ROWS).into_shared(),
        state: SyncMutex::new((1, false)),
    });
    window.draw();
    let viewer = Arc::clone(player);
    player.open_menu(
        gui::title(gui::AMOUNT, "Выставить счёт"),
        move |context| {
            let mut builder =
                MenuBuilder::new(&vanilla_menu_types::GENERIC_9X3, context.container_id);
            builder.section_all_with(ContainerRef::from(&window.buttons), SectionKind::Display);
            builder.player_inventory(&viewer.inventory);
            builder.build(ButtonsKind(window))
        },
    );
}

// ---- paying ----------------------------------------------------------------------------------

struct PayState {
    /// Which of the cards pays.
    selected: usize,
    busy: bool,
    close: bool,
}

/// The window of a buyer: what is paid for, with which card.
struct Pay {
    bank: &'static Arc<Bank>,
    player: Uuid,
    world: Arc<World>,
    pos: BlockPos,
    buttons: Shared<SimpleContainer>,
    /// The invoice the window was opened on; None at a vending terminal.
    invoice: Option<Uuid>,
    amount: i64,
    /// The cards the player carried when the window opened.
    cards: Vec<ItemStack>,
    state: SyncMutex<PayState>,
}

impl Pay {
    /// Slot 2 pays, slot 3 is the card that will be charged: its own stack, so the number,
    /// the owner and the account name from the card's lore are right there. With several
    /// cards a click on it picks the next one.
    fn show_card(&self) {
        let selected = self.state.lock().selected;
        let mut buttons = self.buttons.lock();
        let name = format!("Оплатить {}", ar(self.amount));
        let Some(card) = self.cards.get(selected) else {
            let mut pay = gui::button(
                &vanilla_items::PAPER,
                &name,
                &["Нужна карта MineHouse в инвентаре.", "Её выдаёт банкомат."],
            );
            gui::model(&mut pay, "minehouse-bank:card");
            buttons.set_item(PAY_OK, pay);
            buttons.set_item(PAY_CARD, ItemStack::empty());
            return;
        };
        let mut pay = gui::button(
            &vanilla_items::PAPER,
            &name,
            &[&format!("Спишется с карты {}.", card::number(card))],
        );
        gui::model(&mut pay, "minehouse-bank:card");
        buttons.set_item(PAY_OK, pay);
        let mut lore = vec![
            TextComponent::plain(""),
            gui::line("Оплата пройдёт с этой карты", Color::Green),
        ];
        if self.cards.len() > 1 {
            lore.push(gui::line(
                &format!(
                    "Нажми, чтобы выбрать другую ({} из {})",
                    selected + 1,
                    self.cards.len()
                ),
                Color::Yellow,
            ));
        }
        let mut shown = card.copy_with_count(1);
        gui::add_lore(&mut shown, lore);
        buttons.set_item(PAY_CARD, shown);
    }

    fn terminal(&self) -> Option<Terminal> {
        self.bank.terminals.get(&self.world.key, self.pos)
    }

    fn end(&self) {
        self.state.lock().busy = false;
    }

    fn close(&self) {
        self.state.lock().close = true;
    }

    fn act(self: &Arc<Self>, server: &Arc<Server>, slot: usize) {
        let Some(player) = online(server, self.player) else {
            return;
        };
        if slot == PAY_CANCEL {
            self.close();
        } else if slot == PAY_CARD && self.cards.len() > 1 {
            {
                let mut state = self.state.lock();
                if state.busy {
                    return;
                }
                state.selected = (state.selected + 1) % self.cards.len();
            }
            gui::click(&player);
            self.show_card();
        } else if slot == PAY_OK {
            self.pay(server, &player);
        }
    }

    fn pay(self: &Arc<Self>, server: &Arc<Server>, player: &Arc<Player>) {
        let Some(terminal) = self.terminal() else {
            return;
        };
        let selected = {
            let state = self.state.lock();
            if state.busy || !terminal.locked {
                return;
            }
            state.selected
        };
        let Some(card) = self.cards.get(selected).and_then(card::id) else {
            gui::error(
                player,
                "Нужна карта MineHouse в инвентаре. Её выдаёт банкомат.",
            );
            return;
        };
        // The chosen card may have been dropped meanwhile.
        let carried = card::all(player)
            .iter()
            .any(|stack| card::id(stack).as_deref() == Some(card.as_str()));
        if !carried {
            gui::error(player, "Выбранной карты больше нет в инвентаре.");
            self.close();
            return;
        }
        if terminal.vending() {
            self.state.lock().busy = true;
            gui::click(player);
            self.pay_vending(player, &terminal, &card);
            return;
        }
        let Some(invoice) = terminal
            .active_invoice()
            .filter(|invoice| Some(invoice.id) == self.invoice)
        else {
            gui::error(
                player,
                "Счёт уже не действует. Попроси продавца выставить новый.",
            );
            self.close();
            return;
        };
        if invoice.paying {
            gui::error(player, "Этот счёт уже оплачивает другой игрок.");
            return;
        }
        self.state.lock().busy = true;
        gui::click(player);
        self.pay_till(server, &terminal, invoice.amount, &card);
    }

    fn pay_body(
        &self,
        terminal: &Terminal,
        op: &str,
        card: &str,
        amount: i64,
        memo: &str,
    ) -> Value {
        let mut body = body(self.player);
        body["op_id"] = json!(op);
        body["card_id"] = json!(card);
        body["to_number"] = json!(terminal.account);
        body["amount"] = json!(amount);
        body["terminal_id"] = json!(terminal.id.to_string());
        if !memo.is_empty() {
            body["memo"] = json!(memo);
        }
        body
    }

    fn paying(&self, paying: bool) {
        let invoice = self.invoice;
        self.bank
            .terminals
            .change(&self.world.key, self.pos, |terminal| {
                if let Some(now) = terminal.invoice.as_mut()
                    && Some(now.id) == invoice
                {
                    now.paying = paying;
                }
            });
    }

    fn pay_till(
        self: &Arc<Self>,
        _server: &Arc<Server>,
        terminal: &Terminal,
        amount: i64,
        card: &str,
    ) {
        self.paying(true);
        let seller = terminal.account.clone();
        let op = Uuid::new_v4().to_string();
        let body = self.pay_body(terminal, &op, card, amount, "касса");
        let window = Arc::clone(self);
        self.bank.call("pay", &body, move |server, answer| {
            window.end();
            window.paying(false);
            let player = online(server, window.player);
            if !ok(&answer) {
                if let Some(player) = player {
                    let line = if is_offline(&answer) {
                        "Нет связи с банком. Проверь на сайте, прошла ли оплата, прежде чем \
                         платить снова."
                            .to_owned()
                    } else {
                        gui::error_text(error(&answer))
                    };
                    gui::error(&player, &line);
                }
                return;
            }
            let invoice = window.invoice;
            window
                .bank
                .terminals
                .change(&window.world.key, window.pos, |terminal| {
                    if terminal.invoice.as_ref().map(|invoice| invoice.id) == invoice {
                        terminal.invoice = None;
                    }
                });
            relabel(window.bank, &window.world, window.pos);
            let buyer = player.as_ref().map_or_else(
                || "Покупатель".to_owned(),
                |player| player.gameprofile.name().to_owned(),
            );
            if let Some(player) = player {
                gui::ding(&player);
                gui::info(&player, &format!("Оплачено {}.", ar(amount)));
            }
            notify_seller(
                window.bank,
                server,
                &seller,
                TextComponent::plain(format!("{buyer} оплатил счёт на {}.", ar(amount))),
            );
            window.close();
        });
    }

    fn pay_vending(self: &Arc<Self>, player: &Arc<Player>, terminal: &Terminal, card: &str) {
        let Some(stock) = stock(&self.world, self.pos) else {
            self.end();
            gui::error(player, "У автомата нет сундука с товаром.");
            return;
        };
        let Some(sample) = terminal.sample_stack() else {
            self.end();
            gui::error(player, "Товар закончился.");
            return;
        };
        if !ores::fits(player, &sample, terminal.count) {
            self.end();
            gui::error(player, "В инвентаре нет места для покупки.");
            return;
        }
        let Some(goods) = with_slots(&stock, |items| take(items, &sample, terminal.count)) else {
            self.end();
            gui::error(player, "Товар закончился.");
            return;
        };
        let encoded: Option<Vec<String>> = goods.iter().map(journal::encode).collect();
        let Some(encoded) = encoded else {
            // What can't be written down is not sold: back into the chest it goes.
            with_slots(&stock, |items| {
                for stack in goods {
                    insert(items, stack);
                }
            });
            self.end();
            gui::error(player, "Этот товар автомат продать не может.");
            return;
        };
        let below = self.pos.below();
        let mut entry = Entry::new(VENDING_ENTRY, self.player);
        let op = Uuid::new_v4().to_string();
        entry.op = Some(op.clone());
        entry.amount = i32::try_from(terminal.price).unwrap_or(i32::MAX);
        entry.level = Some(self.world.key.to_string());
        (entry.x, entry.y, entry.z) = (below.x(), below.y(), below.z());
        entry.terminal = Some(terminal.id.to_string());
        entry.to_number = Some(terminal.account.clone());
        entry.items = encoded;
        // Goods leave the chest into the journal before the money moves.
        self.bank.journal.add(entry.clone());
        relabel(self.bank, &self.world, self.pos);

        let seller = terminal.account.clone();
        let price = terminal.price;
        let count = terminal.count;
        let memo = format!("{} × {count}", item_text(&sample));
        // The client shows the item name in its own language; the site gets plain text.
        let what = TextComponent::plain("")
            .add_child(item_name(&sample))
            .add_child(TextComponent::plain(format!(" × {count}")));
        let body = self.pay_body(terminal, &op, card, price, &memo);
        let window = Arc::clone(self);
        self.bank.call("pay", &body, move |server, answer| {
            window.end();
            let player = online(server, window.player);
            if ok(&answer) {
                if window.bank.journal.remove(&entry) {
                    hand_out(window.bank, window.player, player.as_deref(), goods);
                }
                let buyer = player.as_ref().map_or_else(
                    || "Покупатель".to_owned(),
                    |player| player.gameprofile.name().to_owned(),
                );
                if let Some(player) = player {
                    gui::ding(&player);
                    player.send_message(
                        &TextComponent::plain("Куплено: ")
                            .color(Color::Green)
                            .add_child(what.clone())
                            .add_child(TextComponent::plain(format!(" за {}.", ar(price)))),
                    );
                }
                notify_seller(
                    window.bank,
                    server,
                    &seller,
                    TextComponent::plain(format!("{buyer} купил "))
                        .add_child(what)
                        .add_child(TextComponent::plain(format!(" за {}.", ar(price)))),
                );
                window.close();
            } else if is_offline(&answer) {
                if let Some(player) = player {
                    gui::error(
                        &player,
                        "Нет связи с банком. Если оплата прошла, товар выдадим сам, иначе он \
                         вернётся в автомат.",
                    );
                }
                window.close();
            } else {
                if window.bank.journal.remove(&entry) {
                    return_to_stock(server, &entry);
                }
                if let Some(player) = player {
                    gui::error(&player, &gui::error_text(error(&answer)));
                }
            }
        });
    }
}

impl Buttons for Pay {
    fn closing(&self) -> bool {
        self.state.lock().close || !is_terminal(&self.world, self.pos) || self.terminal().is_none()
    }

    fn pos(&self) -> Option<BlockPos> {
        Some(self.pos)
    }

    fn bank(&self) -> &Bank {
        self.bank
    }

    fn clicked(self: Arc<Self>, slot: usize) {
        let bank = self.bank;
        bank.later(move |server| self.act(server, slot));
    }
}

fn pay_window(
    bank: &'static Arc<Bank>,
    player: &Arc<Player>,
    world: Arc<World>,
    pos: BlockPos,
    terminal: &Terminal,
) {
    let vending = terminal.vending();
    let invoice = terminal.active_invoice();
    let amount = if vending {
        terminal.price
    } else {
        invoice.map_or(0, |invoice| invoice.amount)
    };
    let info = if vending {
        let left = stock_count(&world, pos, terminal);
        terminal
            .sample_stack()
            .map_or_else(ItemStack::empty, |sample| {
                shown_goods(
                    &sample,
                    terminal.count,
                    vec![
                        gui::line(
                            &format!("{} шт. за {}", terminal.count, ar(terminal.price)),
                            Color::Aqua,
                        ),
                        gui::line(
                            &format!("В наличии: на {left} {}", purchases(left)),
                            Color::Gray,
                        ),
                    ],
                )
            })
    } else {
        gui::button(
            &vanilla_items::PAPER,
            &format!("Счёт на {}", ar(amount)),
            &[&format!(
                "Получатель: {} ({})",
                terminal.owner, terminal.account
            )],
        )
    };
    let mut buttons = SimpleContainer::new(5);
    buttons.set_item(PAY_INFO, info);
    buttons.set_item(
        PAY_CANCEL,
        gui::button(&vanilla_items::BARRIER, "Отмена", &[]),
    );
    let title = if vending {
        format!("Автомат · {}", terminal.owner)
    } else {
        format!("Касса · {}", terminal.owner)
    };
    let window = Arc::new(Pay {
        bank,
        player: player.gameprofile.id(),
        world,
        pos,
        buttons: buttons.into_shared(),
        invoice: (!vending)
            .then(|| invoice.map(|invoice| invoice.id))
            .flatten(),
        amount,
        cards: card::all(player),
        state: SyncMutex::new(PayState {
            selected: 0,
            busy: false,
            close: false,
        }),
    });
    window.show_card();
    let viewer = Arc::clone(player);
    player.open_menu(gui::title(gui::PAY, &title), move |context| {
        let mut builder = MenuBuilder::new(&vanilla_menu_types::HOPPER, context.container_id);
        builder.section_all_with(ContainerRef::from(&window.buttons), SectionKind::Display);
        builder.player_inventory(&viewer.inventory);
        builder.build(ButtonsKind(window))
    });
}

// ---- after the payment -----------------------------------------------------------------------

/// Paid goods go to the buyer, or wait for their next join.
fn hand_out(bank: &Bank, buyer: Uuid, player: Option<&Player>, goods: Vec<ItemStack>) {
    if let Some(player) = player {
        for stack in goods {
            ores::give_stack(player, stack);
        }
        return;
    }
    let encoded: Vec<String> = goods.iter().filter_map(journal::encode).collect();
    bank.journal.add(Entry::owed(buyer, encoded));
}

/// Tells the owner of `account`, if online, what their terminal just did.
fn notify_seller(bank: &Arc<Bank>, server: &Server, account: &str, text: TextComponent) {
    for player in server.get_players() {
        let uuid = player.gameprofile.id();
        if bank.account_number(uuid).as_deref() != Some(account) {
            continue;
        }
        player.send_message(
            &TextComponent::plain("Терминал: ")
                .color(Color::Gold)
                .add_child(text.clone()),
        );
        bank.sales_seen(uuid);
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// After a crash or an outage: did the payment go through? Yes: the goods go to the buyer,
/// no: back to the chest.
pub(super) fn retry_vending(bank: &Arc<Bank>, entry: Entry) {
    if now_ms().saturating_sub(entry.created_at) < VENDING_RETRY_MS {
        return;
    }
    let Some(op) = entry.op.clone() else {
        return;
    };
    let then = Arc::clone(bank);
    bank.call("op", &json!({"op_id": op}), move |server, answer| {
        if !ok(&answer) || !then.journal.remove(&entry) {
            return;
        }
        let paid = answer.get("op").is_some_and(Value::is_object);
        if !paid {
            return_to_stock(server, &entry);
            return;
        }
        let Ok(buyer) = Uuid::parse_str(&entry.uuid) else {
            log::error!("Bank: a paid purchase of {:?} has no buyer", entry.uuid);
            return;
        };
        let goods: Option<Vec<ItemStack>> = entry
            .items
            .iter()
            .map(|item| journal::decode(item))
            .collect();
        match (online(server, buyer), goods) {
            (Some(player), Some(goods)) => {
                for stack in goods {
                    ores::give_stack(&player, stack);
                }
                gui::info(&player, "Автомат: выдан оплаченный товар.");
            }
            _ => then.journal.add(Entry::owed(buyer, entry.items.clone())),
        }
    });
}

/// The goods of a purchase that was not paid go back into the chest they came from; what
/// does not fit, or has no chest any more, is dropped where the terminal stood.
fn return_to_stock(server: &Server, entry: &Entry) {
    let world = entry
        .level
        .as_deref()
        .and_then(|level| level.parse::<Identifier>().ok())
        .and_then(|key| server.worlds.get(&key).cloned());
    let Some(world) = world else {
        log::error!(
            "Bank: the goods of purchase {:?} have no world to go back to",
            entry.op
        );
        return;
    };
    let pos = BlockPos::new(entry.x, entry.y, entry.z);
    let stock = HopperBlockEntity::get_container_at(&world, pos);
    for item in &entry.items {
        let Some(mut goods) = journal::decode(item) else {
            log::error!("Bank: could not read the goods of purchase {:?}", entry.op);
            continue;
        };
        if let Some(stock) = &stock {
            goods = with_slots(stock, |items| insert(items, goods));
        }
        if !goods.is_empty() {
            let above = DVec3::new(
                f64::from(entry.x) + 0.5,
                f64::from(entry.y) + 1.2,
                f64::from(entry.z) + 0.5,
            );
            world.spawn_item(above, goods);
        }
    }
}

// ---- block callbacks -------------------------------------------------------------------------

/// Adds where the terminal stands to a call.
fn place(body: &mut Value, world: &World, pos: BlockPos) {
    body["world"] = json!(world.key.to_string());
    body["x"] = json!(pos.x());
    body["y"] = json!(pos.y());
    body["z"] = json!(pos.z());
}

/// A player is breaking the terminal at `pos`: a locked one is reported to the site and to
/// its owner.
pub(super) fn broken(world: &Arc<World>, pos: BlockPos, by_name: &str) {
    let Some(bank) = bank() else {
        return;
    };
    let Some(terminal) = bank
        .terminals
        .get(&world.key, pos)
        .filter(|terminal| terminal.locked)
    else {
        return;
    };
    log::info!(
        "Bank: terminal {} at {pos:?} (account {}) broken by {by_name}",
        terminal.id,
        terminal.account
    );
    let mut body = json!({
        "terminal_id": terminal.id.to_string(),
        "action": "break",
        "by_name": by_name,
    });
    place(&mut body, world, pos);
    bank.call("terminal", &body, |_, _| {});
    let line = format!(
        "терминал на {} {} {} сломал {by_name}.",
        pos.x(),
        pos.y(),
        pos.z()
    );
    bank.later(move |server| {
        notify_seller(bank, server, &terminal.account, TextComponent::plain(line));
    });
}

#[cfg(test)]
mod tests {
    use steel_registry::vanilla_items;

    use super::*;

    fn stacks(counts: &[(i32, bool)]) -> Vec<ItemStack> {
        counts
            .iter()
            .map(|&(count, diamond)| {
                if count == 0 {
                    ItemStack::empty()
                } else if diamond {
                    ItemStack::with_count(&vanilla_items::DIAMOND, count)
                } else {
                    ItemStack::with_count(&vanilla_items::DIRT, count)
                }
            })
            .collect()
    }

    fn counts(items: &[ItemStack]) -> Vec<i32> {
        items.iter().map(ItemStack::count).collect()
    }

    #[test]
    fn purchases_are_counted_in_russian() {
        assert_eq!(purchases(1), "покупку");
        assert_eq!(purchases(2), "покупки");
        assert_eq!(purchases(5), "покупок");
        assert_eq!(purchases(11), "покупок");
        assert_eq!(purchases(12), "покупок");
        assert_eq!(purchases(21), "покупку");
        assert_eq!(purchases(24), "покупки");
        assert_eq!(purchases(0), "покупок");
    }

    #[test]
    fn goods_are_taken_from_several_stacks_or_not_at_all() {
        steel_registry::init_vanilla_registry();
        let sample = ItemStack::new(&vanilla_items::DIAMOND);
        let mut items = stacks(&[(3, true), (10, false), (0, false), (4, true)]);
        assert_eq!(count_like(&items, &sample), 7);
        assert!(take(&mut items, &sample, 8).is_none());
        assert_eq!(counts(&items), vec![3, 10, 0, 4]);

        let taken = take(&mut items, &sample, 5);
        assert_eq!(taken.as_deref().map(counts), Some(vec![3, 2]));
        assert_eq!(counts(&items), vec![0, 10, 0, 2]);
        assert!(take(&mut items, &sample, 0).is_none());
    }

    #[test]
    fn returned_goods_fill_stacks_of_the_same_and_then_empty_slots() {
        steel_registry::init_vanilla_registry();
        let mut items = stacks(&[(60, true), (64, false), (0, false)]);
        let rest = insert(
            &mut items,
            ItemStack::with_count(&vanilla_items::DIAMOND, 10),
        );
        assert!(rest.is_empty());
        assert_eq!(counts(&items), vec![64, 64, 6]);

        let rest = insert(&mut items, ItemStack::with_count(&vanilla_items::DIRT, 5));
        assert_eq!(rest.count(), 5);
        assert_eq!(counts(&items), vec![64, 64, 6]);
    }

    #[test]
    fn the_site_gets_a_readable_name_of_the_goods() {
        steel_registry::init_vanilla_registry();
        assert_eq!(title_case("diamond_sword"), "Diamond Sword");
        assert_eq!(
            item_text(&ItemStack::new(&vanilla_items::DEEPSLATE_DIAMOND_ORE)),
            "Deepslate Diamond Ore"
        );
        let mut named = ItemStack::new(&vanilla_items::DIAMOND);
        named.set(CUSTOM_NAME, TextComponent::plain("Алмаз удачи"));
        assert_eq!(item_text(&named), "Алмаз удачи");
    }

    #[test]
    fn the_label_tells_what_the_terminal_is_set_to() {
        steel_registry::init_vanilla_registry();
        let mut terminal = Terminal::new();
        let (text, line) = label(&terminal, 0);
        assert_eq!(text.to_plain(&NoResolutor), "Терминал\nне закреплён");
        assert_eq!(line, "unlocked");

        terminal.locked = true;
        terminal.owner = "seller".to_owned();
        let (text, waiting) = label(&terminal, 0);
        assert_eq!(text.to_plain(&NoResolutor), "Касса · seller\nждёт счёта");
        terminal.set_invoice(12, 60);
        let (text, asking) = label(&terminal, 0);
        assert_eq!(
            text.to_plain(&NoResolutor),
            "Касса · seller\nК оплате: 12 АР"
        );
        assert_ne!(waiting, asking);

        terminal.mode = VENDING.to_owned();
        terminal.price = 3;
        terminal.count = 16;
        let mut named = ItemStack::new(&vanilla_items::DIAMOND);
        named.set(CUSTOM_NAME, TextComponent::plain("Алмаз"));
        terminal.sample = journal::encode(&named);
        let (text, stocked) = label(&terminal, 2);
        assert_eq!(text.to_plain(&NoResolutor), "Алмаз × 16\n3 АР");
        let (text, empty) = label(&terminal, 0);
        assert_eq!(
            text.to_plain(&NoResolutor),
            "Алмаз × 16\n3 АР · нет в наличии"
        );
        assert_ne!(stocked, empty);
    }

    #[test]
    fn the_setup_window_shows_goods_and_price_only_for_vending() {
        steel_registry::init_vanilla_registry();
        let mut terminal = Terminal::new();
        let slots = |terminal: &Terminal| {
            let mut slots: Vec<usize> = Setup::layout(terminal)
                .into_iter()
                .map(|(slot, _)| slot)
                .collect();
            slots.sort_unstable();
            slots
        };
        assert_eq!(slots(&terminal), vec![10, 12, 16, 26]);
        terminal.mode = VENDING.to_owned();
        assert_eq!(
            slots(&terminal),
            vec![10, 12, 14, 16, 19, 20, 21, 22, 23, 26]
        );
    }
}
