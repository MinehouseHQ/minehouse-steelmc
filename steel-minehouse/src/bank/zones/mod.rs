//! Spawn zones (the Fabric `zones` package): a shared circle in the middle of the spawn island
//! and lots around it that go to auction and are rented for 7 days for АР.
//!
//! There is no claim protection: a zone only says whose land it is. The site keeps everything.
//! The server asks for the list every 30 s (`zones`), shows the zone a player is in on the
//! action bar, tells the site who stands in which lot (a bid is taken only from inside the
//! zone) and announces auctions in chat.

mod bossbars;
pub mod commands;
pub mod gui;
pub mod markup;
mod zone;

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use steel_core::{entity::Entity as _, player::Player, server::Server};
use steel_utils::locks::SyncMutex;
use text_components::{
    Modifier as _, TextComponent,
    format::Color,
    interactivity::{ClickEvent, HoverEvent},
};
use uuid::Uuid;

pub use self::zone::{Auction, Zone, left};
use super::{Bank, ok};

/// How often the list is asked for again.
const REFRESH_TICKS: u64 = 600;
/// How often the zone under each player is looked up.
const HUD_TICKS: u64 = 10;
/// The site trusts a player's place for a minute: the list also goes out this often unchanged.
const PRESENCE_TICKS: u64 = 400;
/// How often the boss bars of the auctions are redrawn.
const BAR_TICKS: u64 = 20;
/// How often an open window asks the site again, so bids by others show up live.
const LIVE_TICKS: u64 = 60;

/// What runs on the server tick with a fresh `zones` answer.
pub type Listed = Box<dyn FnOnce(&Arc<Server>, &Value) + Send>;

/// A window that follows the list while it is open.
pub(super) trait Live: Send + Sync {
    /// The window is gone and needs no more news.
    fn closed(&self) -> bool;
    /// Asks the site again and redraws.
    fn refresh(self: Arc<Self>);
}

#[derive(Default)]
struct Presence {
    /// The lot each online player stands in.
    zones: BTreeMap<Uuid, String>,
    dirty: bool,
    sending: bool,
}

/// What the server knows of the zones.
pub struct Zones {
    list: SyncMutex<Arc<Vec<Zone>>>,
    /// What the action bar last told each player.
    current: SyncMutex<BTreeMap<Uuid, String>>,
    presence: SyncMutex<Presence>,
    loading: AtomicBool,
    live: SyncMutex<Vec<Weak<dyn Live>>>,
    /// The boss bars of the auctions.
    pub(super) bars: bossbars::BossBars,
    /// The drafts of the admins and the borders players asked to see.
    pub markup: markup::Markup,
}

impl Zones {
    /// No zones yet: the first answer of the site fills the list.
    #[must_use]
    pub fn new() -> Self {
        Self {
            list: SyncMutex::new(Arc::new(Vec::new())),
            current: SyncMutex::new(BTreeMap::new()),
            presence: SyncMutex::new(Presence {
                dirty: true,
                ..Presence::default()
            }),
            loading: AtomicBool::new(false),
            live: SyncMutex::new(Vec::new()),
            bars: bossbars::BossBars::load(),
            markup: markup::Markup::default(),
        }
    }

    /// Every zone, as the site last listed them.
    #[must_use]
    pub fn all(&self) -> Arc<Vec<Zone>> {
        Arc::clone(&self.list.lock())
    }

    /// The zone with this number.
    #[must_use]
    pub fn by_num(&self, num: i32) -> Option<Zone> {
        self.all().iter().find(|zone| zone.num == num).cloned()
    }

    pub(super) fn watch(&self, window: Weak<dyn Live>) {
        self.live.lock().push(window);
    }

    fn forget(&self, player: Uuid) {
        self.current.lock().remove(&player);
        self.markup.forget(player);
        let mut presence = self.presence.lock();
        if presence.zones.remove(&player).is_some() {
            presence.dirty = true;
        }
    }
}

impl Default for Zones {
    fn default() -> Self {
        Self::new()
    }
}

/// The zone at a position; lots win over the common circle where they overlap.
#[must_use]
pub fn at<'a>(zones: &'a [Zone], world: &str, x: f64, y: f64, z: f64) -> Option<&'a Zone> {
    let mut common = None;
    for zone in zones {
        if !zone.contains(world, x, y, z) {
            continue;
        }
        if !zone.common {
            return Some(zone);
        }
        common = Some(zone);
    }
    common
}

