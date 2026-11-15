//! Boss bars for zone auctions (the Fabric `ZoneBossBars`): one bar per announced or running
//! auction, the soonest three, with a countdown to its start or end, redrawn every second for
//! everyone online.
//!
//! `/zones bossbar` hides them for one player; who hid them is kept in
//! `zones-hidden-bars.txt` of the data directory.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::Arc,
};

use chrono::{DateTime, Utc};
use steel_core::{boss_event::ServerBossEvent, player::Player, server::Server};
use steel_protocol::packets::game::{BossBarColor, BossBarOverlay};
use steel_utils::locks::SyncMutex;
use text_components::TextComponent;
use uuid::Uuid;

use super::Zone;
use crate::{bank::Bank, config};

/// The most bars shown at once.
pub const MAX: usize = 3;
/// A running auction turns red in its last ten minutes.
const LAST_SECONDS: i64 = 600;
const FILE: &str = "zones-hidden-bars.txt";

#[derive(Default)]
struct State {
    bars: BTreeMap<String, Arc<ServerBossEvent>>,
    /// The length of the «начнётся через» countdown of an auction the site gave no
    /// announcement time for.
    soon_span: BTreeMap<String, i64>,
}

/// The bars and who hid them.
pub struct BossBars {
    state: SyncMutex<State>,
    hidden: SyncMutex<BTreeSet<Uuid>>,
    file: PathBuf,
}

impl BossBars {
    /// Reads who hid the bars.
    #[must_use]
    pub fn load() -> Self {
        let file = config::data_path(FILE);
        let hidden = fs::read_to_string(&file).map_or_default(|text| read_hidden(&text));
        Self {
            state: SyncMutex::new(State::default()),
            hidden: SyncMutex::new(hidden),
            file,
        }
    }

    /// Turns the bars off or on again for a player; true if they are shown now.
    pub fn toggle(&self, player: &Player) -> bool {
        let uuid = player.gameprofile.id();
        let (shown, text) = {
            let mut hidden = self.hidden.lock();
            let shown = hidden.remove(&uuid);
            if !shown {
                hidden.insert(uuid);
            }
            let lines: Vec<String> = hidden.iter().map(Uuid::to_string).collect();
            (shown, lines.join("\n"))
        };
        if !shown {
            for bar in self.state.lock().bars.values() {
                bar.remove_player(player);
            }
        }
        if let Some(parent) = self.file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(error) = fs::write(&self.file, text) {
            log::warn!("Zones: can't write {}: {error}", self.file.display());
        }
        shown
    }

    /// Draws the bars; every second.
    fn tick(&self, zones: &[Zone], open: bool, players: &[Arc<Player>], now: DateTime<Utc>) {
        let auctions = if open { shown(zones, now) } else { Vec::new() };
        let hidden = self.hidden.lock().clone();
        let mut state = self.state.lock();
        let state = &mut *state;
        let mut keep = BTreeSet::new();
        for zone in auctions {
            let Some(auction) = &zone.auction else {
                continue;
            };
            keep.insert(auction.id.clone());
            let left = (auction.starts_at - now).num_seconds().max(1);
            let span = state.soon_span.entry(auction.id.clone()).or_insert(left);
            *span = (*span).max(left);
            let view = view(zone, now, *span);
            let bar = state.bars.entry(auction.id.clone()).or_insert_with(|| {
                Arc::new(ServerBossEvent::new(
                    Uuid::new_v4(),
                    TextComponent::plain(view.name.clone()),
                    view.color,
                    BossBarOverlay::Progress,
                ))
            });
            bar.set_name(TextComponent::plain(view.name));
            bar.set_color(view.color);
            bar.set_progress(view.progress);
            // A player who left is no longer in the list of the server.
            for tracked in bar.players() {
                let online = players.iter().any(|player| Arc::ptr_eq(player, &tracked));
                if !online || hidden.contains(&tracked.gameprofile.id()) {
                    bar.remove_player(&tracked);
                }
            }
            for player in players {
                if !hidden.contains(&player.gameprofile.id()) {
                    bar.add_player(player);
                }
            }
        }
        state.bars.retain(|id, bar| {
            let kept = keep.contains(id);
            if !kept {
                bar.remove_all_players();
            }
            kept
        });
        state.soon_span.retain(|id, _| keep.contains(id));
    }
}

fn read_hidden(text: &str) -> BTreeSet<Uuid> {
    text.lines()
        .filter_map(|line| Uuid::parse_str(line.trim()).ok())
        .collect()
}

/// The auctions that get a bar: not over yet, the soonest moment first.
fn shown(zones: &[Zone], now: DateTime<Utc>) -> Vec<&Zone> {
    let mut auctions: Vec<(DateTime<Utc>, &Zone)> = zones
        .iter()
        .filter_map(|zone| {
            let auction = zone.auction.as_ref().filter(|_| zone.status == "auction")?;
            if auction.ends_at <= now {
                return None;
            }
            let next = if auction.started(now) {
                auction.ends_at
            } else {
                auction.starts_at
            };
            Some((next, zone))
        })
        .collect();
    auctions.sort_by_key(|(next, _)| *next);
    auctions
        .into_iter()
        .take(MAX)
        .map(|(_, zone)| zone)
        .collect()
}

/// What one bar shows.
#[derive(Debug, PartialEq)]
struct View {
    name: String,
    color: BossBarColor,
    progress: f32,
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a countdown of days fits a float well enough for a bar"
)]
fn part(left: i64, of: i64) -> f32 {
    (left as f32 / of.max(1) as f32).clamp(0.0, 1.0)
}

