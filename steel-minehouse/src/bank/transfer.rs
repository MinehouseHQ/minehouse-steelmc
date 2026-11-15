//! «Переводы»: АР from an account the player may use to any account by its number (the Fabric
//! `Transfer`).
//!
//! No card and no ATM: `/transfer` opens the window from anywhere. On the Fabric server the
//! entry was a line of the server menu, which Steel does not have yet. The amount picker is the
//! ATM's, the number is typed in an anvil window and checked with `resolve` before the transfer
//! is offered.

use std::sync::Arc;

use serde_json::{Value, json};
use steel_core::{
    command::{
        CommandContext, CommandError, CommandNode, CommandRegistration, CommandRegistrationError,
        CommandRegistry, literal,
    },
    inventory::{
        container::{Container as _, SimpleContainer},
        lock::ContainerRef,
        menu::{MenuBuilder, SectionKind},
    },
    player::Player,
    server::Server,
};
use steel_registry::{
    data_components::vanilla_components::CUSTOM_NAME, item_stack::ItemStack, vanilla_items,
    vanilla_menu_types,
};
use steel_utils::{
    BlockPos, Identifier,
    locks::{IntoShared as _, Shared, SyncMutex},
};
use text_components::{Modifier as _, TextComponent, format::Color};
use uuid::Uuid;

use super::{
    Bank, ar,
    atm::{Buttons, ButtonsKind, ROWS, STEPS, extra_id, number, text, title},
    bank, body, error, gui, ok, online, personal,
};

const ACCOUNT: usize = 4;
const AMOUNT: usize = 13;
const RECIPIENT: usize = 18;
const CONFIRM: usize = 22;
const CLOSE: usize = 26;

const NUMBER_BACK: usize = 1;
const NUMBER_OK: usize = 2;

/// The most one transfer moves.
const MAX: i64 = 1_000_000;

/// What the player has chosen so far. It outlives the windows: the number input opens over
/// the amount window and gives the draft back.
#[derive(Clone, Debug)]
struct Draft {
    /// The account the money leaves.
    account: Value,
    /// Every account the player may use.
    accounts: Vec<Value>,
    /// The number of the receiving account and the nickname of its owner.
    to: Option<(String, String)>,
    amount: i64,
}

impl Draft {
    fn balance(&self) -> i64 {
        number(&self.account, "balance")
    }

    fn max(&self) -> i64 {
        self.balance().clamp(1, MAX)
    }

    fn step(&mut self, delta: i64) {
        self.amount = (self.amount + delta).clamp(1, self.max());
    }

    /// The account after this one in the list, when there is more than one.
    fn next_account(&self) -> Option<String> {
        if self.accounts.len() < 2 {
            return None;
        }
        let current = text(&self.account, "account_id");
        let index = self
            .accounts
            .iter()
            .position(|account| text(account, "id") == current)
            .unwrap_or(0);
        let next = &self.accounts[(index + 1) % self.accounts.len()];
        Some(text(next, "id").to_owned())
    }
}

fn account_button(draft: &Draft) -> ItemStack {
    let mut lore = vec![gui::line(
        &format!("Баланс: {}", ar(draft.balance())),
        Color::Gray,
    )];
    if draft.accounts.len() > 1 {
        let current = text(&draft.account, "account_id");
        for account in &draft.accounts {
            let here = text(account, "id") == current;
            let owner = account
                .get("owner")
                .and_then(Value::as_str)
                .map_or_else(String::new, |owner| format!(" · {owner}"));
            let mark = if here { "▶ " } else { "  " };
            lore.push(gui::line(
                &format!("{mark}{}{owner}", text(account, "title")),
                if here { Color::White } else { Color::Gray },
            ));
        }
        lore.push(gui::line(
            "Нажми, чтобы перевести с другого.",
            Color::Yellow,
        ));
    }
    let mut button = gui::button_with(
        &vanilla_items::PAPER,
        &format!(
            "С какого: {} {}",
            title(&draft.account),
            text(&draft.account, "number")
        ),
        lore,
    );
    gui::model(&mut button, "minehouse-bank:card");
    button
}

