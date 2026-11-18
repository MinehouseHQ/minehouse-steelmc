//! The opening of Chapter 2: on a player's first visit to this server, the `MineHouse` client mod
//! plays a short scene (the hole the old world fell into, and a new universe out of it).
//!
//! The server only gives the word: an empty `minehouse:intro` plugin message. A client without
//! the mod drops it. Who has had it is kept in `data/chapter2-intro.json`, so the scene plays
//! once; take a player out of that file to show it to them again.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

use steel_core::{
    player::Player,
    server::Server,
    server::jobs::{JobPoll, ServerJob, ServerJobContext},
};
use steel_protocol::packets::common::CPluginMessage;
use steel_utils::Identifier;
use uuid::Uuid;

use crate::config;

const CHANNEL: Identifier = Identifier::new_static("minehouse", "intro");
/// A moment for the client to take the world in; the scene covers it until its end.
const DELAY_TICKS: u64 = 5;

/// Who has seen the scene, kept on disk.
struct Seen {
    file: PathBuf,
    players: BTreeSet<Uuid>,
}

impl Seen {
    fn load(file: PathBuf) -> Self {
        let players = match fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
                log::warn!("{}: {error}, starting over", file.display());
                BTreeSet::new()
            }),
            Err(_) => BTreeSet::new(),
        };
        Self { file, players }
    }

    /// Marks `player`; true the first time.
    fn mark(&mut self, player: Uuid) -> bool {
        if !self.players.insert(player) {
            return false;
        }
        let saved = serde_json::to_string_pretty(&self.players)
            .map_err(|error| error.to_string())
            .and_then(|text| {
                if let Some(dir) = self.file.parent() {
                    fs::create_dir_all(dir).map_err(|error| error.to_string())?;
                }
                fs::write(&self.file, text).map_err(|error| error.to_string())
            });
        if let Err(error) = saved {
            log::warn!("{}: {error}", self.file.display());
        }
        true
    }
}

/// Watches who comes online and gives the newcomers to Chapter 2 their scene.
struct ChapterIntro {
    seen: Seen,
    online: BTreeSet<Uuid>,
    /// Players waiting for the scene, and the tick it is due.
    due: BTreeMap<Uuid, u64>,
}

fn play(player: &Player) {
    player.send_packet(CPluginMessage::new(CHANNEL, Box::default()));
}

impl ServerJob for ChapterIntro {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        let Some(server) = context.server() else {
            return JobPoll::Finished;
        };
        let tick = context.tick_count;
        let players = server.get_players();
        let now: BTreeSet<Uuid> = players
            .iter()
            .map(|player| player.gameprofile.id())
            .collect();
        for id in &now {
            if !self.online.contains(id) && !self.seen.players.contains(id) {
                self.due.insert(*id, tick + DELAY_TICKS);
            }
        }
        self.online = now;

        let seen = &mut self.seen;
        self.due.retain(|id, at| {
            let Some(player) = players.iter().find(|player| player.gameprofile.id() == *id) else {
                return false;
            };
            if *at > tick {
                return true;
            }
            if seen.mark(*id) {
                log::info!("{} sees the Chapter 2 opening", player.gameprofile.name());
                play(player);
            }
            false
        });
        JobPoll::Pending
    }
}

/// Starts watching for players new to Chapter 2.
pub(super) fn start(server: &Server) {
    server.jobs.spawn(ChapterIntro {
        seen: Seen::load(config::data_path("chapter2-intro.json")),
        online: BTreeSet::new(),
        due: BTreeMap::new(),
    });
}

#[cfg(test)]
mod tests {
    use std::env;

    use super::*;

    #[test]
    fn a_player_is_marked_once_and_kept_on_disk() {
        let dir = env::temp_dir().join(format!("chapter-intro-{}", Uuid::new_v4()));
        let file = dir.join("chapter2-intro.json");
        let id = Uuid::new_v4();
        let mut seen = Seen::load(file.clone());
        assert!(seen.mark(id));
        assert!(!seen.mark(id));
        assert!(Seen::load(file).players.contains(&id));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_word_is_an_empty_message_on_minehouse_intro() {
        let packet = CPluginMessage::new(CHANNEL, Box::default());
        assert_eq!(packet.channel.to_string(), "minehouse:intro");
        assert!(packet.data.is_empty());
    }
}
