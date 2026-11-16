//! Player statistics for the site: a port of the Fabric mod `MineHouse` Stats. The site
//! shows them on the public profile (`/player/<nick>`) and in the list of players.
//!
//! A player is sent when they join, when they leave, and every few minutes while they
//! play, to `POST /api/mc/stats` with the same signature as the login check. Statistics
//! and advancements are read from memory, so nothing waits for a save. If the site is
//! down a warning is logged and the data goes out the next time.

use std::{
    collections::BTreeMap,
    mem,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, SecondsFormat, Utc};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use steel_core::{player::Player, server::Server};
use steel_registry::{ProfileProperty, REGISTRY, advancement::Advancement};
use steel_utils::locks::SyncMutex;
use tokio::{
    task::JoinHandle,
    time::{sleep, timeout},
};
use uuid::Uuid;

use crate::{config, site::SiteClient};

/// How often to look for players who joined or left.
const JOIN_CHECK: Duration = Duration::from_secs(3);
/// How old what is sent about a player who left may be.
const REFRESH: Duration = Duration::from_secs(60);
/// The site takes this many players in one request, and about this many bytes.
const MAX_PLAYERS: usize = 50;
const MAX_BYTES: usize = 2_000_000;
/// How long the last upload may hold up a shutdown.
const LAST_UPLOAD: Duration = Duration::from_secs(12);

/// `config/minehouse/stats.toml`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct StatsConfig {
    /// Turns the upload off.
    pub enabled: bool,
    /// How often the players online are sent, seconds (30 at least).
    pub interval_seconds: u64,
}

impl Default for StatsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: 300,
        }
    }
}

/// The running upload of one server.
pub struct Stats {
    server: Arc<Server>,
    site: SiteClient,
    known: Known,
    task: SyncMutex<Option<JoinHandle<()>>>,
}

/// What was last read of everyone online: sent as their last word when they leave.
type Known = Arc<SyncMutex<FxHashMap<Uuid, Value>>>;

impl Stats {
    /// Starts the upload, unless the config turns it off.
    #[must_use]
    pub fn start(server: &Arc<Server>, site: SiteClient) -> Option<Self> {
        let config: StatsConfig = config::load(&config::path("stats")).unwrap_or_else(|error| {
            log::warn!("Bad stats config, statistics are not sent: {error}");
            StatsConfig {
                enabled: false,
                ..StatsConfig::default()
            }
        });
        if !config.enabled {
            log::info!("Player statistics are not sent: off in config/minehouse/stats.toml");
            return None;
        }
        let every = Duration::from_secs(config.interval_seconds.max(30));
        let known = Known::default();
        let task = tokio::spawn(run(
            Arc::clone(server),
            site.clone(),
            every,
            Arc::clone(&known),
        ));
        Some(Self {
            server: Arc::clone(server),
            site,
            known,
            task: SyncMutex::new(Some(task)),
        })
    }

    /// Stops the upload and sends everyone one last time, as offline. The players may
    /// already be gone from the server by now, so those are sent as they were last read.
    pub async fn stop(&self) {
        let task = self.task.lock().take();
        if let Some(task) = task {
            task.abort();
        }
        let mut last = mem::take(&mut *self.known.lock());
        for player in &self.server.get_players() {
            last.insert(player.gameprofile.id(), entry(player, false));
        }
        let players: Vec<Value> = last
            .into_values()
            .map(|mut player| {
                player["online"] = Value::Bool(false);
                player
            })
            .collect();
        if timeout(LAST_UPLOAD, upload(&self.site, players))
            .await
            .is_err()
        {
            log::warn!("Player statistics: the last upload took too long");
        }
    }
}

async fn run(server: Arc<Server>, site: SiteClient, every: Duration, known: Known) {
    let mut last_full = Instant::now();
    let mut last_refresh = Instant::now();

    loop {
        sleep(JOIN_CHECK).await;
        let full = last_full.elapsed() >= every;
        let refresh = full || last_refresh.elapsed() >= REFRESH;
        if full {
            last_full = Instant::now();
        }
        if refresh {
            last_refresh = Instant::now();
        }

        let batch = changes(&server, &known, full, refresh);
        upload(&site, batch).await;
    }
}

/// Who to send now: players who left (as they were last read), players who joined, and
/// everyone online when it is time for a full upload.
fn changes(server: &Server, known: &Known, full: bool, refresh: bool) -> Vec<Value> {
    let online = server.get_players();
    let mut batch = Vec::new();
    let mut known_now = known.lock();
    let here: Vec<Uuid> = online
        .iter()
        .map(|player| player.gameprofile.id())
        .collect();
    let left: Vec<Uuid> = known_now
        .keys()
        .filter(|id| !here.contains(id))
        .copied()
        .collect();
    for id in left {
        if let Some(mut last) = known_now.remove(&id) {
            last["online"] = Value::Bool(false);
            batch.push(last);
        }
    }
    for player in &online {
        let id = player.gameprofile.id();
        let joined = !known_now.contains_key(&id);
        if joined || refresh {
            let entry = entry(player, true);
            if joined || full {
                batch.push(entry.clone());
            }
            known_now.insert(id, entry);
        }
    }
    batch
}