/// What the window shows for `draft`, by slot.
fn layout(draft: &Draft) -> Vec<(usize, ItemStack)> {
    let mut layout = vec![(ACCOUNT, account_button(draft))];
    for (slot, delta) in STEPS {
        let (item, sign) = if delta < 0 {
            (&vanilla_items::RED_STAINED_GLASS_PANE, "−")
        } else {
            (&vanilla_items::LIME_STAINED_GLASS_PANE, "+")
        };
        let mut button = gui::button(item, &format!("{sign}{}", delta.abs()), &[]);
        button.set_count(delta.abs());
        layout.push((slot, button));
    }
    let mut amount = gui::button(
        &vanilla_items::DEEPSLATE_DIAMOND_ORE,
        &ar(draft.amount),
        &[&format!("На счёте: {}", ar(draft.balance()))],
    );
    amount.set_count(i32::try_from(draft.amount.clamp(1, 64)).unwrap_or(1));
    layout.push((AMOUNT, amount));
    let (recipient, confirm) = match &draft.to {
        None => (
            gui::button(
                &vanilla_items::NAME_TAG,
                "Кому: введи номер счёта",
                &[
                    "Номер вида MH-1234-5678.",
                    "Его видно в банкомате и на сайте.",
                ],
            ),
            gui::button(
                &vanilla_items::GRAY_DYE,
                "Перевести",
                &["Сначала укажи, кому."],
            ),
        ),
        Some((to, owner)) => (
            gui::button(
                &vanilla_items::NAME_TAG,
                &format!("Кому: {to}"),
                &[owner, "Нажми, чтобы сменить."],
            ),
            gui::button(
                &vanilla_items::LIME_DYE,
                &format!("Перевести {}", ar(draft.amount)),
                &[&format!("Получатель: {owner}"), &format!("Счёт {to}")],
            ),
        ),
    };
    layout.push((RECIPIENT, recipient));
    layout.push((CONFIRM, confirm));
    layout.push((CLOSE, gui::button(&vanilla_items::BARRIER, "Закрыть", &[])));
    layout
}

struct State {
    draft: Draft,
    /// A call is in flight.
    busy: bool,
    /// The window is to be closed.
    close: bool,
}

/// The transfer window.
struct Window {
    bank: &'static Arc<Bank>,
    player: Uuid,
    buttons: Shared<SimpleContainer>,
    state: SyncMutex<State>,
}

impl Window {
    fn draw(&self) {
        let layout = layout(&self.state.lock().draft);
        let mut buttons = self.buttons.lock();
        for slot in 0..ROWS {
            buttons.set_item(slot, ItemStack::empty());
        }
        for (slot, item) in layout {
            buttons.set_item(slot, item);
        }
    }

    fn act(self: &Arc<Self>, server: &Server, slot: usize) {
        let Some(player) = online(server, self.player) else {
            return;
        };
        if slot == CLOSE {
            self.state.lock().close = true;
        } else if let Some((_, delta)) = STEPS.iter().find(|(at, _)| *at == slot) {
            gui::click(&player);
            self.state.lock().draft.step(i64::from(*delta));
            self.draw();
        } else if slot == ACCOUNT {
            self.switch_account(&player);
        } else if slot == RECIPIENT {
            let draft = {
                let mut state = self.state.lock();
                if state.busy {
                    return;
                }
                state.close = true;
                state.draft.clone()
            };
            gui::click(&player);
            number_window(self.bank, &player, draft);
        } else if slot == CONFIRM {
            self.confirm(&player);
        }
    }

