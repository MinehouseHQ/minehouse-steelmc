//! «Банкомат», the windows (the Fabric `Atm`).
//!
//! A chest window of three rows: the account in the first, five slots for ores and «Внести»
//! in the second, withdrawals and cards in the third. From it open the amount picker and the
//! name of a new account, typed in an anvil.
//!
//! A click only notes what was pressed: the action itself runs on the next server tick
//! ([`Bank::later`]), outside the window's locks, so it may open another window, give items
//! or call the site.

use std::{mem, ptr, sync::Arc};

use serde_json::{Value, json};
use steel_core::{
    entity::Entity as _, inventory::prelude::*, player::Player, server::Server, world::World,
};
use steel_registry::{
    blocks::block_state_ext::BlockStateExt as _, data_components::vanilla_components::CUSTOM_NAME,
    item_stack::ItemStack, vanilla_items, vanilla_menu_types,
};
use steel_utils::{BlockPos, DowncastType, DowncastTypeKey, locks::SyncMutex, types::GameType};
use text_components::{TextComponent, format::Color};
use uuid::Uuid;

use super::{
    Bank, ar,
    atm_block::ATM_BLOCK,
    bank, body, card, error, gui, is_offline,
    journal::{self, DEPOSIT, Entry},
    ok, online, ores, personal,
};
use crate::content::TERMINAL as TERMINAL_ITEM;

const INFO: usize = 4;
const ORE_SLOTS: [usize; 5] = [10, 11, 12, 13, 14];
const DEPOSIT_SLOT: usize = 16;
const WITHDRAW: usize = 19;
const CARD: usize = 21;
const REISSUE: usize = 22;
const TERMINAL: usize = 24;
const CLOSE: usize = 26;
const SWITCH: usize = 8;
const NEW_ACCOUNT: usize = 0;
/// «Открыть личный счёт»: only when the player sees others' accounts but has no personal one.
const OPEN_PERSONAL: usize = 1;
/// «Открыть счёт» of the window of a player without an account.
const WELCOME_OPEN: usize = 13;
pub(super) const ROWS: usize = 27;

/// The model of the card, for buttons.
const CARD_MODEL: &str = "minehouse-bank:card";
/// The model of the payment terminal, for buttons.
const TERMINAL_MODEL: &str = "minehouse-bank:terminal";

pub(super) fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

pub(super) fn number(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or_default()
}

fn card_of(account: &Value) -> Option<&Value> {
    account.get("card").filter(|card| card.is_object())
}

/// «Личный» or the account's own name.
pub(super) fn title(account: &Value) -> String {
    if personal(account) {
        return "Личный".to_owned();
    }
    account
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_else(|| text(account, "number"))
        .to_owned()
}

/// The id of a non-personal account: the site picks the personal one itself.
pub(super) fn extra_id(account: &Value) -> Option<String> {
    (!personal(account)).then(|| text(account, "account_id").to_owned())
}

fn last4(number: &str) -> String {
    let digits: Vec<char> = number.chars().filter(char::is_ascii_digit).collect();
    digits[digits.len().saturating_sub(4)..].iter().collect()
}

pub(super) fn near(bank: &Bank, player: &Player, pos: BlockPos) -> bool {
    let at = player.position().floor();
    let (dx, dy, dz) = (
        at.x - f64::from(pos.x()),
        at.y - f64::from(pos.y()),
        at.z - f64::from(pos.z()),
    );
    let distance = bank.config.use_distance;
    dx * dx + dy * dy + dz * dz <= distance * distance
}

/// The account card in slot [`INFO`].
fn info_item(account: &Value) -> ItemStack {
    let name = if personal(account) {
        format!("Счёт {}", text(account, "number"))
    } else {
        format!("Счёт «{}» {}", title(account), text(account, "number"))
    };
    let mut lore = vec![gui::line(
        &format!("Баланс: {}", ar(number(account, "balance"))),
        Color::Gray,
    )];
    if let Some(card) = card_of(account) {
        lore.push(gui::line(
            &format!(
                "Карта •••• {}, {}",
                last4(text(card, "number")),
                text(card, "tier_title")
            ),
            Color::Gray,
        ));
        if number(card, "daily_limit") >= 0 {
            lore.push(gui::line(
                &format!(
                    "Сегодня можно потратить ещё {} из {}",
                    ar(number(card, "left_today")),
                    number(card, "daily_limit")
                ),
                Color::DarkGray,
            ));
        }
    } else {
        lore.push(gui::line("Карты нет: нажми «Выдать карту»", Color::Yellow));
    }
    if text(account, "account_status") == "frozen" {
        lore.push(gui::line("Счёт заморожен администрацией", Color::Red));
    }
    lore.push(gui::line(
        "История и блокировка карты: minehouse.pro/bank",
        Color::DarkGray,
    ));
    let mut stack = gui::button_with(&vanilla_items::PAPER, &name, lore);
    gui::model(&mut stack, CARD_MODEL);
    stack
}

fn card_button(name: &str, lore: &[&str]) -> ItemStack {
    let mut stack = gui::button(&vanilla_items::PAPER, name, lore);
    gui::model(&mut stack, CARD_MODEL);
    stack
}

