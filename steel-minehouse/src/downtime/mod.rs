//! Maintenance mode and the restart countdown: `/maintenance` and `/restart`, a port of
//! `Maintenance` and `Restart` of the Fabric mod `minehouse-tweaks`.
//!
//! `/maintenance on [время] [причина]` closes the server to everyone without the permission
//! `minehouse.maintenance.bypass`, kicks those already online and shows a notice in the
//! server list. `/restart [время] [причина]` counts down with a bossbar, chat warnings,
//! titles and a sound, then kicks everyone and stops the server; the host brings it back up.
//! Both share one countdown: a new one replaces the old.

use std::{
    fs, io,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use steel_core::{
    boss_event::ServerBossEvent,
    command::CommandRegistry,
    entity::Entity,
    permission::{PermissionExpr, PermissionKey},
    player::Player,
    server::Server,
};
use steel_protocol::packets::game::{BossBarColor, BossBarOverlay, CSound, SoundSource};
use steel_registry::sound_events;
use steel_utils::locks::SyncMutex;
use text_components::TextComponent;
use tokio::{runtime::Handle, task::AbortHandle, time::sleep};
use uuid::Uuid;

use crate::{config, moderation::format::render};

mod commands;
pub mod settings;
pub mod time;

use settings::DowntimeConfig;
use time::{clock, crossed, describe};

/// Who may join and stay while the server is closed.
const BYPASS_PERMISSION: &str = "minehouse.maintenance.bypass";
/// Seconds left at which chat (and a sound) warns everyone.
const CHAT_AT: [u64; 16] = [
    3600, 1800, 900, 600, 300, 180, 120, 60, 30, 15, 10, 5, 4, 3, 2, 1,
];
/// Seconds left at which a title shows in the middle of the screen.
const TITLE_AT: [u64; 13] = [300, 60, 30, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1];
/// How often the countdown looks at the clock.
const COUNTDOWN_STEP: Duration = Duration::from_millis(200);
/// Time for the kick messages to reach the clients before the server stops.
const KICK_FLUSH: Duration = Duration::from_millis(500);

/// What survives a restart: `maintenance.toml` in the data directory, apart from the config
/// a deploy overwrites.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
struct MaintenanceState {
    enabled: bool,
    reason: String,
}

/// A running countdown, to a restart or to maintenance mode.
struct Countdown {
    bar: Arc<ServerBossEvent>,
    deadline: Instant,
    total: u64,
    reason: String,
    to_maintenance: bool,
    task: AbortHandle,
}

impl Countdown {
    /// Seconds left, rounded up.
    fn seconds_left(&self) -> u64 {
        seconds_until(self.deadline)
    }
}

fn seconds_until(deadline: Instant) -> u64 {
    let left = deadline.saturating_duration_since(Instant::now());
    left.as_secs() + u64::from(left.subsec_nanos() > 0)
}

/// What `/maintenance` and `/restart` show about the countdown.
struct CountdownInfo {
    seconds_left: u64,
    reason: String,
    to_maintenance: bool,
}

/// Maintenance mode and the countdown of one server.
pub struct Downtime {
    runtime: Handle,
    state_file: PathBuf,
    maintenance: SyncMutex<MaintenanceState>,
    countdown: SyncMutex<Option<Countdown>>,
}

impl Downtime {
    /// Loads the saved mode and registers the commands. Call before the server is built,
    /// inside the Tokio runtime.
    pub fn register(registry: &mut CommandRegistry) -> Option<Arc<Self>> {
        let state_file = config::data_path("maintenance.toml");
        let state = match fs::read_to_string(&state_file) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|error| {
                log::error!(
                    "Could not read {}, maintenance stays off: {error}",
                    state_file.display()
                );
                MaintenanceState::default()
            }),
            Err(_) => MaintenanceState::default(),
        };
        let downtime = Arc::new(Self {
            runtime: Handle::try_current()
                .inspect_err(|error| log::error!("Maintenance and restart are off: {error}"))
                .ok()?,
            state_file,
            maintenance: SyncMutex::new(state),
            countdown: SyncMutex::new(None),
        });
        if let Err(error) = commands::register(registry, &downtime) {
            log::error!("Could not register /maintenance and /restart: {error}");
        }
        Some(downtime)
    }

    /// Applies the saved mode to the server list. Call once the server exists.
    pub fn start(&self, server: &Server) {
        let state = self.maintenance.lock().clone();
        if state.enabled {
            log::warn!("Maintenance mode is on: only players with {BYPASS_PERMISSION} get in");
        }
        apply_motd(server, &state);
    }

    /// Stops the countdown task.
    pub fn stop(&self) {
        let countdown = self.countdown.lock().take();
        if let Some(countdown) = countdown {
            countdown.task.abort();
        }
    }

    // --- Maintenance -------------------------------------------------------------------

    fn maintenance(&self) -> MaintenanceState {
        self.maintenance.lock().clone()
    }

    /// The text that refuses a login while the server is closed; `None` lets the player in.
    #[must_use]
    pub fn refusal(&self, server: &Server, uuid: Uuid, name: &str) -> Option<TextComponent> {
        let state = self.maintenance();
        if !state.enabled {
            return None;
        }
        let bypass = bypass_permission().is_some_and(|permission| {
            server
                .player_effective_permissions(uuid)
                .allows(&permission)
        });
        if bypass {
            log::info!("Maintenance: letting {name} in");
            return None;
        }
        log::info!("Maintenance: refused {name}");
        Some(kick_message(&DowntimeConfig::load(), &state.reason))
    }

    /// Switches the mode and writes it to disk. Returns whether the file was written.
    fn set_maintenance(&self, server: &Server, enabled: bool, reason: &str) -> bool {
        let state = MaintenanceState {
            enabled,
            reason: if enabled {
                reason.trim().to_owned()
            } else {
                String::new()
            },
        };
        *self.maintenance.lock() = state.clone();
        apply_motd(server, &state);
        self.save(&state)
            .inspect_err(|error| {
                log::error!("Could not save {}: {error}", self.state_file.display());
            })
            .is_ok()
    }

    fn save(&self, state: &MaintenanceState) -> io::Result<()> {
        let text = toml::to_string(state).map_err(io::Error::other)?;
        if let Some(parent) = self.state_file.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp = self.state_file.with_extension("toml.tmp");
        fs::write(&temp, text)?;
        fs::rename(&temp, &self.state_file)
    }

    /// Turns the mode on and kicks everyone without the bypass permission. Returns how many
    /// were kicked and whether the mode was saved.
    fn turn_on(&self, server: &Server, reason: &str) -> (usize, bool) {
        let saved = self.set_maintenance(server, true, reason);
        let message = kick_message(&DowntimeConfig::load(), reason.trim());
        let permission = bypass_permission();
        let mut kicked = 0;
        for player in server.get_players() {
            let bypass = permission
                .as_ref()
                .is_some_and(|permission| player.has_permission(permission));
            if !bypass {
                player.disconnect(message.clone());
                kicked += 1;
            }
        }
        log::info!("Maintenance on, kicked {kicked}");
        (kicked, saved)
    }

    // --- Countdown ---------------------------------------------------------------------

    fn countdown_info(&self) -> Option<CountdownInfo> {
        self.countdown
            .lock()
            .as_ref()
            .map(|countdown| CountdownInfo {
                seconds_left: countdown.seconds_left(),
                reason: countdown.reason.clone(),
                to_maintenance: countdown.to_maintenance,
            })
    }

    /// Starts a countdown, replacing the one that runs. Returns whether there was one.
    fn start_countdown(
        self: &Arc<Self>,
        server: &Arc<Server>,
        seconds: u64,
        reason: &str,
        to_maintenance: bool,
    ) -> bool {
        let replaced = self.remove_countdown().is_some();
        let reason = reason.trim().to_owned();
        let config = DowntimeConfig::load();
        log::info!(
            "{} in {}{}",
            if to_maintenance {
                "Maintenance"
            } else {
                "Restart"
            },
            describe(seconds),
            if reason.is_empty() {
                String::new()
            } else {
                format!(" ({reason})")
            }
        );

        let bar = Arc::new(ServerBossEvent::new(
            Uuid::new_v4(),
            TextComponent::plain(""),
            BossBarColor::Green,
            BossBarOverlay::Progress,
        ));
        let deadline = Instant::now() + Duration::from_secs(seconds);
        let task = self
            .runtime
            .spawn(Arc::clone(self).run_countdown(Arc::clone(server), Arc::clone(&bar)));
        *self.countdown.lock() = Some(Countdown {
            bar,
            deadline,
            total: seconds,
            reason: reason.clone(),
            to_maintenance,
            task: task.abort_handle(),
        });

        let format = if to_maintenance {
            &config.maintenance_start_message
        } else {
            &config.restart_start_message
        };
        announce(
            server,
            format,
            &config.restart_reason_line,
            &reason,
            &describe(seconds),
        );
        replaced
    }

    /// Takes the countdown away without telling players.
    fn remove_countdown(&self) -> Option<Countdown> {
        let countdown = self.countdown.lock().take()?;
        countdown.task.abort();
        countdown.bar.remove_all_players();
        Some(countdown)
    }

    /// Cancels the countdown and tells players. Returns whether it led to maintenance, or
    /// `None` when nothing ran.
    fn cancel_countdown(&self, server: &Server) -> Option<bool> {
        let countdown = self.remove_countdown()?;
        let config = DowntimeConfig::load();
        log::info!(
            "{} cancelled",
            if countdown.to_maintenance {
                "Maintenance countdown"
            } else {
                "Restart"
            }
        );
        let format = if countdown.to_maintenance {
            &config.maintenance_cancel_message
        } else {
            &config.restart_cancel_message
        };
        announce(server, format, "", "", "");
        for player in server.get_players() {
            player.clear_titles();
        }
        Some(countdown.to_maintenance)
    }

    async fn run_countdown(self: Arc<Self>, server: Arc<Server>, bar: Arc<ServerBossEvent>) {
        // The first look only draws the bar: the command already said it in chat.
        let mut shown: Option<u64> = None;
        loop {
            let Some((left, total, reason, to_maintenance)) = self
                .countdown
                .lock()
                .as_ref()
                .filter(|countdown| Arc::ptr_eq(&countdown.bar, &bar))
                .map(|countdown| {
                    (
                        countdown.seconds_left(),
                        countdown.total,
                        countdown.reason.clone(),
                        countdown.to_maintenance,
                    )
                })
            else {
                return;
            };

            if left == 0 {
                // Not through `remove_countdown`: that would abort this very task.
                let finished = self.countdown.lock().take();
                if let Some(finished) = finished {
                    finished.bar.remove_all_players();
                }
                if to_maintenance {
                    self.turn_on(&server, &reason);
                } else {
                    stop_server(&server, &reason).await;
                }
                return;
            }

            if shown != Some(left) {
                let config = DowntimeConfig::load();
                update_bar(&server, &bar, &config, left, total, to_maintenance);
                if let Some(previous) = shown {
                    warn_players(&server, &config, previous, left, &reason, to_maintenance);
                }
                shown = Some(left);
            }
            sleep(COUNTDOWN_STEP).await;
        }
    }

    /// `/restart` and `/restart now`: no countdown.
    fn restart_now(self: &Arc<Self>, server: &Arc<Server>) {
        self.remove_countdown();
        let server = Arc::clone(server);
        self.runtime.spawn(async move {
            stop_server(&server, "").await;
        });
    }
}

