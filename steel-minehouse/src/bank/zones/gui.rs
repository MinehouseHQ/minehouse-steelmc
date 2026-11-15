//! The «Зоны спавна» windows (the Fabric `ZoneGui`): every zone with its status, a zone card
//! with «Сделать ставку» and «Продлить», and the bid amount picker.
//!
//! Bids come from the player's personal account; other accounts can bid on the site. A bid is
//! taken only from a player standing in the zone. An open window asks the site again every
//! three seconds, so bids by others show up live.

use std::sync::{Arc, Weak};

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use steel_core::{
    inventory::{
        container::{Container as _, SimpleContainer},
        lock::ContainerRef,
        menu::{MenuBuilder, SectionKind},
    },
    player::Player,
    server::Server,
};
use steel_registry::{
    item_stack::ItemStack, items::ItemRef, menu_type::MenuTypeRef, vanilla_items,
    vanilla_menu_types,
};
use steel_utils::{
    BlockPos,
    locks::{IntoShared as _, Shared, SyncMutex},
};
use text_components::{Modifier as _, TextComponent, format::Color};
use uuid::Uuid;

use super::{Live, Zone, left, parse, standing_in};
use crate::bank::{
    Bank, ar,
    atm::{AMOUNT_BACK, AMOUNT_OK, AMOUNT_SHOWN, Buttons, ButtonsKind, ROWS, STEPS},
    bank, body, error, gui, ok, online,
};

/// The slots of the list that hold zones; the bottom row is buttons.
const LIST_ZONES: usize = 45;
const LIST_SIZE: usize = 54;
const LIST_HELP: usize = 49;
const LIST_CLOSE: usize = 53;

const CARD: usize = 4;
const BID: usize = 13;
const RENEW: usize = 15;
const BACK: usize = 18;
const CLOSE: usize = 26;
const BALANCE: usize = 8;

/// The most one bid may be.
const MAX_BID: i64 = 1_000_000;

/// What an error code of the zones means, in Russian.
#[must_use]
pub fn error_text(code: &str) -> String {
    let text = match code {
        "no_auction" | "auction_closed" => "Аукцион по этой зоне уже закончился.",
        "not_started" => "Аукцион ещё не начался, ставки пока не принимаются.",
        "not_in_zone" => {
            "Ставку можно сделать, только стоя в этой зоне. Вернись в неё и попробуй снова."
        }
        "low_bid" => "Ставка меньше минимальной.",
        "already_top" => "Твоя ставка и так самая высокая.",
        "has_zone" => "У тебя уже есть зона: одна зона на игрока.",
        "other_bid" => "Твоя ставка лидирует на другом аукционе: одна зона на игрока.",
        "not_owner" => "Это не твоя зона.",
        "too_early" => "Продлить можно только за 2 дня до конца аренды.",
        "no_zone" => "Такой зоны нет.",
        "leased" => "Зону сейчас арендуют.",
        "auction_open" => "По зоне уже идёт аукцион.",
        "common_zone" => "Общую зону нельзя выставить на аукцион.",
        "bad_input" => "Неверные данные.",
        _ => return gui::error_text(code),
    };
    text.to_owned()
}

fn icon(zone: &Zone, now: DateTime<Utc>) -> ItemRef {
    if zone.common {
        return &vanilla_items::WHITE_STAINED_GLASS;
    }
    match zone.status.as_str() {
        "auction" if zone.auction_soon(now) => &vanilla_items::ORANGE_STAINED_GLASS,
        "auction" => &vanilla_items::YELLOW_STAINED_GLASS,
        "leased" if zone.mine => &vanilla_items::CYAN_STAINED_GLASS,
        "leased" => &vanilla_items::RED_STAINED_GLASS,
        _ => &vanilla_items::LIME_STAINED_GLASS,
    }
}

fn button(item: ItemRef, name: &str, lore: &[String]) -> ItemStack {
    let lore: Vec<&str> = lore.iter().map(String::as_str).collect();
    gui::button(item, name, &lore)
}