/// The button that goes to the next of the accounts the player may use, and the id of that
/// account. Nothing when there is only one.
fn switch_button(account: &Value, accounts: &[Value]) -> Option<(ItemStack, String)> {
    if accounts.len() < 2 {
        return None;
    }
    let current = text(account, "account_id");
    let index = accounts
        .iter()
        .position(|other| text(other, "id") == current)
        .unwrap_or(0);
    let mut lore: Vec<String> = accounts
        .iter()
        .enumerate()
        .map(|(at, other)| {
            let owner = other
                .get("owner")
                .and_then(Value::as_str)
                .map_or_else(String::new, |owner| format!(" · {owner}"));
            let mark = if at == index { "▶ " } else { "  " };
            format!("{mark}{}{owner}", text(other, "title"))
        })
        .collect();
    lore.push("Нажми, чтобы перейти на следующий.".to_owned());
    let lore: Vec<&str> = lore.iter().map(String::as_str).collect();
    let next = text(accounts.get((index + 1) % accounts.len())?, "id").to_owned();
    let button = gui::button(
        &vanilla_items::BOOK,
        &format!("Счёт: {}", title(account)),
        &lore,
    );
    Some((button, next))
}

/// Opens the ATM at `pos` for `player`: on the account of the card in hand, or on the
/// personal one.
pub fn open(player: &Player, pos: BlockPos) {
    let Some(bank) = bank() else {
        gui::error(player, "Банк пока закрыт.");
        return;
    };
    let card = card::find(player, true);
    open_on(
        bank,
        player.gameprofile.id(),
        player.get_world(),
        pos,
        None,
        card,
    );
}

/// Asks the site for the account and opens the window on it.
fn open_on(
    bank: &Arc<Bank>,
    player: Uuid,
    world: Arc<World>,
    pos: BlockPos,
    account_id: Option<String>,
    card: Option<String>,
) {
    let opened = Arc::clone(bank);
    bank.account(
        player,
        account_id.as_deref(),
        card.as_deref(),
        move |server, answer| {
            let Some(player) = online(server, player) else {
                return;
            };
            if !ok(&answer) {
                gui::error(&player, &gui::error_text(error(&answer)));
                return;
            }
            if !near(&opened, &player, pos) {
                return;
            }
            let account = answer.get("account").filter(|a| a.is_object()).cloned();
            let accounts = answer
                .get("accounts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            Session::show(&opened, &player, world, pos, account, accounts);
        },
    );
}

/// «Купить терминал».
fn terminal_button(price: i32) -> ItemStack {
    let mut terminal = gui::button(
        &vanilla_items::STONE_BUTTON,
        "Купить терминал",
        &[
            &format!("Цена: {} со счёта.", ar(i64::from(price))),
            "Касса или автомат для магазина.",
        ],
    );
    gui::model(&mut terminal, TERMINAL_MODEL);
    terminal
}

/// What a click on the main window asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Action {
    Close,
    OpenAccount,
    NewAccount,
    Switch(String),
    Deposit,
    Withdraw,
    Card { reissue: bool },
    BuyTerminal,
}

struct State {
    /// The account the window is on. None: the player has none yet.
    account: Option<Value>,
    /// Every account the player may use.
    accounts: Vec<Value>,
    /// A call to the site is in flight.
    busy: bool,
    /// The window is to be closed.
    close: bool,
    /// The window is gone.
    gone: bool,
    /// What a payment terminal costs, АР.
    terminal_price: i32,
}

impl State {
    /// Whether the player has a personal account among the ones they may use.
    fn has_personal(&self) -> bool {
        self.accounts
            .iter()
            .any(|account| account.get("personal").and_then(Value::as_bool) == Some(true))
    }

    /// What the window shows, and what each button does.
    fn layout(&self) -> Vec<(usize, ItemStack, Option<Action>)> {
        let close = (
            CLOSE,
            gui::button(&vanilla_items::BARRIER, "Закрыть", &[]),
            Some(Action::Close),
        );
        let Some(account) = &self.account else {
            return vec![
                (
                    WELCOME_OPEN,
                    card_button(
                        "Открыть счёт",
                        &[
                            "Счёт в алмазных рудах и карта MineHouse.",
                            "Открытие бесплатное.",
                        ],
                    ),
                    Some(Action::OpenAccount),
                ),
                close,
            ];
        };
        let mut layout = vec![(INFO, info_item(account), None)];
        if let Some((button, next)) = switch_button(account, &self.accounts) {
            layout.push((SWITCH, button, Some(Action::Switch(next))));
        }
        // Invited into someone else's account before opening one's own: the window opens on
        // that account, so the welcome «Открыть счёт» never shows. Offer the personal one here.
        if !self.has_personal() {
            layout.push((
                OPEN_PERSONAL,
                card_button(
                    "Открыть личный счёт",
                    &[
                        "Свой счёт в алмазных рудах и карта на него.",
                        "Открытие бесплатное.",
                    ],
                ),
                Some(Action::OpenAccount),
            ));
        }
        layout.push((
            NEW_ACCOUNT,
            gui::button(
                &vanilla_items::WRITABLE_BOOK,
                "Открыть ещё счёт",
                &[
                    "Отдельный счёт со своим названием,",
                    "например для магазина или клана.",
                    "Участников добавляют на minehouse.pro/bank.",
                ],
            ),
            Some(Action::NewAccount),
        ));
        layout.push((
            DEPOSIT_SLOT,
            gui::button(
                &vanilla_items::LIME_DYE,
                "Внести",
                &[
                    "Положи алмазную руду в слоты слева.",
                    "1 руда = 1 АР, глубинная тоже.",
                ],
            ),
            Some(Action::Deposit),
        ));
        layout.push((
            WITHDRAW,
            gui::button(
                &vanilla_items::DEEPSLATE_DIAMOND_ORE,
                "Снять АР",
                &["Выдаёт глубинную алмазную руду."],
            ),
            Some(Action::Withdraw),
        ));
        layout.push((
            CARD,
            card_button(
                "Выдать карту",
                &[
                    "Карта на этот счёт, ещё одна копия твоей.",
                    "Платить ей можешь только ты.",
                ],
            ),
            Some(Action::Card { reissue: false }),
        ));
        layout.push((
            REISSUE,
            gui::button(
                &vanilla_items::SHEARS,
                "Перевыпустить карту",
                &[
                    "Старая карта перестанет работать.",
                    "Если карту украли или потеряли.",
                ],
            ),
            Some(Action::Card { reissue: true }),
        ));
        let terminal = terminal_button(self.terminal_price);
        layout.push((TERMINAL, terminal, Some(Action::BuyTerminal)));
        layout.push(close);
        layout
    }
}

