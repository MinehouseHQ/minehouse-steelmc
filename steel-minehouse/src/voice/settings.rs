//! `config/minehouse/voice.toml`.

use std::fs;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config;

/// The settings of voice chat. The names follow `plasmovoice/server/config.toml` of the
/// Fabric server.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VoiceConfig {
    /// `false`: no voice chat, the UDP port stays closed.
    pub enabled: bool,
    /// Clients keep their volumes per server id. Empty: made once and kept in the data
    /// directory.
    pub server_id: String,
    /// The address the UDP socket listens on.
    pub host: String,
    pub port: u16,
    /// The address clients send sound to. Empty: the address they joined the game with.
    pub public_host: String,
    /// 0: `port`.
    pub public_port: u16,
    pub sample_rate: i32,
    pub mtu_size: i32,
    /// A client silent for this long is out of voice chat.
    pub keep_alive_timeout_ms: u64,
    /// How far a voice may carry, in blocks: the choices of the client.
    pub distances: Vec<i32>,
    pub default_distance: i32,
    /// Sound goes this much further than the chosen distance (at most twice as far), so a
    /// voice fades out rather than cuts off.
    pub max_extra_audio_broadcast_distance: i32,
    /// `VOIP`, `AUDIO` or `RESTRICTED_LOWDELAY`.
    pub opus_mode: String,
    /// Bits per second; -1000 lets opus choose.
    pub opus_bitrate: i32,
    /// When the icon above a head is hidden: `HIDE_NOT_INSTALLED`, `HIDE_VOICE_CHAT_DISABLED`,
    /// `HIDE_SERVER_MUTED`, `HIDE_CLIENT_MUTED`, `HIDE_SOURCE_ICON`.
    pub player_icon_visibility: Vec<String>,
    pub player_icon_y_offset: f64,
    /// Older client mods are not let in.
    pub client_mod_min_version: String,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server_id: String::new(),
            host: "0.0.0.0".to_owned(),
            port: 24454,
            public_host: String::new(),
            public_port: 0,
            sample_rate: 48000,
            mtu_size: 1024,
            keep_alive_timeout_ms: 15000,
            distances: vec![8, 16, 32],
            default_distance: 16,
            max_extra_audio_broadcast_distance: 16,
            opus_mode: "VOIP".to_owned(),
            opus_bitrate: -1000,
            player_icon_visibility: Vec::new(),
            player_icon_y_offset: 0.0,
            client_mod_min_version: "2.0.0".to_owned(),
        }
    }
}

impl VoiceConfig {
    /// The distance a voice carries when the client asks for `distance`: one of the
    /// choices, or the default.
    #[must_use]
    pub fn allowed_distance(&self, distance: i16) -> i16 {
        let distance = i32::from(distance);
        let allowed = match self.distances.as_slice() {
            [] => true,
            [-1, max] => (1..=*max).contains(&distance),
            distances => distances.contains(&distance),
        };
        let distance = if allowed {
            distance
        } else {
            self.default_distance
        };
        i16::try_from(distance).unwrap_or(i16::MAX)
    }

    /// How far from the speaker a listener still gets the sound.
    #[must_use]
    pub fn listeners_distance(&self, distance: i16) -> f64 {
        let distance = i32::from(distance).max(0);
        f64::from(
            distance
                .saturating_add(self.max_extra_audio_broadcast_distance)
                .min(distance.saturating_mul(2)),
        )
    }

    /// Whether a client mod of this version may join: Plasmo Voice 2, not older than
    /// `client_mod_min_version`.
    #[must_use]
    pub fn accepts_version(&self, version: &str) -> bool {
        let Some(client) = parse_version(version) else {
            return false;
        };
        let min = parse_version(&self.client_mod_min_version).unwrap_or([2, 0, 0]);
        client[0] == 2 && client >= min
    }

    /// The id of this voice server: from the config, or made once and kept.
    #[must_use]
    pub fn resolve_server_id(&self) -> Uuid {
        if let Ok(id) = Uuid::parse_str(self.server_id.trim()) {
            return id;
        }
        let path = config::data_path("voice-server-id");
        if let Some(id) = fs::read_to_string(&path)
            .ok()
            .and_then(|text| Uuid::parse_str(text.trim()).ok())
        {
            return id;
        }
        let id = Uuid::new_v4();
        let written = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&path, id.to_string()));
        if let Err(error) = written {
            log::warn!(
                "Could not keep the voice server id in {}: {error}",
                path.display()
            );
        }
        id
    }
}

/// `2.1.17` or `2.2.0-beta.3+fabric` as numbers.
fn parse_version(version: &str) -> Option<[u32; 3]> {
    let core = version.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u32>().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_listed_distances_are_allowed() {
        let config = VoiceConfig::default();
        assert_eq!(config.allowed_distance(8), 8);
        assert_eq!(config.allowed_distance(32), 32);
        assert_eq!(config.allowed_distance(500), 16);
        assert_eq!(config.allowed_distance(-3), 16);
        let range = VoiceConfig {
            distances: vec![-1, 64],
            default_distance: 20,
            ..VoiceConfig::default()
        };
        assert_eq!(range.allowed_distance(64), 64);
        assert_eq!(range.allowed_distance(65), 20);
        assert_eq!(range.allowed_distance(0), 20);
    }

    #[test]
    fn sound_carries_a_little_further_than_the_voice() {
        let config = VoiceConfig::default();
        assert!((config.listeners_distance(8) - 16.0).abs() < f64::EPSILON);
        assert!((config.listeners_distance(16) - 32.0).abs() < f64::EPSILON);
        assert!((config.listeners_distance(32) - 48.0).abs() < f64::EPSILON);
    }

    #[test]
    fn old_and_foreign_client_versions_are_refused() {
        let config = VoiceConfig {
            client_mod_min_version: "2.1.0".to_owned(),
            ..VoiceConfig::default()
        };
        assert!(config.accepts_version("2.1.17"));
        assert!(config.accepts_version("2.2.0-beta.3"));
        assert!(config.accepts_version("2.1.0+fabric"));
        assert!(!config.accepts_version("2.0.9"));
        assert!(!config.accepts_version("3.0.0"));
        assert!(!config.accepts_version("1.2.2"));
        assert!(!config.accepts_version("two"));
    }
}