fn card(zone: &Zone, now: DateTime<Utc>) -> ItemStack {
    button(icon(zone, now), &zone.name(), &zone.describe(now))
}

/// Asks the site for the zones as `player` sees them (their own zone is marked).
fn fetch(
    bank: &'static Arc<Bank>,
    player: Uuid,
    then: impl FnOnce(&Arc<Server>, Result<Vec<Zone>, String>) + Send + 'static,
) {
    bank.call("zones", &body(player), move |server, answer| {
        let zones = if ok(&answer) {
            Ok(parse(&answer))
        } else {
            Err(error_text(error(&answer)))
        };
        then(server, zones);
    });
}

fn find(zones: &[Zone], old: &Zone) -> Zone {
    zones
        .iter()
        .find(|zone| zone.id == old.id)
        .unwrap_or(old)
        .clone()
}

fn show(
    player: &Arc<Player>,
    title: TextComponent,
    kind: MenuTypeRef,
    buttons: &Shared<SimpleContainer>,
    window: Arc<dyn Buttons>,
) {
    let buttons = ContainerRef::from(buttons);
    let viewer = Arc::clone(player);
    player.open_menu(title, move |context| {
        let mut builder = MenuBuilder::new(kind, context.container_id);
        builder.section_all_with(buttons, SectionKind::Display);
        builder.player_inventory(&viewer.inventory);
        builder.build(ButtonsKind(window))
    });
}

fn fill(buttons: &Shared<SimpleContainer>, size: usize, layout: Vec<(usize, ItemStack)>) {
    let mut buttons = buttons.lock();
    for slot in 0..size {
        buttons.set_item(slot, ItemStack::empty());
    }
    for (slot, item) in layout {
        buttons.set_item(slot, item);
    }
}

// ---- the list --------------------------------------------------------------------------------

fn list_layout(zones: &[Zone], now: DateTime<Utc>) -> Vec<(usize, ItemStack)> {
    let mut layout: Vec<(usize, ItemStack)> = zones
        .iter()
        .take(LIST_ZONES)
        .enumerate()
        .map(|(slot, zone)| {
            let mut lore = zone.describe(now);
            if zone.mine {
                lore.insert(0, "Это твоя зона".to_owned());
            }
            if !zone.common {
                lore.push("Нажми, чтобы открыть".to_owned());
            }
            let mut item = button(icon(zone, now), &zone.name(), &lore);
            item.set_count(zone.num.clamp(1, 64));
            (slot, item)
        })
        .collect();
    layout.push((
        LIST_HELP,
        gui::button(
            &vanilla_items::BOOK,
            "Как это работает",
            &[
                "Зона уходит с аукциона в аренду на 7 дней.",
                "Ставка сразу списывается со счёта; если её",
                "перебьют, АР вернутся. Последние 10 минут",
                "каждая ставка продлевает аукцион.",
                "Продлить аренду можно за 2 дня до конца",
                "по цене зоны, иначе зона снова",
                "уйдёт на аукцион.",
                "Одна зона на игрока.",
            ],
        ),
    ));
    layout.push((
        LIST_CLOSE,
        gui::button(&vanilla_items::BARRIER, "Закрыть", &[]),
    ));
    layout
}

struct ListWindow {
    bank: &'static Arc<Bank>,
    player: Uuid,
    buttons: Shared<SimpleContainer>,
    /// The zones shown and whether the window is closed.
    state: SyncMutex<(Vec<Zone>, bool)>,
}

impl ListWindow {
    fn act(&self, server: &Server, slot: usize) {
        let Some(player) = online(server, self.player) else {
            return;
        };
        if slot == LIST_CLOSE {
            self.state.lock().1 = true;
            return;
        }
        let (zone, all) = {
            let state = self.state.lock();
            let zone = state.0.get(slot).filter(|_| slot < LIST_ZONES).cloned();
            (zone, state.0.clone())
        };
        if let Some(zone) = zone.filter(|zone| !zone.common) {
            gui::click(&player);
            self.state.lock().1 = true;
            zone_window(self.bank, &player, zone, all);
        }
    }
}

