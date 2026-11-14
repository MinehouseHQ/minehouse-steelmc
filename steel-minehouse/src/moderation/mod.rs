//! Bans, warns and the whitelist: `/ban`, `/tempban`, `/unban`, `/banlist`, `/warn`, `/unwarn`,
//! `/warns` and `/whitelist`, a port of the commands of the Fabric mod `minehouse-auth`.
//!
//! All three live on the site and hang on the player's Discord account, so they cover every
//! nickname and alt and survive a nickname change. The commands only call the site; a
//! background task kicks whoever got banned elsewhere and announces those bans in chat.

use std::{
    sync::{Arc, OnceLock, Weak},
    time::Duration,
};

use rustc_hash::FxHashSet;
use steel_core::{command::CommandRegistry, server::Server};
use steel_utils::locks::SyncMutex;
use text_components::TextComponent;
use tokio::{runtime::Handle, task::JoinHandle, time::sleep};
use uuid::Uuid;

use crate::{config, site::SiteClient};

pub mod api;
mod commands;
pub mod format;
pub mod settings;
pub mod wipe;

use api::BanEntry;
use settings::BansConfig;

/// How long to wait before looking at the config again while the sync is turned off.
const IDLE_RECHECK: Duration = Duration::from_secs(60);

/// The moderation commands and the ban sync of one server.
pub struct Moderation {
    site: SiteClient,
    /// Which server this is for the site, as in the login check.
    server_id: String,
    /// Disconnect text when the site sends none.
    banned_message: String,
    runtime: Handle,
    /// Set by [`Self::start`]. Weak: the server owns the commands, which own this.
    server: OnceLock<Weak<Server>>,
    sync: SyncMutex<Option<JoinHandle<()>>>,
}

impl Moderation {
    /// Registers the commands. Call before the server is built, inside the Tokio runtime.
    pub fn register(
        registry: &mut CommandRegistry,
        site: SiteClient,
        server_id: String,
        banned_message: String,
    ) -> Option<Arc<Self>> {
        let moderation = Arc::new(Self {
            site,
            server_id,
            banned_message,
            runtime: Handle::try_current()
                .inspect_err(|error| log::error!("Moderation commands are off: {error}"))
                .ok()?,
            server: OnceLock::new(),
            sync: SyncMutex::new(None),
        });
        if let Err(error) = commands::register(registry, &moderation) {
            log::error!("Could not register the moderation commands: {error}");
        }
        Some(moderation)
    }

    /// Starts the ban sync. Call once the server is ready for players.
    pub fn start(self: &Arc<Self>, server: &Arc<Server>) {
        let _ = self.server.set(Arc::downgrade(server));
        let task = self
            .runtime
            .spawn(Arc::clone(self).run_sync(Arc::clone(server)));
        let previous = self.sync.lock().replace(task);
        if let Some(previous) = previous {
            previous.abort();
        }
    }

    /// Stops the ban sync.
    pub fn stop(&self) {
        let task = self.sync.lock().take();
        if let Some(task) = task {
            task.abort();
        }
    }

    async fn run_sync(self: Arc<Self>, server: Arc<Server>) {
        // Ids of the active bans at the last poll; `None` until the first successful poll,
        // which only remembers what is already there.
        let mut known: Option<FxHashSet<i64>> = None;

        loop {
            let config = BansConfig::load();
            if config.sync_seconds == 0 {
                known = None;
                sleep(IDLE_RECHECK).await;
                continue;
            }

            self.announce_site_bans(&server, &config, &mut known).await;
            sleep(Duration::from_secs(config.sync_seconds)).await;
            self.kick_banned(&server).await;
            self.wipe_requested(&server).await;
        }
    }

    /// The running server, for work a command does off the tick.
    fn server(&self) -> Option<Arc<Server>> {
        self.server.get()?.upgrade()
    }

    /// Resets the progress of the accounts it was asked for on the site (unban or ban with
    /// «сбросить прогресс»).
    async fn wipe_requested(&self, server: &Arc<Server>) {
        // The site asks a server by its console id; a server without one goes by the id of
        // its login check.
        let server_id =
            config::env_value("MINEHOUSE_SERVER_ID").unwrap_or_else(|| self.server_id.clone());
        let wipes = match self.site.wipes(&server_id).await {
            Ok(wipes) => wipes,
            Err(error) => {
                log::debug!("Wipe sync failed: {error}");
                return;
            }
        };

        for wipe in wipes {
            let report = wipe::wipe_players(server, &wipe.uuids, wipe::WIPED_MESSAGE).await;
            log::info!(
                "Site wipe #{} of {}: {} nickname(s) reset",
                wipe.id,
                wipe.nickname,
                report.wiped
            );
            let error = (!report.errors.is_empty()).then(|| report.errors.join("; "));
            if let Err(error) = self
                .site
                .wipe_done(wipe.id, report.wiped, error.as_deref())
                .await
            {
                log::warn!("Could not report site wipe #{}: {error}", wipe.id);
            }
        }
    }