/// Shows the maintenance notice in the server list, or puts the usual MOTD back.
fn apply_motd(server: &Server, state: &MaintenanceState) {
    let motd = DowntimeConfig::load().maintenance_motd;
    let motd = (state.enabled && !motd.trim().is_empty())
        .then(|| legacy_codes(&motd.replace("{reason}", &state.reason)));
    server.set_status_motd(motd);
}

fn bypass_permission() -> Option<PermissionExpr> {
    PermissionKey::parse(BYPASS_PERMISSION)
        .inspect_err(|error| log::error!("Bad permission {BYPASS_PERMISSION}: {error}"))
        .ok()
        .map(PermissionExpr::key)
}

/// Renders a text whose `{reason}` is a whole line (`reason_line`), empty without a reason.
fn fill(format: &str, reason_line: &str, reason: &str, time: &str) -> TextComponent {
    let line = if reason.is_empty() { "" } else { reason_line };
    render(
        &format.replace("{reason}", line),
        &[("reason", reason), ("time", time)],
    )
}

fn kick_message(config: &DowntimeConfig, reason: &str) -> TextComponent {
    fill(
        &config.maintenance_kick_message,
        &config.maintenance_reason_line,
        reason,
        "",
    )
}

fn announce(server: &Server, format: &str, reason_line: &str, reason: &str, time: &str) {
    if !format.trim().is_empty() {
        server.broadcast_system_chat(&fill(format, reason_line, reason, time), None);
    }
}