/// The zone a player stands in right now; spectators stand nowhere.
#[must_use]
pub fn standing_in(zones: &[Zone], player: &Player) -> Option<Zone> {
    if player.is_spectator() {
        return None;
    }
    let position = player.position();
    let world = player.get_world().key.to_string();
    at(zones, &world, position.x, position.y, position.z).cloned()
}

/// What the action bar says of a zone.
#[must_use]
pub fn label(zone: &Zone, now: DateTime<Utc>) -> TextComponent {
    let name = TextComponent::plain(zone.name()).color(Color::White);
    if zone.common {
        return name;
    }
    let dot = TextComponent::plain(" · ").color(Color::DarkGray);
    let text = TextComponent::plain("").add_child(name).add_child(dot);
    match (zone.status.as_str(), &zone.auction) {
        ("auction", Some(auction)) => {
            let bid = auction.top.map_or_else(
                || format!("аукцион, старт {} АР", auction.start_price),
                |top| format!("аукцион, ставка {top} АР"),
            );
            let hint = if zone.auction_open(now) {
                " · ставка: /zones bid"
            } else {
                " · /zones"
            };
            text.add_child(TextComponent::plain(bid).color(Color::Gold))
                .add_child(TextComponent::plain(hint).color(Color::DarkGray))
        }
        ("leased", _) => text.add_child(
            TextComponent::plain(zone.owner.clone().unwrap_or_else(|| "занята".to_owned()))
                .color(Color::Aqua),
        ),
        _ => text.add_child(TextComponent::plain("свободна").color(Color::Green)),
    }
}

/// A line for everyone and, for the winner of an auction, a line of their own.
#[derive(Debug, PartialEq, Eq)]
pub struct Announcement {
    /// For everyone online.
    pub all: String,
    /// The nickname of the winner and what they are told.
    pub winner: Option<(String, String)>,
}

/// What changed between two lists that players should hear of: auctions that ended and zones
/// that came up for auction.
#[must_use]
pub fn announcements(before: &[Zone], after: &[Zone], now: DateTime<Utc>) -> Vec<Announcement> {
    let mut lines = Vec::new();
    for zone in after {
        let Some(was) = before.iter().find(|was| was.id == zone.id) else {
            continue;
        };
        if zone.common {
            continue;
        }
        let name = zone.name();
        let (was_open, was_soon) = (was.auction_open(now), was.auction_soon(now));
        if was_open
            && zone.leased()
            && let Some(owner) = &zone.owner
        {
            lines.push(Announcement {
                all: format!("Аукцион: {name} уходит {owner} на 7 дней."),
                winner: Some((
                    owner.clone(),
                    format!(
                        "Поздравляем! {name} твоя на 7 дней. Продлить можно в /zones за 2 дня \
                         до конца аренды."
                    ),
                )),
            });
        } else if !was_soon
            && !was_open
            && zone.auction_soon(now)
            && let Some(auction) = &zone.auction
        {
            lines.push(Announcement {
                all: format!(
                    "Аукцион: {name} выставят через {}, старт {} АР. Ставки принимаются только \
                     от тех, кто стоит в зоне. Подробнее в /zones.",
                    left(auction.starts_at, now),
                    auction.start_price
                ),
                winner: None,
            });
        } else if !was_open
            && zone.auction_open(now)
            && let Some(auction) = &zone.auction
        {
            lines.push(Announcement {
                all: format!(
                    "Аукцион: {name} выставлена, старт {} АР, {}. Чтобы поставить, встань в эту \
                     зону и нажми /zones bid.",
                    auction.start_price,
                    left(auction.ends_at, now)
                ),
                winner: None,
            });
        }
    }
    lines
}

/// The zones of a `zones` answer; a zone the server can't read is logged and left out.
#[must_use]
pub fn parse(answer: &Value) -> Vec<Zone> {
    answer
        .get("zones")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|zone| {
            let parsed = Zone::parse(zone);
            if parsed.is_none() {
                log::warn!("Zones: bad zone {zone}");
            }
            parsed
        })
        .collect()
}

impl Bank {
    /// Whether the zones are open to players.
    #[must_use]
    pub const fn zones_open(&self) -> bool {
        self.config.zones
    }