impl Buttons for ListWindow {
    fn closing(&self) -> bool {
        self.state.lock().1
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

    fn removed(&self) {
        self.state.lock().1 = true;
    }
}

impl Live for ListWindow {
    fn closed(&self) -> bool {
        self.state.lock().1
    }

    fn refresh(self: Arc<Self>) {
        fetch(self.bank, self.player, move |_, zones| {
            let Ok(zones) = zones else {
                return;
            };
            fill(&self.buttons, LIST_SIZE, list_layout(&zones, Utc::now()));
            self.state.lock().0 = zones;
        });
    }
}

fn list_window(bank: &'static Arc<Bank>, player: &Arc<Player>, zones: Vec<Zone>) {
    let buttons = SimpleContainer::new(LIST_SIZE).into_shared();
    fill(&buttons, LIST_SIZE, list_layout(&zones, Utc::now()));
    let window = Arc::new(ListWindow {
        bank,
        player: player.gameprofile.id(),
        buttons,
        state: SyncMutex::new((zones, false)),
    });
    let live: Weak<dyn Live> = Arc::downgrade(&window) as Weak<dyn Live>;
    bank.zones.watch(live);
    show(
        player,
        TextComponent::plain("Зоны спавна"),
        &vanilla_menu_types::GENERIC_9X6,
        &Arc::clone(&window.buttons),
        window,
    );
}

/// `/zones`: a fresh list for this player, then the window.
pub fn open(player: &Player) {
    let Some(bank) = bank() else {
        gui::error(player, "Банк пока закрыт.");
        return;
    };
    if !bank.zones_open() && !super::commands::is_admin(player) {
        gui::error(player, "Зоны спавна пока закрыты.");
        return;
    }
    let uuid = player.gameprofile.id();
    fetch(bank, uuid, move |server, zones| {
        let Some(player) = online(server, uuid) else {
            return;
        };
        match zones {
            Ok(zones) => list_window(bank, &player, zones),
            Err(why) => gui::error(&player, &why),
        }
    });
}

// ---- one zone --------------------------------------------------------------------------------

/// Whether a lease that ends at `until` may be renewed already: two days before its end.
fn renewable(until: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    until.is_some_and(|until| until < now + Duration::days(2))
}

fn zone_layout(zone: &Zone, standing: bool, now: DateTime<Utc>) -> Vec<(usize, ItemStack)> {
    let mut layout = vec![(CARD, card(zone, now))];
    if let Some(auction) = zone.auction.as_ref().filter(|_| zone.status == "auction") {
        let item = if !auction.started(now) {
            gui::button(
                &vanilla_items::CLOCK,
                &format!("Аукцион начнётся через {}", left(auction.starts_at, now)),
                &[
                    "Ставки откроются в момент старта.",
                    &format!("Первая ставка от {}.", ar(auction.start_price)),
                ],
            )
        } else if standing {
            gui::button(
                &vanilla_items::LIME_DYE,
                "Сделать ставку",
                &[
                    &format!("От {}, со своего счёта.", ar(auction.min_bid)),
                    &format!(
                        "Ставок: {}, осталось {}",
                        auction.bids,
                        left(auction.ends_at, now)
                    ),
                ],
            )
        } else {
            gui::button(
                &vanilla_items::GRAY_DYE,
                "Ставка только из зоны",
                &[
                    "Ставку принимают, только когда стоишь",
                    "в этой зоне. Дойди до неё и открой /zones bid.",
                    &format!(
                        "Сейчас от {}, осталось {}",
                        ar(auction.min_bid),
                        left(auction.ends_at, now)
                    ),
                ],
            )
        };
        layout.push((BID, item));
    }
    if zone.mine && zone.leased() {
        let open = renewable(zone.lease_until, now);
        let price = zone.renew_price.map_or_else(|| "?".to_owned(), ar);
        let (item, when): (ItemRef, &str) = if open {
            (&vanilla_items::LIME_DYE, "Спишется с личного счёта.")
        } else {
            (&vanilla_items::GRAY_DYE, "Можно за 2 дня до конца аренды.")
        };
        layout.push((
            RENEW,
            gui::button(
                item,
                &format!("Продлить на 7 дней за {price}"),
                &[when, "7 дней прибавятся к текущему сроку."],
            ),
        ));
    }
    layout.push((BACK, gui::button(&vanilla_items::ARROW, "Назад", &[])));
    layout.push((CLOSE, gui::button(&vanilla_items::BARRIER, "Закрыть", &[])));
    layout
}

struct ZoneState {
    zone: Zone,
    all: Vec<Zone>,
    busy: bool,
    closed: bool,
}

struct ZoneWindow {
    bank: &'static Arc<Bank>,
    player: Uuid,
    buttons: Shared<SimpleContainer>,
    state: SyncMutex<ZoneState>,
}

impl ZoneWindow {
    fn draw(&self, player: &Player) {
        let zone = self.state.lock().zone.clone();
        let standing = standing_in(&self.bank.zones.all(), player)
            .is_some_and(|standing| standing.id == zone.id);
        fill(
            &self.buttons,
            ROWS,
            zone_layout(&zone, standing, Utc::now()),
        );
    }