    fn switch_account(self: &Arc<Self>, player: &Player) {
        let next = {
            let mut state = self.state.lock();
            if state.busy {
                return;
            }
            let Some(next) = state.draft.next_account() else {
                return;
            };
            state.busy = true;
            next
        };
        gui::click(player);
        let window = Arc::clone(self);
        self.bank
            .account(self.player, Some(&next), None, move |server, answer| {
                let account = answer
                    .get("account")
                    .filter(|account| ok(&answer) && account.is_object());
                let mut state = window.state.lock();
                state.busy = false;
                let Some(account) = account else {
                    drop(state);
                    if let Some(player) = online(server, window.player) {
                        gui::error(&player, &gui::error_text(error(&answer)));
                    }
                    return;
                };
                state.draft.account = account.clone();
                state.draft.step(0);
                drop(state);
                window.draw();
            });
    }

    fn confirm(self: &Arc<Self>, player: &Player) {
        let (draft, to, owner) = {
            let mut state = self.state.lock();
            let Some((to, owner)) = state.draft.to.clone() else {
                return;
            };
            if state.busy {
                return;
            }
            if state.draft.balance() < state.draft.amount {
                drop(state);
                gui::error(player, &gui::error_text("no_money"));
                return;
            }
            state.busy = true;
            (state.draft.clone(), to, owner)
        };
        gui::click(player);
        let amount = draft.amount;
        let mut body = body(self.player);
        body["op_id"] = json!(Uuid::new_v4().to_string());
        if let Some(id) = extra_id(&draft.account) {
            body["account_id"] = json!(id);
        }
        body["to_number"] = json!(to);
        body["amount"] = json!(amount);
        let window = Arc::clone(self);
        self.bank.call("transfer", &body, move |server, answer| {
            window.state.lock().busy = false;
            let player = online(server, window.player);
            if !ok(&answer) {
                if let Some(player) = player {
                    gui::error(&player, &gui::error_text(error(&answer)));
                }
                return;
            }
            window.state.lock().close = true;
            let left = answer
                .get("balance")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| draft.balance() - amount);
            log::info!(
                "Bank: {} moved {amount} АР from {} to {to}",
                window.player,
                text(&draft.account, "number")
            );
            let Some(player) = player else {
                return;
            };
            gui::ding(&player);
            gui::info(
                &player,
                &format!(
                    "Переведено {} на счёт {to} ({owner}). На счёте осталось {}.",
                    ar(amount),
                    ar(left)
                ),
            );
            notify_recipient(server, &player, &answer, amount);
        });
    }
}

impl Buttons for Window {
    fn closing(&self) -> bool {
        self.state.lock().close
    }

    fn pos(&self) -> Option<BlockPos> {
        None
    }

    fn bank(&self) -> &Bank {
        self.bank
    }

    fn clicked(self: Arc<Self>, slot: usize) {
        let bank = self.bank;
        bank.later(move |server| self.act(server, slot));
    }
}

/// Tells the owner of the receiving account, if they are online.
fn notify_recipient(server: &Server, from: &Player, answer: &Value, amount: i64) {
    let Some(nick) = answer.get("to_nickname").and_then(Value::as_str) else {
        return;
    };
    let to = server
        .get_players()
        .into_iter()
        .find(|player| player.gameprofile.name().eq_ignore_ascii_case(nick));
    let Some(to) = to.filter(|to| to.gameprofile.id() != from.gameprofile.id()) else {
        return;
    };
    to.send_message(
        &TextComponent::plain(format!(
            "{} перевёл тебе {}.",
            from.gameprofile.name(),
            ar(amount)
        ))
        .color(Color::Green),
    );
    gui::ding(&to);
}

fn window(bank: &'static Arc<Bank>, player: &Arc<Player>, mut draft: Draft) {
    draft.step(0);
    let window = Arc::new(Window {
        bank,
        player: player.gameprofile.id(),
        buttons: SimpleContainer::new(ROWS).into_shared(),
        state: SyncMutex::new(State {
            draft,
            busy: false,
            close: false,
        }),
    });
    window.draw();
    let viewer = Arc::clone(player);
    player.open_menu(gui::title(gui::AMOUNT, "Перевод"), move |context| {
        let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X3, context.container_id);
        builder.section_all_with(ContainerRef::from(&window.buttons), SectionKind::Display);
        builder.player_inventory(&viewer.inventory);
        builder.build(ButtonsKind(window))
    });
}