/// The bar of a zone whose auction is announced or runs. `soon_span` is how long the wait for
/// the start is taken to be when the site did not say when the auction was announced.
fn view(zone: &Zone, now: DateTime<Utc>, soon_span: i64) -> View {
    let name = zone.name();
    let Some(auction) = &zone.auction else {
        return View {
            name,
            color: BossBarColor::Yellow,
            progress: 1.0,
        };
    };
    if !auction.started(now) {
        let left = (auction.starts_at - now).num_seconds().max(0);
        let span = auction
            .announced_at
            .filter(|announced| *announced < auction.starts_at)
            .map_or(soon_span, |announced| {
                (auction.starts_at - announced).num_seconds()
            });
        return View {
            name: format!(
                "Аукцион «{name}» начнётся через {} · старт {} АР · /zones",
                clock(left),
                auction.start_price
            ),
            color: BossBarColor::Yellow,
            progress: part(left, span),
        };
    }
    let left = (auction.ends_at - now).num_seconds().max(0);
    let total = (auction.ends_at - auction.starts_at).num_seconds();
    let bid = auction.top.map_or_else(
        || format!("старт {} АР", auction.start_price),
        |top| format!("ставка {top} АР"),
    );
    View {
        name: format!(
            "Аукцион «{name}»: {bid} · осталось {} · /zones",
            clock(left)
        ),
        color: if left <= LAST_SECONDS {
            BossBarColor::Red
        } else {
            BossBarColor::Green
        },
        progress: part(left, total),
    }
}

/// `2 д 03:05:09`, `1:05:00`, `4:32`.
fn clock(seconds: i64) -> String {
    let (days, hours) = (seconds / 86400, seconds % 86400 / 3600);
    let (minutes, seconds) = (seconds % 3600 / 60, seconds % 60);
    if days > 0 {
        format!("{days} д {hours:02}:{minutes:02}:{seconds:02}")
    } else if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

impl Bank {
    /// The boss bars' part of the server tick.
    pub(super) fn tick_bars(&self, server: &Server) {
        self.zones.bars.tick(
            &self.zones.all(),
            self.zones_open(),
            &server.get_players(),
            Utc::now(),
        );
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use serde_json::json;

    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-15T12:00:00Z")
            .map_or_default(|time| time.with_timezone(&Utc))
    }

    fn zone(num: i32, starts: i64, ends: i64, top: Option<i64>) -> Zone {
        let at = |minutes: i64| (now() + Duration::minutes(minutes)).to_rfc3339();
        let listed = json!({
            "id": format!("z-{num}"), "num": num, "kind": "lot", "world": "minecraft:overworld",
            "points": [[0, 0], [10, 0], [10, 10], [0, 10]], "y_min": 60, "y_max": 100,
            "start_price": 5, "status": "auction",
            "auction": {
                "id": format!("a-{num}"), "starts_at": at(starts), "ends_at": at(ends),
                "start_price": 5, "top": top, "top_nickname": top.map(|_| "buyer"),
                "min_bid": top.map_or(5, |top| top + 1), "bids": i32::from(top.is_some()),
            },
        });
        Zone::parse(&listed).unwrap_or_else(|| unreachable!())
    }

    #[test]
    fn the_clock_drops_the_parts_it_does_not_need() {
        assert_eq!(clock(272), "4:32");
        assert_eq!(clock(3900), "1:05:00");
        assert_eq!(clock(2 * 86400 + 3 * 3600 + 5 * 60 + 9), "2 д 03:05:09");
        assert_eq!(clock(0), "0:00");
    }

    #[test]
    fn a_bar_counts_down_to_the_start_and_then_to_the_end() {
        let soon = zone(3, 30, 90, None);
        assert_eq!(
            view(&soon, now(), 3600),
            View {
                name: "Аукцион «Зона 3» начнётся через 30:00 · старт 5 АР · /zones".to_owned(),
                color: BossBarColor::Yellow,
                progress: 0.5,
            }
        );
        let running = zone(3, -30, 30, Some(12));
        assert_eq!(
            view(&running, now(), 1),
            View {
                name: "Аукцион «Зона 3»: ставка 12 АР · осталось 30:00 · /zones".to_owned(),
                color: BossBarColor::Green,
                progress: 0.5,
            }
        );
        let ending = zone(3, -50, 10, None);
        let view = view(&ending, now(), 1);
        assert_eq!(view.color, BossBarColor::Red);
        assert_eq!(
            view.name,
            "Аукцион «Зона 3»: старт 5 АР · осталось 10:00 · /zones"
        );
    }

    #[test]
    fn the_soonest_three_auctions_get_a_bar() {
        let zones = [
            zone(1, -10, 300, None),
            zone(2, 20, 80, None),
            zone(3, -60, -1, None),
            zone(4, -10, 5, None),
            zone(5, 600, 700, None),
        ];
        let nums: Vec<i32> = shown(&zones, now()).iter().map(|zone| zone.num).collect();
        assert_eq!(nums, [4, 2, 1]);
    }

    #[test]
    fn the_file_of_hidden_bars_is_one_uuid_a_line() {
        let text = "9b2e6f0a-3c1d-4f5e-8a7b-6c5d4e3f2a1b\n\nnot a uuid\n";
        assert_eq!(read_hidden(text).len(), 1);
    }
}