    fn act(self: &Arc<Self>, server: &Server, slot: usize) {
        let Some(player) = online(server, self.player) else {
            return;
        };
        let (zone, all) = {
            let state = self.state.lock();
            (state.zone.clone(), state.all.clone())
        };
        let now = Utc::now();
        if slot == CLOSE {
            self.state.lock().closed = true;
        } else if slot == BACK {
            self.state.lock().closed = true;
            list_window(self.bank, &player, all);
        } else if slot == BID && zone.auction_open(now) {
            let standing = standing_in(&self.bank.zones.all(), &player)
                .is_some_and(|standing| standing.id == zone.id);
            if standing {
                gui::click(&player);
                self.state.lock().closed = true;
                bid_window(self.bank, &player, zone, all);
            }
        } else if slot == RENEW && zone.mine && zone.leased() && renewable(zone.lease_until, now) {
            self.renew(&player, &zone);
        }
    }

    fn renew(self: &Arc<Self>, player: &Player, zone: &Zone) {
        {
            let mut state = self.state.lock();
            if state.busy {
                return;
            }
            state.busy = true;
        }
        gui::click(player);
        let mut body = body(self.player);
        body["op_id"] = json!(Uuid::new_v4().to_string());
        body["zone_id"] = json!(zone.id);
        let name = zone.name();
        let window = Arc::clone(self);
        self.bank.call("zone_renew", &body, move |server, answer| {
            window.state.lock().busy = false;
            let player = online(server, window.player);
            if !ok(&answer) {
                if let Some(player) = player {
                    gui::error(&player, &error_text(error(&answer)));
                }
                return;
            }
            window.state.lock().closed = true;
            window.bank.refresh_zones(Some(Box::new(|_, _| {})));
            let Some(player) = player else {
                return;
            };
            let paid = answer.get("paid").and_then(Value::as_i64).unwrap_or(0);
            gui::ding(&player);
            gui::info(
                &player,
                &format!("{name} продлена на 7 дней, списано {}.", ar(paid)),
            );
        });
    }
}

impl Buttons for ZoneWindow {
    fn closing(&self) -> bool {
        self.state.lock().closed
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

    fn removed(&self) {
        self.state.lock().closed = true;
    }
}

impl Live for ZoneWindow {
    fn closed(&self) -> bool {
        self.state.lock().closed
    }