/// The number of the receiving account, typed in an anvil window.
struct NumberInput {
    bank: &'static Arc<Bank>,
    player: Uuid,
    draft: Draft,
    /// The number typed so far, whether a call is in flight, whether the window is to close.
    state: SyncMutex<(String, bool, bool)>,
}

/// Why a resolved account can't be paid to, or its number and the nickname of its owner.
fn recipient(draft: &Draft, answer: &Value) -> Result<(String, String), String> {
    if !ok(answer) {
        return Err(gui::error_text(error(answer)));
    }
    let to = text(answer, "number");
    if to == text(&draft.account, "number") {
        return Err(gui::error_text("own_account"));
    }
    if text(answer, "account_status") != "active" {
        return Err("Этот счёт заморожен, перевести на него нельзя.".to_owned());
    }
    Ok((to.to_owned(), text(answer, "owner").to_owned()))
}

impl NumberInput {
    fn act(self: &Arc<Self>, server: &Server, slot: usize) {
        let Some(player) = online(server, self.player) else {
            return;
        };
        if slot == NUMBER_BACK {
            gui::click(&player);
            self.state.lock().2 = true;
            window(self.bank, &player, self.draft.clone());
        } else if slot == NUMBER_OK {
            let typed = {
                let mut state = self.state.lock();
                if state.1 {
                    return;
                }
                state.1 = true;
                state.0.trim().to_owned()
            };
            gui::click(&player);
            let input = Arc::clone(self);
            let body = json!({"number": typed});
            self.bank.call("resolve", &body, move |server, answer| {
                input.state.lock().1 = false;
                let Some(player) = online(server, input.player) else {
                    return;
                };
                match recipient(&input.draft, &answer) {
                    Err(why) => gui::error(&player, &why),
                    Ok(to) => {
                        input.state.lock().2 = true;
                        let mut draft = input.draft.clone();
                        draft.to = Some(to);
                        window(input.bank, &player, draft);
                    }
                }
            });
        }
    }
}

impl Buttons for NumberInput {
    fn closing(&self) -> bool {
        self.state.lock().2
    }

    fn pos(&self) -> Option<BlockPos> {
        None
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

fn number_window(bank: &'static Arc<Bank>, player: &Arc<Player>, draft: Draft) {
    let start = draft
        .to
        .as_ref()
        .map_or_else(|| "MH-".to_owned(), |(to, _)| to.clone());
    // The text field of an anvil shows the name of the item in the first slot.
    let mut typed = ItemStack::new(&vanilla_items::PAPER);
    typed.set(CUSTOM_NAME, TextComponent::plain(start.clone()));
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
    let input = Arc::new(NumberInput {
        bank,
        player: player.gameprofile.id(),
        draft,
        state: SyncMutex::new((start, false, false)),
    });
    let buttons = buttons.into_shared();
    let viewer = Arc::clone(player);
    player.open_menu(
        TextComponent::plain("Номер получателя"),
        move |context| {
            let mut builder = MenuBuilder::new(&vanilla_menu_types::ANVIL, context.container_id);
            builder.section_all_with(ContainerRef::from(&buttons), SectionKind::Display);
            builder.player_inventory(&viewer.inventory);
            // The level cost an anvil window shows.
            builder.data_slot(0);
            builder.build(ButtonsKind(input))
        },
    );
}

/// Opens the transfer window on the player's personal account.
pub fn open(player: &Player) {
    let Some(bank) = bank() else {
        gui::error(player, "Банк пока закрыт.");
        return;
    };
    let uuid = player.gameprofile.id();
    bank.account(uuid, None, None, move |server, answer| {
        let Some(player) = online(server, uuid) else {
            return;
        };
        if !ok(&answer) {
            gui::error(&player, &gui::error_text(error(&answer)));
            return;
        }
        let Some(account) = answer.get("account").filter(|account| account.is_object()) else {
            gui::error(&player, &gui::error_text("no_account"));
            return;
        };
        debug_assert!(personal(account) || !text(account, "account_id").is_empty());
        let accounts = answer
            .get("accounts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let draft = Draft {
            account: account.clone(),
            accounts,
            to: None,
            amount: 1,
        };
        window(bank, &player, draft);
    });
}

/// `/transfer`, for every player.
pub(crate) fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "transfer"), tree).default_access(),
    )?;
    Ok(())
}

