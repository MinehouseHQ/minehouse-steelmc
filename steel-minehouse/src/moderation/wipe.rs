//! Progress reset: the account starts over as a new player. A port of `PlayerWipe` of the
//! Fabric mod `minehouse-auth`, behind `/ban`, `/tempban`, `/unban` and `/warn` with `--wipe`
//! and behind the resets asked for on the site.
//!
//! Nothing is deleted: the player's data, statistics and advancements of every domain move
//! to `wipes/<uuid>-<time>/` in the `MineHouse` data directory, so a reset can be undone by
//! hand while the server is stopped.

use std::{sync::Arc, time::Duration};

use steel_core::server::Server;
use text_components::TextComponent;
use tokio::time::sleep;
use uuid::Uuid;

use crate::config;

/// Disconnect text for a player who is online when their progress is reset.
pub const WIPED_MESSAGE: &str =
    "Твой прогресс на MineHouse сброшен. Зайди снова, чтобы начать заново.";

/// How long a kicked player may take to leave before the reset gives up.
const LEAVE_TIMEOUT: Duration = Duration::from_secs(5);
const LEAVE_CHECK: Duration = Duration::from_millis(50);

/// What a reset of an account did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct WipeReport {
    /// How many nicknames had saved progress that is now gone.
    pub wiped: usize,
    /// Nicknames that could not be reset, with the reason.
    pub errors: Vec<String>,
}

/// Resets every nickname of an account. Players who are online are disconnected first, with
/// `kick_message` unless something else already disconnected them.
pub async fn wipe_players(server: &Arc<Server>, uuids: &[Uuid], kick_message: &str) -> WipeReport {
    let mut report = WipeReport::default();
    for &uuid in uuids {
        match wipe_player(server, uuid, kick_message).await {
            Ok(true) => report.wiped += 1,
            Ok(false) => {}
            Err(error) => {
                log::error!("Could not reset the progress of {uuid}: {error}");
                report.errors.push(format!("{uuid}: {error}"));
            }
        }
    }
    report
}

/// Resets one player. Returns whether there was anything saved.
async fn wipe_player(server: &Arc<Server>, uuid: Uuid, kick_message: &str) -> Result<bool, String> {
    let online = |server: &Server| {
        server
            .get_players()
            .into_iter()
            .find(|player| player.gameprofile.id() == uuid)
    };

    // The data of a player who is online lives in the game and is saved when they leave:
    // they have to go first, and that save has to be the one that gets archived.
    if let Some(player) = online(server) {
        player.disconnect(TextComponent::plain(kick_message.to_owned()));
        let mut waited = Duration::ZERO;
        while online(server).is_some() {
            if waited >= LEAVE_TIMEOUT {
                return Err("игрок не вышел с сервера".to_owned());
            }
            sleep(LEAVE_CHECK).await;
            waited += LEAVE_CHECK;
        }
    }

    let domains: Vec<String> = server.worlds.domain_names().map(str::to_owned).collect();
    let time = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let archive = config::data_path("wipes").join(format!("{uuid}-{time}"));
    let found = server
        .player_data_storage
        .archive_player(uuid, &domains, &archive)
        .await
        .map_err(|error| error.to_string())?;
    if found {
        log::info!(
            "Progress of {uuid} reset, the old data is in {}",
            archive.display()
        );
    }
    Ok(found)
}

/// One reset asked for on the site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteWipe {
    /// The id to report back with.
    pub id: i64,
    /// A nickname of the account, for the log.
    pub nickname: String,
    /// Every nickname of the account.
    pub uuids: Vec<Uuid>,
}
