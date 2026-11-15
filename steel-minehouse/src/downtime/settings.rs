//! `config/minehouse/downtime.toml`: the texts of maintenance mode and `/restart`, the
//! `maintenance.*` and `restart.*` keys of the Fabric `minehouse-tweaks.properties`.

use serde::{Deserialize, Serialize};

use crate::config;

/// Texts of maintenance mode and of the restart countdown.
///
/// They take `&` color codes and the placeholders `{time}` and `{reason}`. In the chat, kick
/// and server list texts `{reason}` is the matching `*_reason_line` (empty without a reason),
/// in a subtitle it is the bare reason. The file is read again before every use.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DowntimeConfig {
    /// What a player without the bypass permission reads while the server is closed.
    pub maintenance_kick_message: String,
    /// `{reason}` of the kick message when maintenance has a reason.
    pub maintenance_reason_line: String,
    /// The server list text while the server is closed; `{reason}` is the bare reason.
    /// Empty keeps the usual MOTD.
    pub maintenance_motd: String,
    /// Chat, when a countdown to maintenance starts.
    pub maintenance_start_message: String,
    /// Chat, at the marks of that countdown.
    pub maintenance_warn_message: String,
    /// Chat, when that countdown is cancelled.
    pub maintenance_cancel_message: String,
    /// The bossbar of that countdown.
    pub maintenance_bossbar: String,
    /// The subtitle under the time of that countdown.
    pub maintenance_subtitle: String,
    /// Chat, when a restart countdown starts.
    pub restart_start_message: String,
    /// Chat, at the marks of the restart countdown.
    pub restart_warn_message: String,
    /// Chat, when the restart is cancelled.
    pub restart_cancel_message: String,
    /// `{reason}` of the restart texts when the restart has a reason.
    pub restart_reason_line: String,
    /// The bossbar of the restart countdown.
    pub restart_bossbar: String,
    /// Show the time in the middle of the screen at 5 min, 1 min, 30 s and the last 10 s.
    pub restart_titles: bool,
    /// That title.
    pub restart_title: String,
    /// The subtitle under it.
    pub restart_subtitle: String,
    /// What everyone reads when the server goes down for the restart.
    pub restart_kick_message: String,
}

impl Default for DowntimeConfig {
    fn default() -> Self {
        Self {
            maintenance_kick_message: "&c&lТехнические работы\n\n&fСервер временно закрыт, скоро вернёмся.{reason}\n\n&7Новости в Discord: &bdiscord.gg/gCYGxDzaCb".to_owned(),
            maintenance_reason_line: "\n&7Причина: &f{reason}".to_owned(),
            maintenance_motd: "&6&lMineHouse &8| &c&lТехнические работы\n&7Скоро вернёмся, новости в Discord".to_owned(),
            maintenance_start_message: "&6[Сервер] &eТехработы через &f{time}&e: сервер закроется для игроков.{reason}".to_owned(),
            maintenance_warn_message: "&6[Сервер] &eТехработы через &f{time}&e.{reason}".to_owned(),
            maintenance_cancel_message: "&6[Сервер] &aТехработы отменены.".to_owned(),
            maintenance_bossbar: "&eТехработы через &f{time}".to_owned(),
            maintenance_subtitle: "&eдо технических работ".to_owned(),
            restart_start_message: "&6[Сервер] &eПерезапуск через &f{time}&e.{reason}".to_owned(),
            restart_warn_message: "&6[Сервер] &eПерезапуск через &f{time}&e.{reason}".to_owned(),
            restart_cancel_message: "&6[Сервер] &aПерезапуск отменён.".to_owned(),
            restart_reason_line: " &7Причина: &f{reason}".to_owned(),
            restart_bossbar: "&eПерезапуск сервера через &f{time}".to_owned(),
            restart_titles: true,
            restart_title: "&c{time}".to_owned(),
            restart_subtitle: "&eдо перезапуска сервера".to_owned(),
            restart_kick_message: "&6&lСервер перезапускается\n\n&fЗаходи снова через минуту.{reason}".to_owned(),
        }
    }
}

impl DowntimeConfig {
    /// Reads the file; a broken one falls back to the defaults.
    #[must_use]
    pub fn load() -> Self {
        config::load(&config::path("downtime")).unwrap_or_else(|error| {
            log::warn!("Bad downtime config, using the defaults: {error}");
            Self::default()
        })
    }
}