/// Sends the players in as many requests as the site's limits ask for.
async fn upload(site: &SiteClient, players: Vec<Value>) {
    if players.is_empty() {
        return;
    }
    let count = players.len();
    for body in bodies(players, counted_total()) {
        if let Err(error) = site.post_json("/api/mc/stats", body).await {
            log::warn!("Could not send the statistics of {count} players: {error}");
            return;
        }
    }
}

/// Splits the players into request bodies of at most 50 players and about 2 MB.
fn bodies(players: Vec<Value>, advancements_total: usize) -> Vec<String> {
    let body = |batch: Vec<Value>| {
        json!({"advancements_total": advancements_total, "players": batch}).to_string()
    };
    let mut bodies = Vec::new();
    let mut batch = Vec::new();
    let mut size = 0;
    for player in players {
        let length = player.to_string().len();
        if !batch.is_empty() && (batch.len() >= MAX_PLAYERS || size + length > MAX_BYTES) {
            bodies.push(body(mem::take(&mut batch)));
            size = 0;
        }
        batch.push(player);
        size += length;
    }
    if !batch.is_empty() {
        bodies.push(body(batch));
    }
    bodies
}

/// Whether the site counts an advancement: the player can see it in the game, so it has a
/// display and is neither a recipe nor the root of a tab.
fn counted(advancement: &Advancement) -> bool {
    let id = advancement.key().to_string();
    advancement.display().is_some() && !id.contains(":recipes/") && !id.ends_with("/root")
}

fn counted_total() -> usize {
    REGISTRY
        .advancements
        .advancements()
        .iter()
        .filter(|advancement| counted(advancement))
        .count()
}

/// One player of an upload: `{uuid, nickname, skin, last_seen, online, stats, advancements}`.
fn entry(player: &Player, online: bool) -> Value {
    let profile = &player.gameprofile;
    let textures = profile
        .properties()
        .iter()
        .find(|property| property.name() == "textures")
        .map(ProfileProperty::value);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    let mut stats: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for (stat, count) in player.stats() {
        if count > 0 {
            stats
                .entry(stat.stat_type_key().to_string())
                .or_default()
                .insert(stat.stat_value_key().to_string(), Value::from(count));
        }
    }

    let mut advancements = Map::new();
    for (advancement, progress) in player.advancements.lock().progress().iter() {
        if !counted(advancement) || !progress.is_done() {
            continue;
        }
        // When the last criterion was met.
        let last = progress
            .criteria()
            .values()
            .filter_map(|criterion| criterion.0)
            .max()
            .unwrap_or(UNIX_EPOCH);
        advancements.insert(advancement.key().to_string(), Value::from(iso(last)));
    }

    json!({
        "uuid": profile.id().to_string(),
        "nickname": profile.name(),
        "skin": textures.and_then(skin_hash),
        "last_seen": u64::try_from(now).unwrap_or(u64::MAX),
        "online": online,
        "stats": stats,
        "advancements": advancements,
    })
}

/// A time the way Java's `Instant.toString` writes it.
fn iso(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

/// The texture hash from the `textures` profile property (base64 JSON from Mojang), or
/// nothing when the player has no skin.
fn skin_hash(textures: &str) -> Option<String> {
    let bytes = STANDARD.decode(textures.trim()).ok()?;
    let root: Value = serde_json::from_slice(&bytes).ok()?;
    let url = root.get("textures")?.get("SKIN")?.get("url")?.as_str()?;
    let hash = url.rsplit('/').next()?;
    let fine = (32..=64).contains(&hash.len())
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    fine.then(|| hash.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_skin_hash_is_the_end_of_the_texture_address() {
        let hash = "1a4af718455d4aab528e7a61f86fa25e6a369d1768dcb13f7df319a713eb810b";
        let property = STANDARD.encode(
            json!({"textures": {"SKIN": {"url": format!("http://textures.minecraft.net/texture/{hash}")}}})
                .to_string(),
        );
        assert_eq!(skin_hash(&property).as_deref(), Some(hash));
        assert_eq!(skin_hash(&STANDARD.encode(r#"{"textures": {}}"#)), None);
        assert_eq!(
            skin_hash(&STANDARD.encode(r#"{"textures": {"SKIN": {"url": "http://a/b/NOPE"}}}"#)),
            None
        );
        assert_eq!(skin_hash("not base64"), None);
    }

    #[test]
    fn uploads_are_split_at_fifty_players() {
        let players: Vec<Value> = (0..120).map(|index| json!({"uuid": index})).collect();
        let bodies = bodies(players, 121);
        let sizes: Vec<usize> = bodies
            .iter()
            .map(|body| {
                let value: Value = serde_json::from_str(body).expect("json");
                assert_eq!(value["advancements_total"], 121);
                value["players"].as_array().map_or(0, Vec::len)
            })
            .collect();
        assert_eq!(sizes, [50, 50, 20]);
        assert_eq!(super::bodies(Vec::new(), 1).len(), 0);
    }

    #[test]
    fn times_are_written_like_java_instants() {
        let time = UNIX_EPOCH + Duration::from_secs(1_793_401_383);
        assert_eq!(iso(time), "2026-10-30T23:03:03Z");
        assert_eq!(iso(UNIX_EPOCH), "1970-01-01T00:00:00Z");
    }
}