fn tree() -> CommandNode {
    literal("transfer").executes(|context| run(*context))
}

fn run(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let player = context
        .source()
        .player()
        .ok_or_else(|| CommandError::from("Только для игроков"))?;
    open(player);
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(balance: i64) -> Draft {
        Draft {
            account: json!({
                "account_id": "a-1", "number": "MH-0000-0001", "balance": balance,
                "personal": true,
            }),
            accounts: vec![
                json!({"id": "a-1", "title": "Личный", "personal": true, "owner": "seller"}),
                json!({"id": "a-2", "title": "Клан", "personal": false, "owner": "seller"}),
            ],
            to: None,
            amount: 1,
        }
    }

    fn slots(layout: &[(usize, ItemStack)]) -> Vec<usize> {
        let mut slots: Vec<usize> = layout.iter().map(|(slot, _)| *slot).collect();
        slots.sort_unstable();
        slots
    }

    #[test]
    fn the_amount_stays_between_one_and_the_balance() {
        let mut draft = draft(70);
        draft.step(64);
        assert_eq!(draft.amount, 65);
        draft.step(64);
        assert_eq!(draft.amount, 70);
        draft.step(-640);
        assert_eq!(draft.amount, 1);
        // An empty account still shows one, the transfer itself is refused.
        let mut empty = self::draft(0);
        empty.step(8);
        assert_eq!(empty.amount, 1);
        let mut rich = self::draft(5_000_000);
        rich.amount = 999_990;
        rich.step(64);
        assert_eq!(rich.amount, MAX);
    }

    #[test]
    fn the_account_button_cycles_through_the_accounts() {
        let mut draft = draft(10);
        assert_eq!(draft.next_account().as_deref(), Some("a-2"));
        draft.account["account_id"] = json!("a-2");
        assert_eq!(draft.next_account().as_deref(), Some("a-1"));
        draft.accounts.truncate(1);
        assert_eq!(draft.next_account(), None);
    }

    #[test]
    fn the_window_has_the_same_buttons_with_and_without_a_recipient() {
        let mut draft = draft(10);
        let expected = vec![4, 9, 10, 11, 13, 15, 16, 17, 18, 22, 26];
        assert_eq!(slots(&layout(&draft)), expected);
        draft.to = Some(("MH-0000-0002".to_owned(), "buyer".to_owned()));
        assert_eq!(slots(&layout(&draft)), expected);
    }

    #[test]
    fn a_resolved_account_is_checked_before_it_is_offered() {
        let draft = draft(10);
        let other = json!({
            "ok": true, "number": "MH-0000-0002", "owner": "buyer", "account_status": "active",
        });
        assert_eq!(
            recipient(&draft, &other),
            Ok(("MH-0000-0002".to_owned(), "buyer".to_owned()))
        );
        let own = json!({
            "ok": true, "number": "MH-0000-0001", "owner": "seller", "account_status": "active",
        });
        assert_eq!(recipient(&draft, &own), Err(gui::error_text("own_account")));
        let frozen = json!({
            "ok": true, "number": "MH-0000-0002", "owner": "buyer", "account_status": "frozen",
        });
        assert!(recipient(&draft, &frozen).is_err());
        let missing = json!({"ok": false, "error": "no_recipient"});
        assert_eq!(
            recipient(&draft, &missing),
            Err(gui::error_text("no_recipient"))
        );
    }
}