/// `&` color codes as the `§` ones the server list understands.
fn legacy_codes(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        let is_code = character == '&'
            && characters
                .peek()
                .is_some_and(|next| "0123456789abcdefklmnor".contains(next.to_ascii_lowercase()));
        result.push(if is_code { '§' } else { character });
    }
    result
}

fn update_bar(
    server: &Server,
    bar: &Arc<ServerBossEvent>,
    config: &DowntimeConfig,
    left: u64,
    total: u64,
    to_maintenance: bool,
) {
    let format = if to_maintenance {
        &config.maintenance_bossbar
    } else {
        &config.restart_bossbar
    };
    bar.set_name(render(format, &[("time", &clock(left))]));
    #[expect(
        clippy::cast_precision_loss,
        reason = "A countdown is at most a day of seconds."
    )]
    bar.set_progress(if total == 0 {
        0.0
    } else {
        (left as f32 / total as f32).clamp(0.0, 1.0)
    });
    bar.set_color(if left > 300 {
        BossBarColor::Green
    } else if left > 60 {
        BossBarColor::Yellow
    } else {
        BossBarColor::Red
    });

    let online = server.get_players();
    for tracked in bar.players() {
        if !online.iter().any(|player| Arc::ptr_eq(player, &tracked)) {
            bar.remove_player(&tracked);
        }
    }
    for player in &online {
        bar.add_player(player);
    }
}