/// One open ATM window.
struct Session {
    bank: Arc<Bank>,
    player: Uuid,
    world: Arc<World>,
    pos: BlockPos,
    buttons: Shared<SimpleContainer>,
    ores: Shared<SimpleContainer>,
    state: SyncMutex<State>,
}

impl Session {
    fn show(
        bank: &Arc<Bank>,
        player: &Arc<Player>,
        world: Arc<World>,
        pos: BlockPos,
        account: Option<Value>,
        accounts: Vec<Value>,
    ) {
        let background = if account.is_some() {
            gui::ATM
        } else {
            gui::WELCOME
        };
        let session = Arc::new(Self {
            bank: Arc::clone(bank),
            player: player.gameprofile.id(),
            world,
            pos,
            buttons: SimpleContainer::new(ROWS).into_shared(),
            ores: SimpleContainer::new(ORE_SLOTS.len()).into_shared(),
            state: SyncMutex::new(State {
                account,
                accounts,
                busy: false,
                close: false,
                gone: false,
                terminal_price: bank.config.terminal_price,
            }),
        });
        session.draw();
        let welcome = background == gui::WELCOME;
        let viewer = Arc::clone(player);
        player.open_menu(gui::title(background, "Банкомат"), move |context| {
            let mut builder =
                MenuBuilder::new(&vanilla_menu_types::GENERIC_9X3, context.container_id);
            let buttons = ContainerRef::from(&session.buttons);
            if welcome {
                // No account yet: nothing to pay in, every slot is a button.
                builder.section_at(&buttons, 0..ROWS, SectionKind::Display);
                builder.player_inventory(&viewer.inventory);
                return builder.build(MainKind {
                    session,
                    takes_ores: false,
                });
            }
            builder.section_at(&buttons, 0..ORE_SLOTS[0], SectionKind::Display);
            let ores = builder.section_all_with(
                ContainerRef::from(&session.ores),
                SectionKind::restricted(|_, item| ores::is_ore(item)),
            );
            builder.section_at(&buttons, ORE_SLOTS[4] + 1..ROWS, SectionKind::Display);
            let own = builder.player_inventory(&viewer.inventory);
            builder.route(ores, own.all(), FillDirection::Backward);
            builder.route(own.all(), ores, FillDirection::Forward);
            // Ores left in the slots go back to the player.
            builder.drain(ores);
            builder.build(MainKind {
                session,
                takes_ores: true,
            })
        });
    }

    fn draw(&self) {
        let layout = self.state.lock().layout();
        let mut buttons = self.buttons.lock();
        for slot in 0..ROWS {
            buttons.set_item(slot, ItemStack::empty());
        }
        for (slot, stack, _) in layout {
            buttons.set_item(slot, stack);
        }
    }

    /// The window's player, if the window is still theirs to act in.
    fn viewer(&self, server: &Server) -> Option<Arc<Player>> {
        if self.state.lock().gone {
            return None;
        }
        online(server, self.player)
    }

    fn close(&self) {
        self.state.lock().close = true;
    }

    /// Marks the window busy. False when it already is.
    fn begin(&self) -> bool {
        let mut state = self.state.lock();
        !mem::replace(&mut state.busy, true)
    }

    fn end(&self) {
        self.state.lock().busy = false;
    }

    fn extra_id(&self) -> Option<String> {
        self.state.lock().account.as_ref().and_then(extra_id)
    }

    /// Shows the account the site answered with.
    fn refresh(&self, answer: &Value) {
        if answer.get("number").is_some() {
            if personal(answer) {
                self.bank
                    .remember_account(self.player, text(answer, "number"));
            }
            self.state.lock().account = Some(answer.clone());
        }
        if !self.state.lock().gone {
            self.draw();
        }
    }

    /// Opens the window again on `account_id`, with what the site says now.
    fn reopen(&self, account_id: Option<String>) {
        self.close();
        open_on(
            &self.bank,
            self.player,
            Arc::clone(&self.world),
            self.pos,
            account_id,
            None,
        );
    }

    fn clicked(self: &Arc<Self>, slot: usize) {
        let action = {
            let state = self.state.lock();
            state
                .layout()
                .into_iter()
                .find_map(|(at, _, action)| (at == slot).then_some(action).flatten())
        };
        let Some(action) = action else {
            return;
        };
        let session = Arc::clone(self);
        self.bank.later(move |server| session.act(server, action));
    }