    /// Kicks players banned elsewhere (on the site, in Discord) or taken off the whitelist
    /// while they are online.
    async fn kick_banned(&self, server: &Server) {
        let online: Vec<Uuid> = server
            .get_players()
            .iter()
            .map(|player| player.gameprofile.id())
            .collect();
        if online.is_empty() {
            return;
        }

        match self.site.online_bans(&online, &self.server_id).await {
            Ok(banned) => {
                for ban in banned {
                    self.kick(server, &[ban.uuid], ban.message.as_deref());
                }
            }
            Err(error) => log::debug!("Ban sync failed: {error}"),
        }
    }

    /// Announces the bans that appeared on the site since the last poll.
    async fn announce_site_bans(
        &self,
        server: &Server,
        config: &BansConfig,
        known: &mut Option<FxHashSet<i64>>,
    ) {
        if !config.enabled || !config.site {
            *known = None;
            return;
        }

        let bans = match self.site.bans().await {
            Ok(bans) => bans,
            Err(error) => {
                log::debug!("Ban announce poll failed: {error}");
                return;
            }
        };

        // An old site without ids: stay unseeded, otherwise every existing ban would look
        // new once the site starts sending them.
        let Some(current) = bans
            .iter()
            .map(|ban| ban.id)
            .collect::<Option<FxHashSet<_>>>()
        else {
            *known = None;
            return;
        };
        let Some(previous) = known.replace(current) else {
            return;
        };

        for ban in new_site_bans(&bans, &previous) {
            let seconds = ban
                .created_at
                .as_deref()
                .zip(ban.expires_at.as_deref())
                .and_then(|(created, expires)| format::duration_between(created, expires));
            announce_ban(
                server,
                config,
                &ban.nickname,
                ban.reason.as_deref(),
                seconds,
                ban.expires_at.as_deref(),
                ban.by.as_deref(),
            );
        }
    }

    /// Disconnects whoever of `uuids` is online.
    fn kick(&self, server: &Server, uuids: &[Uuid], message: Option<&str>) {
        let message = message.unwrap_or(&self.banned_message);
        for player in server.get_players() {
            if uuids.contains(&player.gameprofile.id()) {
                log::info!(
                    "Kicking banned player {} ({})",
                    player.gameprofile.name(),
                    player.gameprofile.id()
                );
                player.disconnect(TextComponent::plain(message.to_owned()));
            }
        }
    }
}

/// Bans not seen at the last poll. Game bans were announced by the command itself.
fn new_site_bans<'bans>(
    bans: &'bans [BanEntry],
    previous: &FxHashSet<i64>,
) -> impl Iterator<Item = &'bans BanEntry> {
    bans.iter().filter(|ban| {
        ban.id.is_some_and(|id| !previous.contains(&id)) && ban.source.as_deref() != Some("game")
    })
}

/// Tells everyone in chat that a player was banned.
fn announce_ban(
    server: &Server,
    config: &BansConfig,
    nickname: &str,
    reason: Option<&str>,
    seconds: Option<u64>,
    expires_at: Option<&str>,
    by: Option<&str>,
) {
    if !config.enabled {
        return;
    }
    let format = match expires_at {
        None => config.ban.trim(),
        Some(_) => config.tempban.trim(),
    };
    if format.is_empty() {
        return;
    }

    let reason = reason
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .unwrap_or(config.no_reason.trim());
    let by = by
        .filter(|by| !by.trim().is_empty())
        .unwrap_or(config.no_by.trim());
    let duration = match (expires_at, seconds) {
        (None, _) => String::new(),
        (Some(_), Some(seconds)) => format::format_duration(seconds),
        (Some(_), None) => "?".to_owned(),
    };
    let until = expires_at.map_or_default(format::format_time);

    broadcast(
        server,
        format,
        &[
            ("player", nickname),
            ("reason", reason),
            ("by", by),
            ("duration", &duration),
            ("until", &until),
        ],
    );
}

/// Sends `format` to everyone; an empty format sends nothing.
fn broadcast(server: &Server, format: &str, values: &[(&str, &str)]) {
    let format = format.trim();
    if !format.is_empty() {
        server.broadcast_system_chat(&format::render(format, values), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ban(id: i64, source: &str) -> BanEntry {
        BanEntry {
            id: Some(id),
            nickname: format!("player{id}"),
            reason: None,
            expires_at: None,
            by: None,
            source: Some(source.to_owned()),
            created_at: None,
        }
    }

    #[test]
    fn only_new_bans_made_outside_the_game_are_announced() {
        let bans = [
            ban(1, "site"),
            ban(2, "site"),
            ban(3, "game"),
            ban(4, "discord"),
        ];
        let previous: FxHashSet<i64> = [1].into_iter().collect();
        let fresh: Vec<_> = new_site_bans(&bans, &previous).map(|ban| ban.id).collect();
        assert_eq!(fresh, [Some(2), Some(4)]);
    }
}
