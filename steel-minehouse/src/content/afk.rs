//! Players idle for a while get «[AFK]» before their name (the Fabric `AfkDisplay`).
//!
//! An idle player is put into a scoreboard team whose prefix is the mark; the vanilla client
//! draws a team prefix over the head. "Idle" is vanilla's last action time: movement, chat,
//! clicks and inventory use all reset it. The team the player was in before is given back on
//! return. A player marked AFK does not count towards `playersSleepingPercentage`, the way
//! a spectator does not, unless they lie in bed.

use std::{collections::BTreeMap, env, sync::Arc, time::Duration};

use rustc_hash::FxHashSet;
use steel_core::{
    player::Player,
    scoreboard::{ScoreHolder, Scoreboard, TeamProperty},
    server::{
        Server,
        jobs::{JobPoll, ServerJob, ServerJobContext},
    },
    world::SleepObserver,
};
use steel_utils::locks::SyncMutex;
use uuid::Uuid;

use crate::moderation::format::render;

/// The team idle players are in.
const TEAM: &str = "minehouse_afk";
/// What stands before the name (the Fabric `afk.prefix`).
const PREFIX: &str = "&7[AFK] ";
/// How long without an action makes a player AFK (the Fabric `afk.minutes`).
const IDLE_AFTER: Duration = Duration::from_mins(5);
/// Shortens [`IDLE_AFTER`] to this many seconds, for tests on a local server.
const IDLE_SECONDS_ENV: &str = "MINEHOUSE_AFK_SECONDS";
const CHECK_EVERY_TICKS: u64 = 10;

/// Who is marked AFK now, for the sleep count.
static AWAY: SyncMutex<Option<FxHashSet<Uuid>>> = SyncMutex::new(None);

fn mark(id: Uuid, away: bool) {
    let mut marked = AWAY.lock();
    let marked = marked.get_or_insert_with(FxHashSet::default);
    if away {
        marked.insert(id);
    } else {
        marked.remove(&id);
    }
}

/// Leaves AFK players out of the players that have to sleep.
pub struct AfkSleep;

/// The one instance, installed at start.
pub static AFK_SLEEP: AfkSleep = AfkSleep;

impl SleepObserver for AfkSleep {
    fn skips(&self, player: &Player) -> bool {
        AWAY.lock()
            .as_ref()
            .is_some_and(|marked| marked.contains(&player.gameprofile.id()))
    }
}

/// A player marked AFK: where their team is and what to give back.
struct Away {
    name: String,
    domain: String,
    /// The team the player was in before.
    previous: Option<String>,
}

/// Whether a player idle for `idle` is AFK when the mark comes after `after`.
fn should_be_away(idle: Duration, after: Duration) -> bool {
    idle >= after
}

/// Puts `name` into the AFK team of `scoreboard`. Returns the team they were in before.
fn enter(scoreboard: &Scoreboard, name: &str) -> Option<String> {
    let holder = ScoreHolder::new(name);
    let previous = scoreboard
        .holder_team_name(&holder)
        .filter(|team| team != TEAM);
    if scoreboard.team(TEAM).is_none() && scoreboard.add_team(TEAM).is_err() {
        return previous;
    }
    let prefix = render(PREFIX, &[]);
    if scoreboard
        .team(TEAM)
        .is_some_and(|team| *team.player_prefix() != prefix)
        && let Err(error) = scoreboard.modify_team(TEAM, TeamProperty::PlayerPrefix(prefix))
    {
        log::warn!("MineHouse AFK: could not set the prefix: {error}");
    }
    if let Err(error) = scoreboard.add_holder_to_team(&holder, TEAM) {
        log::warn!("MineHouse AFK: could not mark {name}: {error}");
    }
    previous
}

/// Takes `name` out of the AFK team of `scoreboard` and back into `previous`, if it still
/// exists. Does nothing if someone already moved them to another team.
fn leave(scoreboard: &Scoreboard, name: &str, previous: Option<&str>) {
    let holder = ScoreHolder::new(name);
    if scoreboard.holder_team_name(&holder).as_deref() != Some(TEAM) {
        return;
    }
    scoreboard.remove_holder_from_team(name);
    if let Some(previous) = previous
        && scoreboard.team(previous).is_some()
        && let Err(error) = scoreboard.add_holder_to_team(&holder, previous)
    {
        log::warn!("MineHouse AFK: could not put {name} back into {previous}: {error}");
    }
}

/// Watches idle times. A job of the server tick, never finished.
struct Afk {
    after: Duration,
    away: BTreeMap<Uuid, Away>,
    /// Who was online at the last check, to clean up after a crash on their first check.
    online: FxHashSet<Uuid>,
}