    fn act(self: &Arc<Self>, server: &Server, action: Action) {
        let Some(player) = self.viewer(server) else {
            return;
        };
        match action {
            Action::Close => self.close(),
            Action::OpenAccount => self.open_account(&player),
            Action::Deposit => self.deposit(&player),
            Action::Card { reissue } => self.card(&player, reissue),
            Action::BuyTerminal => self.buy_terminal(&player),
            Action::Switch(next) => {
                if !self.state.lock().busy {
                    gui::click(&player);
                    self.reopen(Some(next));
                }
            }
            Action::NewAccount => {
                if !self.state.lock().busy {
                    gui::click(&player);
                    self.close();
                    name_window(&self.bank, &player, Arc::clone(&self.world), self.pos);
                }
            }
            Action::Withdraw => {
                let account = self.state.lock().account.clone();
                if let Some(account) = account {
                    gui::click(&player);
                    self.close();
                    amount_window(
                        &self.bank,
                        &player,
                        Arc::clone(&self.world),
                        self.pos,
                        account,
                    );
                }
            }
        }
    }

    fn give_card(player: &Player, account: &Value) {
        let Some(card) = card_of(account) else {
            return;
        };
        let owner = account
            .get("owner")
            .and_then(Value::as_str)
            .unwrap_or_else(|| player.gameprofile.name());
        let named = (!personal(account)).then(|| title(account));
        if let Some(stack) = card::make(card, owner, named.as_deref()) {
            ores::give_stack(player, stack);
        }
    }

    fn open_account(self: &Arc<Self>, player: &Player) {
        if !self.begin() {
            return;
        }
        gui::click(player);
        let mut body = body(self.player);
        body["reissue"] = json!(false);
        let session = Arc::clone(self);
        self.bank.call("open", &body, move |server, answer| {
            session.end();
            let Some(player) = online(server, session.player) else {
                return;
            };
            if !ok(&answer) {
                gui::error(&player, &gui::error_text(error(&answer)));
                return;
            }
            Self::give_card(&player, &answer);
            gui::ding(&player);
            gui::info(
                &player,
                &format!(
                    "Счёт {} открыт, карта у тебя в инвентаре.",
                    text(&answer, "number")
                ),
            );
            // The window is opened again: its background and the list of accounts change.
            if !session.state.lock().gone {
                let account_id = answer.get("account_id").and_then(Value::as_str);
                session.reopen(account_id.map(str::to_owned));
            }
        });
    }

    /// Sells a payment terminal: the price goes from the account to the server treasury.
    fn buy_terminal(self: &Arc<Self>, player: &Player) {
        let card = {
            let state = self.state.lock();
            if state.busy {
                return;
            }
            state
                .account
                .as_ref()
                .and_then(card_of)
                .map(|card| text(card, "id").to_owned())
        };
        let Some(card) = card else {
            gui::error(player, &gui::error_text("no_card"));
            return;
        };
        if !ores::fits(player, &TERMINAL_ITEM.stack(1), 1) {
            gui::error(player, "В инвентаре нет места для терминала.");
            return;
        }
        if !self.begin() {
            return;
        }
        gui::click(player);
        let price = self.bank.config.terminal_price;
        let mut body = body(self.player);
        body["op_id"] = json!(Uuid::new_v4().to_string());
        body["card_id"] = json!(card);
        body["amount"] = json!(price);
        let session = Arc::clone(self);
        self.bank
            .call("buy_terminal", &body, move |server, answer| {
                session.end();
                let Some(player) = online(server, session.player) else {
                    return;
                };
                if !ok(&answer) {
                    gui::error(&player, &gui::error_text(error(&answer)));
                    return;
                }
                ores::give_stack(&player, TERMINAL_ITEM.stack(1));
                gui::ding(&player);
                gui::info(
                    &player,
                    &format!(
                        "Терминал куплен за {}. Поставь его на стол или на сундук.",
                        ar(i64::from(price))
                    ),
                );
                session.refresh(&answer);
            });
    }

    fn card(self: &Arc<Self>, player: &Player, reissue: bool) {
        if !self.begin() {
            return;
        }
        gui::click(player);
        let mut body = body(self.player);
        body["reissue"] = json!(reissue);
        if let Some(id) = self.extra_id() {
            body["account_id"] = json!(id);
        }
        let session = Arc::clone(self);
        self.bank.call("open", &body, move |server, answer| {
            session.end();
            let Some(player) = online(server, session.player) else {
                return;
            };
            if !ok(&answer) {
                gui::error(&player, &gui::error_text(error(&answer)));
                return;
            }
            Self::give_card(&player, &answer);
            gui::info(
                &player,
                if reissue {
                    "Новая карта выдана, старая больше не работает."
                } else {
                    "Карта выдана."
                },
            );
            session.refresh(&answer);
        });
    }

