//! `MineHouse` Nitro: a subscription bought on the site for `MineHouse` Coins, just to support
//! the server. It colours the nickname in the tab list and in chat. On the Fabric server this
//! is `minehouse-auth` (the poll and `%minehouse:name%`) and `minehouse-chat`.
//!
//! The site answers `POST /api/mc/nitro` `{server}` with every nickname that has Nitro now and
//! its colour; with the feature flag `nitro` off for this server the list is empty. If the site
//! does not answer, the last list stays.

use std::{sync::LazyLock, time::Duration};

use rustc_hash::FxHashMap;
use serde_json::{Value, json};
use steel_utils::locks::SyncRwLock;
use tokio::time::sleep;
use uuid::Uuid;

use crate::site::{SiteClient, SiteError};

/// How often to ask the site.
const POLL: Duration = Duration::from_secs(30);

/// Nickname colours of the players with Nitro, `[r, g, b]`.
static COLORS: LazyLock<SyncRwLock<FxHashMap<Uuid, [u8; 3]>>> =
    LazyLock::new(|| SyncRwLock::new(FxHashMap::default()));

/// The Nitro colour of this player's nickname, if they have Nitro.
#[must_use]
pub fn color(player: Uuid) -> Option<[u8; 3]> {
    COLORS.read().get(&player).copied()
}

/// Starts polling the site. Runs until the process ends.
pub fn start(site: SiteClient, server_id: String) {
    tokio::spawn(async move {
        loop {
            match site.nitro(&server_id).await {
                Ok(colors) => {
                    let changed = *COLORS.read() != colors;
                    if changed {
                        log::info!("MineHouse Nitro: {} nickname(s) coloured", colors.len());
                        *COLORS.write() = colors;
                    }
                }
                Err(error) => log::debug!("Nitro poll failed: {error}"),
            }
            sleep(POLL).await;
        }
    });
}

impl SiteClient {
    /// Players with Nitro on this server and their colours.
    pub async fn nitro(&self, server: &str) -> Result<FxHashMap<Uuid, [u8; 3]>, SiteError> {
        let body = json!({ "server": server });
        parse(&self.post_json("/api/mc/nitro", body.to_string()).await?)
    }
}

/// Parses `POST /api/mc/nitro`; entries with a bad UUID or colour are skipped.
fn parse(text: &str) -> Result<FxHashMap<Uuid, [u8; 3]>, SiteError> {
    let json: Value = serde_json::from_str(text)
        .map_err(|error| SiteError::Answer(format!("invalid JSON: {error}")))?;
    Ok(json
        .get("players")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|player| {
            let uuid = Uuid::parse_str(player.get("uuid")?.as_str()?).ok()?;
            Some((uuid, parse_hex(player.get("color")?.as_str()?)?))
        })
        .collect())
}

/// `#rrggbb` → `[r, g, b]`.
#[must_use]
pub fn parse_hex(color: &str) -> Option<[u8; 3]> {
    let hex = color.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let [_, red, green, blue] = u32::from_str_radix(hex, 16).ok()?.to_be_bytes();
    Some([red, green, blue])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colours_and_skips_bad_entries() {
        let colors = parse(
            r##"{"players": [
                {"uuid": "0f8fad5b-d9cb-469f-a165-70867728950e", "nickname": "Steve", "color": "#a855f7"},
                {"uuid": "not-a-uuid", "color": "#ffffff"},
                {"uuid": "1f8fad5b-d9cb-469f-a165-70867728950e", "color": "purple"},
                "junk"
            ]}"##,
        )
        .expect("valid answer");
        let steve = Uuid::from_u128(0x0f8f_ad5b_d9cb_469f_a165_7086_7728_950e);
        assert_eq!(colors.len(), 1);
        assert_eq!(colors.get(&steve), Some(&[0xa8, 0x55, 0xf7]));
        assert!(parse("{}").is_ok_and(|colors| colors.is_empty()));
        assert!(parse("nope").is_err());
    }
}