    /// Reloads the list from the site; `then` runs on the server tick with the answer.
    pub fn refresh_zones(self: &Arc<Self>, then: Option<Listed>) {
        if !self.configured() {
            return;
        }
        if then.is_none() && self.zones.loading.swap(true, Ordering::AcqRel) {
            return;
        }
        let bank = Arc::clone(self);
        self.call("zones", &json!({}), move |server, answer| {
            bank.zones.loading.store(false, Ordering::Release);
            if ok(&answer) {
                bank.apply_zones(server, &answer);
            }
            if let Some(then) = then {
                then(server, &answer);
            }
        });
    }

    /// Takes a `zones` answer of the site.
    fn apply_zones(&self, server: &Server, answer: &Value) {
        let next = Arc::new(parse(answer));
        let before = {
            let mut list = self.zones.list.lock();
            let before = Arc::clone(&list);
            *list = Arc::clone(&next);
            before
        };
        if before.is_empty() || !self.zones_open() {
            return;
        }
        let players = server.get_players();
        for line in announcements(&before, &next, Utc::now()) {
            let all = TextComponent::plain(line.all).color(Color::Gold);
            for player in &players {
                player.send_message(&all);
            }
            let Some((nick, text)) = line.winner else {
                continue;
            };
            let winner = players
                .iter()
                .find(|player| player.gameprofile.name().eq_ignore_ascii_case(&nick));
            if let Some(winner) = winner {
                winner.send_message(&TextComponent::plain(text).color(Color::Green));
            }
        }
    }

    /// The zones' part of the server tick.
    pub(super) fn tick_zones(self: &Arc<Self>, server: &Server, tick: u64) {
        if tick % REFRESH_TICKS == 5 {
            self.refresh_zones(None);
        }
        if tick.is_multiple_of(BAR_TICKS) {
            self.tick_bars(server);
        }
        if tick.is_multiple_of(HUD_TICKS) {
            self.tick_markup(server, tick);
        }
        if tick.is_multiple_of(LIVE_TICKS) {
            let windows: Vec<Arc<dyn Live>> = {
                let mut live = self.zones.live.lock();
                live.retain(|window| window.upgrade().is_some_and(|window| !window.closed()));
                live.iter().filter_map(Weak::upgrade).collect()
            };
            for window in windows {
                window.refresh();
            }
        }
        if !tick.is_multiple_of(HUD_TICKS) || !self.zones_open() {
            return;
        }
        let zones = self.zones.all();
        let now = Utc::now();
        for player in server.get_players() {
            self.hud(&zones, &player, now);
            let lot = standing_in(&zones, &player)
                .filter(|zone| !zone.common)
                .map(|zone| zone.id);
            let uuid = player.gameprofile.id();
            let mut presence = self.zones.presence.lock();
            if presence.zones.get(&uuid) != lot.as_ref() {
                match lot {
                    Some(lot) => presence.zones.insert(uuid, lot),
                    None => presence.zones.remove(&uuid),
                };
                presence.dirty = true;
            }
        }
        if self.zones.presence.lock().dirty || tick.is_multiple_of(PRESENCE_TICKS) {
            self.send_presence();
        }
    }

    fn send_presence(self: &Arc<Self>) {
        if !self.configured() {
            return;
        }
        let players: Vec<Value> = {
            let mut presence = self.zones.presence.lock();
            if presence.sending {
                return;
            }
            presence.dirty = false;
            presence.sending = true;
            presence
                .zones
                .iter()
                .map(|(uuid, zone)| json!({"uuid": uuid.to_string(), "zone_id": zone}))
                .collect()
        };
        let bank = Arc::clone(self);
        self.call(
            "zone_presence",
            &json!({"players": players}),
            move |_, _| {
                // A list the site missed goes out again with the next unchanged one.
                bank.zones.presence.lock().sending = false;
            },
        );
    }

    /// The action bar on entering another zone, or on leaving them.
    fn hud(&self, zones: &[Zone], player: &Player, now: DateTime<Utc>) {
        // Spectators get no zone messages.
        if player.is_spectator() {
            return;
        }
        let zone = standing_in(zones, player);
        let key = zone
            .as_ref()
            .map(|zone| format!("{}|{}|{:?}", zone.id, zone.status, zone.owner));
        let uuid = player.gameprofile.id();
        let before = {
            let mut current = self.zones.current.lock();
            match &key {
                Some(key) => current.insert(uuid, key.clone()),
                None => current.remove(&uuid),
            }
        };
        if before == key {
            return;
        }
        let Some(zone) = zone else {
            player
                .send_overlay_message(&TextComponent::plain("Ты вышел из зоны").color(Color::Gray));
            return;
        };
        player.send_overlay_message(&label(&zone, now));
        let same_zone = before.is_some_and(|before| before.starts_with(&format!("{}|", zone.id)));
        if zone.auction_open(now) && !same_zone {
            player.send_message(
                &TextComponent::plain(format!(
                    "Ты в зоне «{}», по ней идёт аукцион. Ставку можно сделать, пока стоишь \
                     здесь: ",
                    zone.name()
                ))
                .color(Color::Gold)
                .add_child(
                    TextComponent::plain("[Сделать ставку]")
                        .color(Color::Green)
                        .underlined(true)
                        .click_event(ClickEvent::run_command("/zones bid"))
                        .hover_event(HoverEvent::show_text(TextComponent::plain("/zones bid"))),
                ),
            );
        }
    }