    fn deposit(self: &Arc<Self>, player: &Player) {
        if self.state.lock().busy {
            return;
        }
        if !matches!(player.game_mode(), GameType::Survival | GameType::Adventure) {
            gui::error(player, "Вносить руду можно только в режиме выживания.");
            return;
        }
        // Journal first: the ores are out of the world now and must end up on the account or
        // back.
        let entry = {
            let mut ores = self.ores.lock();
            let stacks: Vec<ItemStack> = (0..ORE_SLOTS.len())
                .map(|slot| ores.remove_item_no_update(slot))
                .filter(|stack| !stack.is_empty())
                .collect();
            Entry {
                op: Some(Uuid::new_v4().to_string()),
                amount: stacks.iter().map(ItemStack::count).sum(),
                account: self.extra_id(),
                items: stacks.iter().filter_map(journal::encode).collect(),
                ..Entry::new(DEPOSIT, self.player)
            }
        };
        if entry.amount == 0 {
            gui::error(player, "Положи алмазную руду в слоты слева от кнопки.");
            return;
        }
        self.begin();
        gui::click(player);
        self.bank.journal.add(entry.clone());
        let session = Arc::clone(self);
        let amount = entry.amount;
        send_deposit(&self.bank, entry, move |server, answer| {
            session.end();
            let Some(player) = online(server, session.player) else {
                return;
            };
            if ok(&answer) {
                gui::ding(&player);
                gui::info(&player, &format!("Внесено {}.", ar(i64::from(amount))));
                if answer.get("number").is_some() {
                    session.refresh(&answer);
                    return;
                }
                let shown = Arc::clone(&session);
                session.bank.account(
                    session.player,
                    session.extra_id().as_deref(),
                    None,
                    move |_, answer| {
                        if ok(&answer)
                            && let Some(account) = answer.get("account").filter(|a| a.is_object())
                        {
                            shown.refresh(account);
                        }
                    },
                );
            } else if is_offline(&answer) {
                gui::error(
                    &player,
                    "Нет связи с банком. Руды не пропадут: они зачислятся, когда связь вернётся.",
                );
            } else {
                gui::error(
                    &player,
                    &format!("{} Руды возвращены.", gui::error_text(error(&answer))),
                );
            }
        });
    }
}

/// Sends a journaled deposit; a real answer settles the entry, refunding the ores on an error.
fn send_deposit(
    bank: &Arc<Bank>,
    entry: Entry,
    then: impl FnOnce(&Arc<Server>, Value) + Send + 'static,
) {
    let mut body = json!({"op_id": entry.op, "uuid": entry.uuid, "amount": entry.amount});
    if let Some(account) = &entry.account {
        body["account_id"] = json!(account);
    }
    let settled = Arc::clone(bank);
    bank.call("deposit", &body, move |server, answer| {
        if ok(&answer) {
            settled.journal.remove(&entry);
        } else if !is_offline(&answer) && settled.journal.remove(&entry) {
            refund(&settled, server, &entry);
        }
        then(server, answer);
    });
}

/// Sends again a deposit the site has not confirmed.
pub(super) fn retry_deposit(bank: &Arc<Bank>, entry: Entry) {
    let (player, amount) = (entry.uuid.clone(), entry.amount);
    send_deposit(bank, entry, move |server, answer| {
        if !ok(&answer) {
            return;
        }
        let player = Uuid::parse_str(&player).ok();
        if let Some(player) = player.and_then(|uuid| online(server, uuid)) {
            gui::info(
                &player,
                &format!(
                    "Банк: зачислено {} по прошлому внесению.",
                    ar(i64::from(amount))
                ),
            );
        }
    });
}

/// The ores of a refused deposit: to the player now, or owed until they join.
fn refund(bank: &Bank, server: &Server, entry: &Entry) {
    let Ok(uuid) = Uuid::parse_str(&entry.uuid) else {
        log::error!("Bank: a refused deposit of {:?} has no player", entry.uuid);
        return;
    };
    let items: Option<Vec<ItemStack>> = entry.items.iter().map(|i| journal::decode(i)).collect();
    match (online(server, uuid), items) {
        (Some(player), Some(items)) => {
            for item in items {
                ores::give_stack(&player, item);
            }
        }
        _ => bank.journal.add(Entry::owed(uuid, entry.items.clone())),
    }
}

/// The main window: buttons in a chest, five slots that take ores.
struct MainKind {
    session: Arc<Session>,
    /// Whether the middle slots take ores; in the welcome window they are buttons.
    takes_ores: bool,
}

// SAFETY: This key is owned by MineHouse and uniquely identifies `MainKind`.
unsafe impl DowncastType for MainKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("minehouse:menu/bank_atm");
}

impl MenuKind for MainKind {
    fn on_slot_clicked(
        &mut self,
        _behavior: &mut MenuBehavior,
        _guard: &mut ContainerLockGuard,
        click: Click,
        _player: &Player,
    ) -> ClickOutcome {
        match click.slot() {
            Some(slot) if slot < ROWS && !(self.takes_ores && ORE_SLOTS.contains(&slot)) => {
                self.session.clicked(slot);
                ClickOutcome::Consume
            }
            _ => ClickOutcome::Fallthrough,
        }
    }

    fn still_valid(&self, _behavior: &MenuBehavior, player: &Player) -> bool {
        !self.session.state.lock().close
            && near(&self.session.bank, player, self.session.pos)
            && ptr::eq(
                self.session
                    .world
                    .get_block_state(self.session.pos)
                    .get_block(),
                &raw const ATM_BLOCK,
            )
    }

    fn removed(&mut self, _behavior: &mut MenuBehavior, _player: &Player) {
        self.session.state.lock().gone = true;
    }
}

/// A window of buttons only: what it shows and what a click does.
pub(super) trait Buttons: Send + Sync + 'static {
    /// The window is to be closed.
    fn closing(&self) -> bool;
    /// The block the window belongs to: it closes when the player walks away. None: a window
    /// that is used from anywhere.
    fn pos(&self) -> Option<BlockPos>;
    fn bank(&self) -> &Bank;
    fn clicked(self: Arc<Self>, slot: usize);
    fn renamed(&self, _name: String) {}
    fn removed(&self) {}
}

pub(super) struct ButtonsKind(pub(super) Arc<dyn Buttons>);

// SAFETY: This key is owned by MineHouse and uniquely identifies `ButtonsKind`.
unsafe impl DowncastType for ButtonsKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("minehouse:menu/bank_buttons");
}