    fn refresh(self: Arc<Self>) {
        fetch(self.bank, self.player, move |server, zones| {
            let (Ok(zones), Some(player)) = (zones, online(server, self.player)) else {
                return;
            };
            {
                let mut state = self.state.lock();
                state.zone = find(&zones, &state.zone);
                state.all = zones;
            }
            self.draw(&player);
        });
    }
}

fn zone_window(bank: &'static Arc<Bank>, player: &Arc<Player>, zone: Zone, all: Vec<Zone>) {
    let title = TextComponent::plain(zone.name());
    let window = Arc::new(ZoneWindow {
        bank,
        player: player.gameprofile.id(),
        buttons: SimpleContainer::new(ROWS).into_shared(),
        state: SyncMutex::new(ZoneState {
            zone,
            all,
            busy: false,
            closed: false,
        }),
    });
    window.draw(player);
    let live: Weak<dyn Live> = Arc::downgrade(&window) as Weak<dyn Live>;
    bank.zones.watch(live);
    show(
        player,
        title,
        &vanilla_menu_types::GENERIC_9X3,
        &Arc::clone(&window.buttons),
        window,
    );
}

// ---- a bid -----------------------------------------------------------------------------------

/// The least a bid on `zone` may be now.
fn min_bid(zone: &Zone) -> i64 {
    zone.auction
        .as_ref()
        .map_or(zone.start_price, |auction| auction.min_bid)
}

fn bid_layout(
    zone: &Zone,
    amount: i64,
    balance: Option<i64>,
    now: DateTime<Utc>,
) -> Vec<(usize, ItemStack)> {
    let mut layout = vec![(CARD, card(zone, now))];
    for (slot, delta) in STEPS {
        let (item, sign) = if delta < 0 {
            (&vanilla_items::RED_STAINED_GLASS_PANE, "−")
        } else {
            (&vanilla_items::LIME_STAINED_GLASS_PANE, "+")
        };
        let mut step = gui::button(item, &format!("{sign}{}", delta.abs()), &[]);
        step.set_count(delta.abs());
        layout.push((slot, step));
    }
    let mut shown = gui::button(
        &vanilla_items::DEEPSLATE_DIAMOND_ORE,
        &ar(amount),
        &[&format!("Минимум {}", ar(min_bid(zone)))],
    );
    shown.set_count(i32::try_from(amount.clamp(1, 64)).unwrap_or(1));
    layout.push((AMOUNT_SHOWN, shown));
    let on_account = balance.map_or_else(
        || "На счёте: …".to_owned(),
        |balance| format!("На счёте: {}", ar(balance)),
    );
    let lacking = if balance.is_some_and(|balance| balance < amount) {
        " — не хватает"
    } else {
        ""
    };
    layout.push((
        AMOUNT_OK,
        gui::button(
            &vanilla_items::LIME_DYE,
            &format!("Поставить {}", ar(amount)),
            &[
                "АР спишутся с личного счёта.",
                "Если ставку перебьют, они вернутся.",
                &format!("{on_account}{lacking}"),
            ],
        ),
    ));
    layout.push((
        BALANCE,
        gui::button(&vanilla_items::GOLD_NUGGET, &on_account, &["Личный счёт"]),
    ));
    layout.push((
        AMOUNT_BACK,
        gui::button(&vanilla_items::ARROW, "Назад", &[]),
    ));
    layout.push((CLOSE, gui::button(&vanilla_items::BARRIER, "Закрыть", &[])));
    layout
}

struct BidState {
    zone: Zone,
    all: Vec<Zone>,
    amount: i64,
    /// The balance of the personal account, once the site has told it.
    balance: Option<i64>,
    busy: bool,
    closed: bool,
}

struct BidWindow {
    bank: &'static Arc<Bank>,
    player: Uuid,
    buttons: Shared<SimpleContainer>,
    state: SyncMutex<BidState>,
}

/// What a refused bid tells the player.
fn refusal(answer: &Value) -> String {
    let code = error(answer);
    let number = |key: &str| answer.get(key).and_then(Value::as_i64);
    match (code, number("min"), number("balance")) {
        ("low_bid", Some(min), _) => {
            format!("Ставку уже подняли: теперь минимум {}.", ar(min))
        }
        ("no_money", _, Some(balance)) => {
            format!("На счёте не хватает АР: там {}.", ar(balance))
        }
        _ => error_text(code),
    }
}

impl BidWindow {
    fn draw(&self) {
        let layout = {
            let state = self.state.lock();
            bid_layout(&state.zone, state.amount, state.balance, Utc::now())
        };
        fill(&self.buttons, ROWS, layout);
    }