    /// A player left: the site and the action bar forget where they stood.
    pub(super) fn leave_zones(&self, player: Uuid) {
        self.zones.forget(player);
    }
}

#[cfg(test)]
mod tests {
    use std::slice;

    use chrono::Duration;

    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-15T12:00:00Z")
            .map_or_default(|time| time.with_timezone(&Utc))
    }

    fn lot(status: &str) -> Zone {
        Zone {
            id: "z-1".to_owned(),
            num: 1,
            common: false,
            title: None,
            world: "minecraft:overworld".to_owned(),
            points: vec![(0, 0), (10, 0), (10, 10), (0, 10)],
            y_min: 60,
            y_max: 100,
            start_price: 5,
            status: status.to_owned(),
            owner: None,
            mine: false,
            lease_until: None,
            renew_price: Some(5),
            auction: None,
        }
    }

    fn auction(starts_in: i64, ends_in: i64) -> Auction {
        Auction {
            id: "a-1".to_owned(),
            announced_at: None,
            starts_at: now() + Duration::minutes(starts_in),
            ends_at: now() + Duration::minutes(ends_in),
            start_price: 5,
            top: None,
            top_nickname: None,
            min_bid: 5,
            bids: 0,
        }
    }

    #[test]
    fn a_lot_wins_over_the_common_circle() {
        let common = Zone {
            id: "z-0".to_owned(),
            num: 0,
            common: true,
            points: vec![(-20, -20), (20, -20), (20, 20), (-20, 20)],
            ..lot("free")
        };
        let zones = vec![common, lot("free")];
        let world = "minecraft:overworld";
        assert_eq!(
            at(&zones, world, 5.0, 70.0, 5.0).map(|zone| zone.num),
            Some(1)
        );
        assert_eq!(
            at(&zones, world, -5.0, 70.0, 5.0).map(|zone| zone.num),
            Some(0)
        );
        assert_eq!(
            at(&zones, world, 50.0, 70.0, 5.0).map(|zone| zone.num),
            None
        );
        assert_eq!(at(&zones, "minecraft:the_nether", 5.0, 70.0, 5.0), None);
        assert_eq!(at(&zones, world, 5.0, 101.5, 5.0), None);
    }

    #[test]
    fn auctions_are_announced_once_as_they_change() {
        let free = lot("free");
        let soon = Zone {
            auction: Some(auction(30, 90)),
            ..lot("auction")
        };
        let open = Zone {
            auction: Some(auction(-1, 59)),
            ..lot("auction")
        };
        let won = Zone {
            owner: Some("buyer".to_owned()),
            ..lot("leased")
        };
        let told = |before: &Zone, after: &Zone| {
            announcements(slice::from_ref(before), slice::from_ref(after), now())
        };
        assert_eq!(told(&free, &free), [] as [Announcement; 0]);
        let line = told(&free, &soon);
        assert_eq!(line.len(), 1);
        assert!(
            line[0]
                .all
                .contains("выставят через 30 мин 0 сек, старт 5 АР")
        );
        assert_eq!(told(&soon, &soon), [] as [Announcement; 0]);
        let line = told(&soon, &open);
        assert!(
            line[0]
                .all
                .contains("выставлена, старт 5 АР, 59 мин 0 сек.")
        );
        assert_eq!(told(&open, &open), [] as [Announcement; 0]);
        let line = told(&open, &won);
        assert_eq!(line[0].all, "Аукцион: Зона 1 уходит buyer на 7 дней.");
        assert_eq!(
            line[0].winner.as_ref().map(|(nick, _)| nick.as_str()),
            Some("buyer")
        );
        // A zone the server sees for the first time is not news.
        assert_eq!(announcements(&[], &[open], now()), [] as [Announcement; 0]);
    }
}