fn warn_players(
    server: &Server,
    config: &DowntimeConfig,
    previous: u64,
    left: u64,
    reason: &str,
    to_maintenance: bool,
) {
    let players = server.get_players();
    if crossed(&CHAT_AT, previous, left) {
        let format = if to_maintenance {
            &config.maintenance_warn_message
        } else {
            &config.restart_warn_message
        };
        announce(
            server,
            format,
            &config.restart_reason_line,
            reason,
            &describe(left),
        );
        let pitch = if left <= 5 {
            2.0
        } else if left <= 60 {
            1.4
        } else {
            1.0
        };
        for player in &players {
            pling(player, pitch);
        }
    }

    if config.restart_titles && crossed(&TITLE_AT, previous, left) {
        let last = left <= 10;
        let time = if last {
            left.to_string()
        } else {
            describe(left)
        };
        let title = render(&config.restart_title, &[("time", &time)]);
        let subtitle = render(
            if to_maintenance {
                &config.maintenance_subtitle
            } else {
                &config.restart_subtitle
            },
            &[("time", &describe(left)), ("reason", reason)],
        );
        for player in &players {
            if last {
                player.send_title_times(0, 25, 5);
            } else {
                player.send_title_times(10, 60, 20);
            }
            player.send_subtitle(subtitle.clone());
            player.send_title(title.clone());
        }
    }
}

fn pling(player: &Player, pitch: f32) {
    player.send_packet(CSound::new(
        &sound_events::BLOCK_NOTE_BLOCK_PLING,
        SoundSource::Master,
        player.position(),
        1.0,
        pitch,
        rand::random(),
    ));
}

/// Kicks everyone with the restart text and stops the server as `/stop` does: worlds and
/// players are saved, then the process ends and the host starts it again.
async fn stop_server(server: &Server, reason: &str) {
    let config = DowntimeConfig::load();
    log::info!("Restart: stopping the server");
    let message = fill(
        &config.restart_kick_message,
        &config.restart_reason_line,
        reason,
        "",
    );
    for player in server.get_players() {
        player.disconnect(message.clone());
    }
    sleep(KICK_FLUSH).await;
    server.cancel_token.cancel();
}

#[cfg(test)]
mod tests {
    use text_components::resolving::NoResolutor;

    use super::*;

    #[test]
    fn reason_line_only_with_a_reason() {
        let config = DowntimeConfig::default();
        let plain = |text: TextComponent| text.to_plain(&NoResolutor);
        assert_eq!(
            plain(fill(
                &config.restart_start_message,
                &config.restart_reason_line,
                "",
                "5 мин"
            )),
            "[Сервер] Перезапуск через 5 мин."
        );
        assert_eq!(
            plain(fill(
                &config.restart_start_message,
                &config.restart_reason_line,
                "обновление &l",
                "5 мин"
            )),
            "[Сервер] Перезапуск через 5 мин. Причина: обновление &l"
        );
        assert!(plain(kick_message(&config, "чиним")).contains("Причина: чиним"));
    }

    #[test]
    fn motd_gets_section_codes() {
        assert_eq!(
            legacy_codes("&6&lMineHouse & Co &x"),
            "§6§lMineHouse & Co &x"
        );
    }

    #[test]
    fn state_round_trips() {
        let state = MaintenanceState {
            enabled: true,
            reason: "обновление".to_owned(),
        };
        let text = toml::to_string(&state).unwrap_or_default();
        assert_eq!(toml::from_str::<MaintenanceState>(&text).ok(), Some(state));
        assert_eq!(
            toml::from_str::<MaintenanceState>("").ok(),
            Some(MaintenanceState::default())
        );
    }
}