impl MenuKind for ButtonsKind {
    fn on_slot_clicked(
        &mut self,
        _behavior: &mut MenuBehavior,
        _guard: &mut ContainerLockGuard,
        click: Click,
        _player: &Player,
    ) -> ClickOutcome {
        let Some(slot) = click.slot() else {
            return ClickOutcome::Fallthrough;
        };
        Arc::clone(&self.0).clicked(slot);
        ClickOutcome::Consume
    }

    fn on_rename(&mut self, _behavior: &mut MenuBehavior, name: String, _player: &Player) {
        self.0.renamed(name);
    }

    fn still_valid(&self, _behavior: &MenuBehavior, player: &Player) -> bool {
        !self.0.closing()
            && self
                .0
                .pos()
                .is_none_or(|pos| near(self.0.bank(), player, pos))
    }

    fn removed(&mut self, _behavior: &mut MenuBehavior, _player: &Player) {
        self.0.removed();
    }
}

/// How much the amount picker may go up to for a withdrawal from `account`.
fn max_withdraw(config_max: i32, account: &Value) -> i32 {
    let mut left = i64::from(config_max);
    if let Some(card) = card_of(account) {
        let today = number(card, "left_today");
        // -1: no daily limit.
        if today >= 0 {
            left = left.min(today);
        }
    }
    left = left.min(number(account, "balance"));
    i32::try_from(left.max(1)).unwrap_or(1)
}

/// The steps of the amount picker by slot: −64 −8 −1 on the left, +1 +8 +64 on the right.
pub(super) const STEPS: [(usize, i32); 6] =
    [(9, -64), (10, -8), (11, -1), (15, 1), (16, 8), (17, 64)];
pub(super) const AMOUNT_SHOWN: usize = 13;
pub(super) const AMOUNT_BACK: usize = 18;
pub(super) const AMOUNT_OK: usize = 22;

/// The amount picker of a withdrawal.
struct Amount {
    bank: Arc<Bank>,
    player: Uuid,
    world: Arc<World>,
    pos: BlockPos,
    account: Value,
    buttons: Shared<SimpleContainer>,
    /// The amount, whether a call is in flight, whether the window is to be closed.
    state: SyncMutex<(i32, bool, bool)>,
}

impl Amount {
    fn max(&self) -> i32 {
        max_withdraw(self.bank.config.max_withdraw, &self.account)
    }

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
            &ar(i64::from(amount)),
            &[&format!(
                "Можно снять сейчас: до {}",
                ar(i64::from(self.max()))
            )],
        );
        shown.set_count(amount.clamp(1, 64));
        buttons.set_item(AMOUNT_SHOWN, shown);
        buttons.set_item(
            AMOUNT_OK,
            gui::button(
                &vanilla_items::LIME_DYE,
                &format!("Снять {}", ar(i64::from(amount))),
                &[],
            ),
        );
        buttons.set_item(
            AMOUNT_BACK,
            gui::button(&vanilla_items::ARROW, "Назад", &[]),
        );
    }

    fn act(self: &Arc<Self>, server: &Server, slot: usize) {
        let Some(player) = online(server, self.player) else {
            return;
        };
        if let Some((_, delta)) = STEPS.iter().find(|(at, _)| *at == slot) {
            gui::click(&player);
            {
                let mut state = self.state.lock();
                state.0 = (state.0 + delta).clamp(1, self.max());
            }
            self.draw();
        } else if slot == AMOUNT_BACK {
            gui::click(&player);
            self.state.lock().2 = true;
            open_on(
                &self.bank,
                self.player,
                Arc::clone(&self.world),
                self.pos,
                extra_id(&self.account),
                None,
            );
        } else if slot == AMOUNT_OK {
            self.confirm(&player);
        }
    }

    fn confirm(self: &Arc<Self>, player: &Player) {
        let (amount, busy, _) = *self.state.lock();
        if busy {
            return;
        }
        let Some(card) = card_of(&self.account) else {
            gui::error(player, &gui::error_text("no_card"));
            return;
        };
        let ore = ItemStack::new(&vanilla_items::DEEPSLATE_DIAMOND_ORE);
        if !ores::fits(player, &ore, amount) {
            gui::error(
                player,
                &format!("В инвентаре нет места для {}.", ar(i64::from(amount))),
            );
            return;
        }
        self.state.lock().1 = true;
        gui::click(player);
        let op = Uuid::new_v4().to_string();
        let mut body = body(self.player);
        body["op_id"] = json!(op);
        body["card_id"] = json!(text(card, "id"));
        body["amount"] = json!(amount);
        let window = Arc::clone(self);
        self.bank.call("withdraw", &body, move |server, answer| {
            window.state.lock().1 = false;
            // Whoever left meanwhile gets the ores on next join: the site still has the
            // withdrawal as not handed out.
            let Some(player) = online(server, window.player) else {
                return;
            };
            if !ok(&answer) {
                let code = error(&answer);
                let mut line = gui::error_text(code);
                if code == "limit" && answer.get("left_today").is_some() {
                    line = format!(
                        "{line} Осталось на сегодня: {}.",
                        ar(number(&answer, "left_today"))
                    );
                }
                gui::error(&player, &line);
                return;
            }
            ores::give(&player, amount);
            window.bank.delivered(&op, window.player, amount);
            gui::ding(&player);
            gui::info(&player, &format!("Снято {}.", ar(i64::from(amount))));
            window.state.lock().2 = true;
        });
    }
}

impl Buttons for Amount {
    fn closing(&self) -> bool {
        self.state.lock().2
    }