    fn load_balance(self: &Arc<Self>) {
        let window = Arc::clone(self);
        self.bank
            .account(self.player, None, None, move |_, answer| {
                let balance = answer
                    .get("account")
                    .filter(|_| ok(&answer))
                    .and_then(|account| account.get("balance"))
                    .and_then(Value::as_i64);
                if let Some(balance) = balance {
                    window.state.lock().balance = Some(balance);
                    window.draw();
                }
            });
    }

    fn act(self: &Arc<Self>, server: &Server, slot: usize) {
        let Some(player) = online(server, self.player) else {
            return;
        };
        if slot == CLOSE {
            self.state.lock().closed = true;
        } else if slot == AMOUNT_BACK {
            let (zone, all) = {
                let mut state = self.state.lock();
                state.closed = true;
                (state.zone.clone(), state.all.clone())
            };
            zone_window(self.bank, &player, zone, all);
        } else if let Some((_, delta)) = STEPS.iter().find(|(at, _)| *at == slot) {
            gui::click(&player);
            {
                let mut state = self.state.lock();
                let min = min_bid(&state.zone);
                state.amount = (state.amount + i64::from(*delta)).min(MAX_BID).max(min);
            }
            self.draw();
        } else if slot == AMOUNT_OK {
            self.confirm(&player);
        }
    }

    fn confirm(self: &Arc<Self>, player: &Player) {
        let (zone, amount) = {
            let mut state = self.state.lock();
            if state.busy {
                return;
            }
            state.busy = true;
            (state.zone.clone(), state.amount)
        };
        gui::click(player);
        let now = Utc::now();
        let standing = standing_in(&self.bank.zones.all(), player)
            .is_some_and(|standing| standing.id == zone.id);
        let refused = match &zone.auction {
            None => Some(error_text("auction_closed")),
            Some(auction) if !auction.started(now) => Some(format!(
                "Аукцион начнётся через {}.",
                left(auction.starts_at, now)
            )),
            Some(_) if !standing => Some(error_text("not_in_zone")),
            Some(_) => None,
        };
        let auction = match (refused, &zone.auction) {
            (None, Some(auction)) => auction.id.clone(),
            (refused, _) => {
                self.state.lock().busy = false;
                gui::error(player, &refused.unwrap_or_default());
                return;
            }
        };
        let mut body = body(self.player);
        body["op_id"] = json!(Uuid::new_v4().to_string());
        body["auction_id"] = json!(auction);
        body["amount"] = json!(amount);
        let name = zone.name();
        let window = Arc::clone(self);
        self.bank.call("zone_bid", &body, move |server, answer| {
            window.state.lock().busy = false;
            let player = online(server, window.player);
            if !ok(&answer) {
                if let Some(player) = player {
                    gui::error(&player, &refusal(&answer));
                }
                return;
            }
            window.state.lock().closed = true;
            window.bank.refresh_zones(Some(Box::new(|_, _| {})));
            let Some(player) = player else {
                return;
            };
            gui::ding(&player);
            gui::info(&player, &accepted(&name, amount, &answer, Utc::now()));
            outbid(server, &player, &name, amount, &answer);
        });
    }
}

/// What an accepted bid tells the bidder.
fn accepted(name: &str, amount: i64, answer: &Value, now: DateTime<Utc>) -> String {
    let ends = answer
        .get("ends_at")
        .and_then(Value::as_str)
        .and_then(|ends| DateTime::parse_from_rfc3339(ends).ok())
        .map_or_default(|ends| format!(" Осталось {}.", left(ends.with_timezone(&Utc), now)));
    let balance = answer
        .get("balance")
        .and_then(Value::as_i64)
        .map_or_default(|balance| format!(" На счёте {}.", ar(balance)));
    format!("Ставка {} на {name} принята.{ends}{balance}", ar(amount))
}

/// Tells the player whose bid was just beaten, if they are online.
fn outbid(server: &Server, by: &Player, name: &str, amount: i64, answer: &Value) {
    let Some(beaten) = answer.get("outbid").filter(|outbid| outbid.is_object()) else {
        return;
    };
    let Some(nick) = beaten.get("nickname").and_then(Value::as_str) else {
        return;
    };
    let other = server.get_players().into_iter().find(|player| {
        player.gameprofile.name().eq_ignore_ascii_case(nick)
            && player.gameprofile.id() != by.gameprofile.id()
    });
    let Some(other) = other else {
        return;
    };
    let back = beaten.get("amount").and_then(Value::as_i64).unwrap_or(0);
    other.send_message(
        &TextComponent::plain(format!(
            "Твою ставку на {name} перебил {} ({}). {} вернулись на счёт. /zones",
            by.gameprofile.name(),
            ar(amount),
            ar(back)
        ))
        .color(Color::Gold),
    );
    gui::ding(&other);
}

impl Buttons for BidWindow {
    fn closing(&self) -> bool {
        self.state.lock().closed
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

    fn removed(&self) {
        self.state.lock().closed = true;
    }
}

impl Live for BidWindow {
    fn closed(&self) -> bool {
        self.state.lock().closed
    }

