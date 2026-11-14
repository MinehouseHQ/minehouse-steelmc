//! `config/minehouse/bans.toml`, the settings of the Fabric `minehouse-bans.properties`.

use serde::{Deserialize, Serialize};

use crate::config;

/// Announcements of bans and warns, and the warn limit.
///
/// Formats take `&` colour codes (`&0`-`&9`, `&a`-`&f`, `&l` bold, `&o` italic, `&n`
/// underlined, `&m` strikethrough, `&r` reset) and the placeholders `{player}`, `{reason}`,
/// `{by}`; bans also `{duration}` and `{until}` (Moscow time), warns `{count}` and `{limit}`.
/// An empty format announces nothing. The file is read again before every use.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BansConfig {
    /// `false` turns every announcement off.
    pub enabled: bool,
    /// Also announce the bans made on the site or through the Discord bot.
    pub site: bool,
    /// How often to ask the site who of the online players got banned elsewhere, and for the
    /// bans to announce. `0` turns both off.
    pub sync_seconds: u64,
    /// Reason of a ban or a warn made without one.
    pub default_reason: String,
    /// A permanent ban.
    pub ban: String,
    /// A temporary ban.
    pub tempban: String,
    /// What `{reason}` shows when the ban has none.
    pub no_reason: String,
    /// What `{by}` shows when the ban has no author.
    pub no_by: String,
    /// After how many warns the account is banned. `0` never bans.
    pub warn_limit: u32,
    /// How long that ban lasts: `30m`, `12h`, `7d`, `2w`, `1mo`, `1d12h`. Empty is forever.
    pub warn_ban_duration: String,
    /// Reason of that ban; `{count}` is the number of warns, `{reason}` the last one's.
    pub warn_ban_reason: String,
    /// A warn, to everyone.
    pub warn: String,
    /// A warn, to the player if they are online.
    pub warn_player: String,
    /// A removed warn, to everyone.
    pub unwarn: String,
}

impl Default for BansConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            site: true,
            sync_seconds: 30,
            default_reason: "Нарушение правил сервера".to_owned(),
            ban: "&c⛔ &f{player} &cзаблокирован навсегда. &7Причина: &f{reason}".to_owned(),
            tempban: "&c⛔ &f{player} &cзаблокирован на {duration}. &7Причина: &f{reason}"
                .to_owned(),
            no_reason: "не указана".to_owned(),
            no_by: "администрация".to_owned(),
            warn_limit: 3,
            warn_ban_duration: String::new(),
            warn_ban_reason: "Предупреждений: {count}. Последнее: {reason}".to_owned(),
            warn: "&6⚠ &f{player} &6получил предупреждение ({count}/{limit}). &7Причина: &f{reason}"
                .to_owned(),
            warn_player: "&6⚠ Тебе выдано предупреждение ({count}/{limit}): &f{reason}&6. При {limit} предупреждениях — бан."
                .to_owned(),
            unwarn: String::new(),
        }
    }
}

impl BansConfig {
    /// Reads the file; a broken one is logged and replaced by the defaults for this use.
    #[must_use]
    pub fn load() -> Self {
        config::load(&config::path("bans")).unwrap_or_else(|error| {
            log::warn!("Bad bans config, using the defaults: {error}");
            Self::default()
        })
    }
}