    fn pos(&self) -> Option<BlockPos> {
        Some(self.pos)
    }

    fn bank(&self) -> &Bank {
        &self.bank
    }

    fn clicked(self: Arc<Self>, slot: usize) {
        let bank = Arc::clone(&self.bank);
        bank.later(move |server| self.act(server, slot));
    }
}

fn amount_window(
    bank: &Arc<Bank>,
    player: &Arc<Player>,
    world: Arc<World>,
    pos: BlockPos,
    account: Value,
) {
    let window = Arc::new(Amount {
        bank: Arc::clone(bank),
        player: player.gameprofile.id(),
        world,
        pos,
        account,
        buttons: SimpleContainer::new(ROWS).into_shared(),
        state: SyncMutex::new((1, false, false)),
    });
    window.draw();
    let viewer = Arc::clone(player);
    player.open_menu(gui::title(gui::AMOUNT, "Снять АР"), move |context| {
        let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X3, context.container_id);
        builder.section_all_with(ContainerRef::from(&window.buttons), SectionKind::Display);
        builder.player_inventory(&viewer.inventory);
        builder.build(ButtonsKind(window))
    });
}

const NAME_BACK: usize = 1;
const NAME_OK: usize = 2;
/// What the name field starts with.
const NAME_DEFAULT: &str = "Магазин";

/// The name of a new extra account, typed in an anvil window.
struct AccountName {
    bank: Arc<Bank>,
    player: Uuid,
    world: Arc<World>,
    pos: BlockPos,
    /// The name typed so far, whether a call is in flight, whether the window is to be closed.
    state: SyncMutex<(String, bool, bool)>,
}

/// The name as the site takes it: trimmed, 1 to 40 characters.
fn account_title(typed: &str) -> Option<&str> {
    let title = typed.trim();
    (1..=40).contains(&title.chars().count()).then_some(title)
}

impl AccountName {
    fn act(self: &Arc<Self>, server: &Server, slot: usize) {
        let Some(player) = online(server, self.player) else {
            return;
        };
        if slot == NAME_BACK {
            gui::click(&player);
            self.state.lock().2 = true;
            open_on(
                &self.bank,
                self.player,
                Arc::clone(&self.world),
                self.pos,
                None,
                card::find(&player, true),
            );
        } else if slot == NAME_OK {
            self.confirm(&player);
        }
    }

    fn confirm(self: &Arc<Self>, player: &Player) {
        let (typed, busy, _) = self.state.lock().clone();
        if busy {
            return;
        }
        let Some(title) = account_title(&typed).map(str::to_owned) else {
            gui::error(player, &gui::error_text("bad_title"));
            return;
        };
        self.state.lock().1 = true;
        gui::click(player);
        let mut body = body(self.player);
        body["title"] = json!(title);
        let window = Arc::clone(self);
        self.bank.call("create", &body, move |server, answer| {
            window.state.lock().1 = false;
            let Some(player) = online(server, window.player) else {
                return;
            };
            if !ok(&answer) {
                gui::error(&player, &gui::error_text(error(&answer)));
                return;
            }
            if let Some(card) = card_of(&answer) {
                let owner = answer
                    .get("owner")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| player.gameprofile.name());
                if let Some(stack) = card::make(card, owner, Some(&title)) {
                    ores::give_stack(&player, stack);
                }
            }
            gui::ding(&player);
            gui::info(
                &player,
                &format!(
                    "Счёт «{title}» {} открыт, карта на него у тебя в инвентаре.",
                    text(&answer, "number")
                ),
            );
            window.state.lock().2 = true;
            open_on(
                &window.bank,
                window.player,
                Arc::clone(&window.world),
                window.pos,
                answer
                    .get("account_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                None,
            );
        });
    }
}

impl Buttons for AccountName {
    fn closing(&self) -> bool {
        self.state.lock().2
    }

    fn pos(&self) -> Option<BlockPos> {
        Some(self.pos)
    }

    fn bank(&self) -> &Bank {
        &self.bank
    }

    fn clicked(self: Arc<Self>, slot: usize) {
        let bank = Arc::clone(&self.bank);
        bank.later(move |server| self.act(server, slot));
    }

    fn renamed(&self, name: String) {
        self.state.lock().0 = name;
    }
}