    fn refresh(self: Arc<Self>) {
        fetch(self.bank, self.player, move |_, zones| {
            let Ok(zones) = zones else {
                return;
            };
            {
                let mut state = self.state.lock();
                state.zone = find(&zones, &state.zone);
                state.all = zones;
                // Someone outbid us: the amount follows the new minimum.
                state.amount = state.amount.max(min_bid(&state.zone));
            }
            self.draw();
            self.load_balance();
        });
    }
}

/// The bid window for `zone` (`/zones bid` and the zone card).
pub(super) fn bid_window(
    bank: &'static Arc<Bank>,
    player: &Arc<Player>,
    zone: Zone,
    all: Vec<Zone>,
) {
    let title = gui::title(gui::AMOUNT, &format!("Ставка: {}", zone.name()));
    let amount = min_bid(&zone);
    let window = Arc::new(BidWindow {
        bank,
        player: player.gameprofile.id(),
        buttons: SimpleContainer::new(ROWS).into_shared(),
        state: SyncMutex::new(BidState {
            zone,
            all,
            amount,
            balance: None,
            busy: false,
            closed: false,
        }),
    });
    window.draw();
    window.load_balance();
    let live: Weak<dyn Live> = Arc::downgrade(&window) as Weak<dyn Live>;
    bank.zones.watch(live);
    show(
        player,
        title,
        &vanilla_menu_types::GENERIC_9X3,
        &Arc::clone(&window.buttons),
        window,
    );
}

/// On join: a tenant whose lease ends within two days is told so every time, until they renew.
pub(crate) fn remind(bank: &'static Arc<Bank>, player: Uuid) {
    if !bank.zones_open() {
        return;
    }
    fetch(bank, player, move |server, zones| {
        let (Ok(zones), Some(player)) = (zones, online(server, player)) else {
            return;
        };
        for line in reminders(&zones, Utc::now()) {
            player.send_message(&TextComponent::plain(line).color(Color::Gold));
        }
    });
}

fn reminders(zones: &[Zone], now: DateTime<Utc>) -> Vec<String> {
    zones
        .iter()
        .filter(|zone| zone.mine && zone.leased() && renewable(zone.lease_until, now))
        .filter_map(|zone| {
            let until = zone.lease_until?;
            let price = zone
                .renew_price
                .map_or_else(String::new, |price| format!(" за {}", ar(price)));
            Some(format!(
                "Аренда зоны «{}» заканчивается через {}. Продли её на 7 дней{price} в /zones, \
                 иначе зона уйдёт на аукцион.",
                zone.name(),
                left(until, now)
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::slice;

    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-15T12:00:00Z")
            .map_or_default(|time| time.with_timezone(&Utc))
    }

    fn zone(status: &str) -> Zone {
        let listed = json!({
            "id": "z-3", "num": 3, "kind": "lot", "world": "minecraft:overworld",
            "points": [[0, 0], [10, 0], [10, 10], [0, 10]], "y_min": 60, "y_max": 100,
            "start_price": 5, "status": status, "rent_price": 7,
            "auction": {
                "id": "a-1", "starts_at": "2026-11-15T11:00:00Z",
                "ends_at": "2026-11-15T13:30:00Z", "start_price": 5, "top": 12,
                "top_nickname": "buyer", "min_bid": 13, "bids": 4,
            },
        });
        Zone::parse(&listed).unwrap_or_else(|| unreachable!())
    }

    fn slots(layout: &[(usize, ItemStack)]) -> Vec<usize> {
        let mut slots: Vec<usize> = layout.iter().map(|(slot, _)| *slot).collect();
        slots.sort_unstable();
        slots
    }

    #[test]
    fn the_zone_card_offers_what_the_player_can_do() {
        let auction = zone("auction");
        assert_eq!(slots(&zone_layout(&auction, true, now())), [4, 13, 18, 26]);
        let free = Zone {
            auction: None,
            ..zone("free")
        };
        assert_eq!(slots(&zone_layout(&free, true, now())), [4, 18, 26]);
        let mine = Zone {
            auction: None,
            mine: true,
            lease_until: Some(now() + Duration::days(1)),
            ..zone("leased")
        };
        assert_eq!(slots(&zone_layout(&mine, true, now())), [4, 15, 18, 26]);
        let theirs = Zone {
            mine: false,
            ..mine
        };
        assert_eq!(slots(&zone_layout(&theirs, true, now())), [4, 18, 26]);
    }

    #[test]
    fn a_lease_is_renewed_in_its_last_two_days() {
        assert!(!renewable(None, now()));
        assert!(!renewable(Some(now() + Duration::days(3)), now()));
        assert!(renewable(Some(now() + Duration::hours(47)), now()));
        let mine = Zone {
            auction: None,
            mine: true,
            lease_until: Some(now() + Duration::hours(30)),
            ..zone("leased")
        };
        assert_eq!(
            reminders(slice::from_ref(&mine), now()),
            [
                "Аренда зоны «Зона 3» заканчивается через 1 д 6 ч. Продли её на 7 дней за 7 АР в \
              /zones, иначе зона уйдёт на аукцион."
            ]
        );
        let later = Zone {
            lease_until: Some(now() + Duration::days(5)),
            ..mine
        };
        assert_eq!(reminders(&[later], now()), [] as [String; 0]);
    }

    #[test]
    fn the_bid_window_starts_from_the_least_bid() {
        let auction = zone("auction");
        assert_eq!(min_bid(&auction), 13);
        let layout = bid_layout(&auction, 13, Some(10), now());
        assert_eq!(
            slots(&layout),
            [4, 8, 9, 10, 11, 13, 15, 16, 17, 18, 22, 26]
        );
    }

    #[test]
    fn a_refused_bid_says_what_changed() {
        let low = json!({"ok": false, "error": "low_bid", "min": 20});
        assert_eq!(refusal(&low), "Ставку уже подняли: теперь минимум 20 АР.");
        let poor = json!({"ok": false, "error": "no_money", "balance": 3});
        assert_eq!(refusal(&poor), "На счёте не хватает АР: там 3 АР.");
        let top = json!({"ok": false, "error": "already_top"});
        assert_eq!(refusal(&top), "Твоя ставка и так самая высокая.");
    }

    #[test]
    fn an_accepted_bid_says_how_long_is_left() {
        let answer = json!({"ok": true, "ends_at": "2026-11-15T12:10:00Z", "balance": 80});
        assert_eq!(
            accepted("Зона 3", 20, &answer, now()),
            "Ставка 20 АР на Зона 3 принята. Осталось 10 мин 0 сек. На счёте 80 АР."
        );
    }
}