impl Afk {
    fn check(&mut self, server: &Server, players: &[Arc<Player>]) {
        for player in players {
            let id = player.gameprofile.id();
            let name = player.gameprofile.name();
            let world = player.get_world();
            let Some(scoreboard) = server.scoreboards.get(world.domain()) else {
                continue;
            };
            let idle = should_be_away(player.idle_time(), self.after);
            let was = self.away.contains_key(&id);
            if !was && !self.online.contains(&id) {
                // Left in the team by a server that stopped while they were AFK.
                leave(scoreboard, name, None);
            }
            if idle && !was {
                let previous = enter(scoreboard, name);
                self.away.insert(
                    id,
                    Away {
                        name: name.to_owned(),
                        domain: world.domain().to_owned(),
                        previous,
                    },
                );
                mark(id, true);
                world.update_sleeping_player_list();
            } else if !idle
                && was
                && let Some(away) = self.away.remove(&id)
            {
                if let Some(scoreboard) = server.scoreboards.get(&away.domain) {
                    leave(scoreboard, &away.name, away.previous.as_deref());
                }
                mark(id, false);
                world.update_sleeping_player_list();
            }
        }

        self.online = players
            .iter()
            .map(|player| player.gameprofile.id())
            .collect();
        // Whoever left while AFK gets their team back for the next join.
        let online = &self.online;
        self.away.retain(|id, away| {
            if online.contains(id) {
                return true;
            }
            if let Some(scoreboard) = server.scoreboards.get(&away.domain) {
                leave(scoreboard, &away.name, away.previous.as_deref());
            }
            mark(*id, false);
            false
        });
    }
}

impl ServerJob for Afk {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        let Some(server) = context.server() else {
            return JobPoll::Finished;
        };
        if context.tick_count.is_multiple_of(CHECK_EVERY_TICKS) {
            self.check(&server, &server.get_players());
        }
        JobPoll::Pending
    }
}

/// Starts watching idle players.
pub(super) fn start(server: &Server) {
    let after = env::var(IDLE_SECONDS_ENV)
        .ok()
        .and_then(|seconds| seconds.parse().ok())
        .map_or(IDLE_AFTER, Duration::from_secs);
    server.jobs.spawn(Afk {
        after,
        away: BTreeMap::new(),
        online: FxHashSet::default(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_player_is_away_from_the_threshold_on() {
        let after = Duration::from_mins(5);
        assert!(!should_be_away(Duration::from_secs(299), after));
        assert!(should_be_away(after, after));
        assert!(should_be_away(Duration::from_hours(1), after));
    }

    #[test]
    fn the_team_gives_the_mark_and_the_old_team_comes_back() {
        let scoreboard = Scoreboard::new();
        assert!(scoreboard.add_team("builders").is_ok());
        assert!(
            scoreboard
                .add_holder_to_team(&ScoreHolder::new("Steve"), "builders")
                .is_ok()
        );

        let previous = enter(&scoreboard, "Steve");
        assert_eq!(previous.as_deref(), Some("builders"));
        assert_eq!(
            scoreboard
                .holder_team_name(&ScoreHolder::new("Steve"))
                .as_deref(),
            Some(TEAM)
        );
        assert_eq!(
            scoreboard
                .team(TEAM)
                .map(|team| team.player_prefix().clone()),
            Some(render(PREFIX, &[]))
        );

        leave(&scoreboard, "Steve", previous.as_deref());
        assert_eq!(
            scoreboard
                .holder_team_name(&ScoreHolder::new("Steve"))
                .as_deref(),
            Some("builders")
        );
    }

    #[test]
    fn a_player_without_a_team_ends_up_without_one() {
        let scoreboard = Scoreboard::new();
        let previous = enter(&scoreboard, "Alex");
        assert_eq!(previous, None);
        leave(&scoreboard, "Alex", None);
        assert_eq!(scoreboard.holder_team_name(&ScoreHolder::new("Alex")), None);
    }

    #[test]
    fn a_team_set_by_hand_meanwhile_is_left_alone() {
        let scoreboard = Scoreboard::new();
        assert!(scoreboard.add_team("red").is_ok());
        enter(&scoreboard, "Alex");
        assert!(
            scoreboard
                .add_holder_to_team(&ScoreHolder::new("Alex"), "red")
                .is_ok()
        );
        leave(&scoreboard, "Alex", None);
        assert_eq!(
            scoreboard
                .holder_team_name(&ScoreHolder::new("Alex"))
                .as_deref(),
            Some("red")
        );
    }
}