fn name_window(bank: &Arc<Bank>, player: &Arc<Player>, world: Arc<World>, pos: BlockPos) {
    let window = Arc::new(AccountName {
        bank: Arc::clone(bank),
        player: player.gameprofile.id(),
        world,
        pos,
        state: SyncMutex::new((NAME_DEFAULT.to_owned(), false, false)),
    });
    // The text field of an anvil shows the name of the item in the first slot.
    let mut typed = ItemStack::new(&vanilla_items::PAPER);
    typed.set(CUSTOM_NAME, TextComponent::plain(NAME_DEFAULT));
    let mut buttons = SimpleContainer::new(3);
    buttons.set_item(0, typed);
    buttons.set_item(NAME_BACK, gui::button(&vanilla_items::ARROW, "Назад", &[]));
    buttons.set_item(
        NAME_OK,
        gui::button(
            &vanilla_items::LIME_DYE,
            "Открыть счёт",
            &["До 40 символов"],
        ),
    );
    let buttons = buttons.into_shared();
    let viewer = Arc::clone(player);
    player.open_menu(
        TextComponent::plain("Название счёта"),
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

#[cfg(test)]
mod tests {
    use text_components::resolving::NoResolutor;

    use super::*;

    fn account() -> Value {
        json!({
            "account_id": "a-1", "number": "MH-0000-0042", "balance": 120, "personal": true,
            "account_status": "active",
            "card": {"id": "c-1", "number": "MH 1234 5678", "tier_title": "Классическая",
                     "daily_limit": 100, "left_today": 30},
        })
    }

    fn state(account: Option<Value>, accounts: Vec<Value>) -> State {
        State {
            account,
            accounts,
            busy: false,
            close: false,
            gone: false,
            terminal_price: 5,
        }
    }

    fn lore(stack: &ItemStack) -> Vec<String> {
        use steel_registry::data_components::vanilla_components::{ItemLore, LORE};
        stack
            .get(LORE)
            .into_iter()
            .flat_map(ItemLore::lines)
            .map(|line| line.to_plain(&NoResolutor))
            .collect()
    }

    #[test]
    fn a_player_without_an_account_sees_only_open_and_close() {
        steel_registry::init_vanilla_registry();
        let layout = state(None, Vec::new()).layout();
        let actions: Vec<(usize, Option<Action>)> =
            layout.into_iter().map(|(slot, _, a)| (slot, a)).collect();
        assert_eq!(
            actions,
            vec![(13, Some(Action::OpenAccount)), (26, Some(Action::Close))]
        );
    }

    #[test]
    fn the_window_of_an_account_has_its_buttons_where_the_fabric_mod_had_them() {
        steel_registry::init_vanilla_registry();
        let accounts = vec![json!({"id": "a-1", "title": "Личный", "personal": true})];
        let layout = state(Some(account()), accounts).layout();
        let mut slots: Vec<usize> = layout.iter().map(|(slot, _, _)| *slot).collect();
        slots.sort_unstable();
        // No switch with one account, no «Открыть личный счёт» with a personal one.
        assert_eq!(slots, vec![0, 4, 16, 19, 21, 22, 24, 26]);
        assert!(slots.iter().all(|slot| !ORE_SLOTS.contains(slot)));
    }

    #[test]
    fn the_account_card_tells_the_balance_the_card_and_the_limit() {
        steel_registry::init_vanilla_registry();
        assert_eq!(
            lore(&info_item(&account())),
            vec![
                "Баланс: 120 АР",
                "Карта •••• 5678, Классическая",
                "Сегодня можно потратить ещё 30 АР из 100",
                "История и блокировка карты: minehouse.pro/bank",
            ]
        );
        let bare = json!({"number": "MH-1", "balance": 0, "account_status": "frozen"});
        assert_eq!(
            lore(&info_item(&bare)),
            vec![
                "Баланс: 0 АР",
                "Карты нет: нажми «Выдать карту»",
                "Счёт заморожен администрацией",
                "История и блокировка карты: minehouse.pro/bank",
            ]
        );
    }

    #[test]
    fn the_switch_goes_to_the_next_account_and_wraps_around() {
        steel_registry::init_vanilla_registry();
        let accounts = vec![
            json!({"id": "a-1", "title": "Личный", "personal": true}),
            json!({"id": "a-2", "title": "Магазин", "owner": "Alex"}),
        ];
        let (button, next) =
            switch_button(&account(), &accounts).unwrap_or_else(|| panic!("two accounts"));
        assert_eq!(next, "a-2");
        assert_eq!(
            lore(&button),
            vec![
                "▶ Личный",
                "  Магазин · Alex",
                "Нажми, чтобы перейти на следующий."
            ]
        );
        let shop = json!({"account_id": "a-2", "number": "MH-2", "personal": false,
                          "title": "Магазин"});
        let (_, next) = switch_button(&shop, &accounts).unwrap_or_else(|| panic!("two accounts"));
        assert_eq!(next, "a-1");
        assert_eq!(switch_button(&account(), &accounts[..1]).map(|b| b.1), None);
    }

    #[test]
    fn a_shared_account_is_named_and_asked_for_by_its_id() {
        let shop = json!({"account_id": "a-2", "number": "MH-2", "personal": false,
                          "title": "Магазин"});
        assert_eq!(title(&shop), "Магазин");
        assert_eq!(extra_id(&shop).as_deref(), Some("a-2"));
        assert_eq!(title(&account()), "Личный");
        assert_eq!(extra_id(&account()), None);
        let unnamed = json!({"account_id": "a-3", "number": "MH-3", "personal": false});
        assert_eq!(title(&unnamed), "MH-3");
    }

    #[test]
    fn a_withdrawal_is_capped_by_the_setting_the_limit_and_the_balance() {
        assert_eq!(max_withdraw(640, &account()), 30);
        let mut rich = account();
        rich["card"]["left_today"] = json!(-1);
        assert_eq!(max_withdraw(640, &rich), 120);
        rich["balance"] = json!(5000);
        assert_eq!(max_withdraw(640, &rich), 640);
        rich["balance"] = json!(0);
        assert_eq!(max_withdraw(640, &rich), 1);
    }

    #[test]
    fn the_last_four_digits_of_a_card_number() {
        assert_eq!(last4("MH 1234 5678"), "5678");
        assert_eq!(last4("MH 12"), "12");
        assert_eq!(last4(""), "");
    }

    #[test]
    fn the_name_of_an_account_is_trimmed_and_1_to_40_characters() {
        assert_eq!(account_title("  Магазин "), Some("Магазин"));
        assert_eq!(account_title("   "), None);
        assert_eq!(account_title(&"я".repeat(40)).map(str::len), Some(80));
        assert_eq!(account_title(&"я".repeat(41)), None);
    }
}
