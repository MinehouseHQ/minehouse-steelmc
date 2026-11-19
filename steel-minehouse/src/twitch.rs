//! Twitch streams of our players (the Fabric `TwitchLive` of `minehouse-auth`).
//!
//! The site knows who linked a channel and who is live. This asks it with
//! `GET /api/mc/twitch`, announces new streams in chat, reminds of the running ones, answers
//! `/streams` and keeps the set of live players for the tab list ([`marker`]) and the label
//! over the head ([`crate::content`]). The texts are in `config/minehouse/twitch.toml`.

use std::{
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use steel_core::{
    command::{
        CommandContext, CommandError, CommandNode, CommandRegistration, CommandRegistrationError,
        CommandRegistry, literal,
    },
    server::Server,
};
use steel_utils::{Identifier, locks::SyncRwLock};
use text_components::{
    Modifier as _, TextComponent,
    interactivity::{ClickEvent, HoverEvent},
};
use tokio::time::sleep;
use uuid::Uuid;

use crate::{
    config,
    moderation::format::render,
    site::{SiteClient, SiteError},
};

/// The config module: `config/minehouse/twitch.toml`.
const CONFIG: &str = "twitch";

/// `config/minehouse/twitch.toml`. Texts take `&` color codes and `{player}`, `{channel}`,
/// `{title}`, `{game}`, `{url}`; an empty text is not sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TwitchConfig {
    /// Whether streams are asked for at all.
    pub enabled: bool,
    /// How often the site is asked, seconds.
    pub poll_seconds: u64,
    /// The chat line about a stream that began.
    pub announce: String,
    /// What stands for the title of a stream without one.
    pub no_title: String,
    /// The hover of a clickable line.
    pub hover: String,
    /// What goes before the nickname of a live player in the tab list.
    pub tab: String,
    /// The label over the head of a live player.
    pub overhead: String,
    /// After a restart, streams that began within this many minutes are announced.
    pub announce_after_restart_minutes: i64,
    /// The first line of the list of streams.
    pub list_header: String,
    /// One stream in the list.
    pub list_line: String,
    /// The list when nobody streams.
    pub list_empty: String,
    /// Every how many minutes the list goes to chat while somebody streams; 0 never.
    pub remind_minutes: u64,
}

impl Default for TwitchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_seconds: 20,
            announce: "&5▶ &d{player} &fв эфире на Twitch: &7{title} &8(нажми, чтобы открыть)"
                .to_owned(),
            no_title: "без названия".to_owned(),
            hover: "Открыть {url}".to_owned(),
            tab: "&c&lLIVE&r ".to_owned(),
            overhead: "&c● &f&lLIVE".to_owned(),
            announce_after_restart_minutes: 5,
            list_header: "&5▶ &fСейчас в эфире на Twitch:".to_owned(),
            list_line: "&8 • &d{player} &7— {title} &8(нажми, чтобы открыть)".to_owned(),
            list_empty: "&7Сейчас никто из игроков не стримит. Свой канал можно привязать на \
                         сайте в личном кабинете."
                .to_owned(),
            remind_minutes: 30,
        }
    }
}

/// One live stream, as the site tells it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stream {
    id: String,
    started_at: Option<String>,
    login: String,
    name: Option<String>,
    title: Option<String>,
    game: Option<String>,
    url: Option<String>,
    /// The player's main nickname on the server.
    nickname: Option<String>,
    /// Every nickname of the account.
    uuids: Vec<Uuid>,
}

/// What the last poll found.
#[derive(Default)]
struct Live {
    config: Option<TwitchConfig>,
    streams: Vec<Stream>,
    players: FxHashSet<Uuid>,
}

static LIVE: LazyLock<SyncRwLock<Live>> = LazyLock::new(|| SyncRwLock::new(Live::default()));

/// Whether this player streams on Twitch now.
#[must_use]
pub fn is_live(player: Uuid) -> bool {
    LIVE.read().players.contains(&player)
}

/// What goes before the nickname of a live player in the tab list, if anything.
#[must_use]
pub fn marker(player: Uuid) -> Option<TextComponent> {
    let live = LIVE.read();
    let tab = &live.config.as_ref()?.tab;
    (live.players.contains(&player) && !tab.trim().is_empty()).then(|| render(tab, &[]))
}

/// The label over the head of a live player, if there is one to show.
#[must_use]
pub fn overhead() -> Option<TextComponent> {
    let live = LIVE.read();
    let text = &live.config.as_ref()?.overhead;
    (!text.trim().is_empty()).then(|| render(text, &[]))
}

impl SiteClient {
    /// Players whose Twitch stream is live right now.
    async fn twitch(&self) -> Result<Vec<Stream>, SiteError> {
        parse(&self.get("/api/mc/twitch").await?)
    }
}

/// Parses `GET /api/mc/twitch`; streams without an id or a channel are skipped.
fn parse(text: &str) -> Result<Vec<Stream>, SiteError> {
    let json: Value = serde_json::from_str(text)
        .map_err(|error| SiteError::Answer(format!("invalid JSON: {error}")))?;
    let string =
        |stream: &Value, key: &str| stream.get(key).and_then(Value::as_str).map(str::to_owned);
    Ok(json
        .get("streams")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|stream| {
            Some(Stream {
                id: string(stream, "stream_id")?,
                login: string(stream, "login")?,
                started_at: string(stream, "started_at"),
                name: string(stream, "name"),
                title: string(stream, "title"),
                game: string(stream, "game"),
                url: string(stream, "url"),
                nickname: string(stream, "nickname"),
                uuids: stream
                    .get("uuids")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|uuid| Uuid::parse_str(uuid.as_str()?).ok())
                    .collect(),
            })
        })
        .collect())
}

/// Whether a stream that began at `started_at` (RFC 3339) began within `minutes` of `now`.
fn started_within(started_at: Option<&str>, minutes: i64, now: DateTime<Utc>) -> bool {
    started_at
        .and_then(|started| DateTime::parse_from_rfc3339(started).ok())
        .is_some_and(|started| (now - started.with_timezone(&Utc)).num_minutes() < minutes)
}

/// `format` about `stream`, clickable where the stream has a link that can be opened.
fn line(config: &TwitchConfig, format: &str, stream: &Stream) -> TextComponent {
    let url = stream
        .url
        .clone()
        .unwrap_or_else(|| format!("https://twitch.tv/{}", stream.login));
    let title = stream
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or(&config.no_title);
    let values = [
        (
            "player",
            stream.nickname.as_deref().unwrap_or(&stream.login),
        ),
        ("channel", stream.name.as_deref().unwrap_or(&stream.login)),
        ("title", title),
        ("game", stream.game.as_deref().unwrap_or_default()),
        ("url", url.as_str()),
    ];
    let text = render(format, &values);
    if !url.starts_with("https://") {
        return text;
    }
    let text = text.click_event(ClickEvent::open_url(url.clone()));
    if config.hover.trim().is_empty() {
        text
    } else {
        text.hover_event(HoverEvent::show_text(render(&config.hover, &values)))
    }
}

/// The list of streams, for `/streams` and the reminder.
fn list(config: &TwitchConfig, streams: &[Stream]) -> TextComponent {
    if streams.is_empty() {
        return render(&config.list_empty, &[]);
    }
    let mut children = vec![render(&config.list_header, &[])];
    for stream in streams {
        children.push(TextComponent::plain("\n"));
        children.push(line(config, &config.list_line, stream));
    }
    TextComponent::plain("").add_children(children)
}

fn broadcast(server: &Server, message: &TextComponent) {
    for player in server.get_players() {
        player.send_message(message);
    }
}

fn load() -> TwitchConfig {
    config::load(&config::path(CONFIG)).unwrap_or_else(|error| {
        log::warn!("MineHouse Twitch: {error}, streams are not shown");
        TwitchConfig {
            enabled: false,
            ..TwitchConfig::default()
        }
    })
}

async fn run(server: Arc<Server>, site: SiteClient) {
    // Stream ids of the last poll; `None` until the first one that worked.
    let mut known: Option<FxHashSet<String>> = None;
    let mut reminded = Instant::now();
    loop {
        // Read again every time, so the texts change without a restart.
        let config = load();
        let every = Duration::from_secs(config.poll_seconds.max(5));
        if !config.enabled {
            known = None;
            *LIVE.write() = Live::default();
            sleep(every).await;
            continue;
        }
        let streams = match site.twitch().await {
            Ok(streams) => streams,
            Err(error) => {
                log::debug!("Twitch poll failed: {error}");
                sleep(every).await;
                continue;
            }
        };
        let now = Utc::now();
        for stream in &streams {
            let new = known.as_ref().map_or_else(
                || {
                    started_within(
                        stream.started_at.as_deref(),
                        config.announce_after_restart_minutes,
                        now,
                    )
                },
                |known| !known.contains(&stream.id),
            );
            if new {
                reminded = Instant::now();
                log::info!("Twitch stream of {} is live", stream.login);
                if !config.announce.trim().is_empty() {
                    broadcast(&server, &line(&config, &config.announce, stream));
                }
            }
        }
        if config.remind_minutes > 0
            && !streams.is_empty()
            && reminded.elapsed() >= Duration::from_secs(config.remind_minutes * 60)
        {
            reminded = Instant::now();
            broadcast(&server, &list(&config, &streams));
        }
        known = Some(streams.iter().map(|stream| stream.id.clone()).collect());
        *LIVE.write() = Live {
            players: streams
                .iter()
                .flat_map(|stream| stream.uuids.iter().copied())
                .collect(),
            streams,
            config: Some(config),
        };
        sleep(every).await;
    }
}

/// Starts asking the site. Runs until the process ends.
pub fn start(server: &Arc<Server>, site: SiteClient) {
    tokio::spawn(run(Arc::clone(server), site));
}

fn streams_command(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let live = LIVE.read();
    let Some(config) = &live.config else {
        return Err(CommandError::from("Список стримов сейчас недоступен."));
    };
    context
        .source()
        .send_success(&list(config, &live.streams), false);
    Ok(1)
}

fn tree() -> CommandNode {
    literal("streams").executes(|context| streams_command(*context))
}

/// Registers `/streams`.
pub fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "streams"), tree).default_access(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use text_components::resolving::NoResolutor;

    use super::*;

    const ANSWER: &str = r#"{"streams": [
        {"stream_id": "41", "started_at": "2026-11-19T15:00:00Z", "login": "steve_tv",
         "name": "Steve TV", "title": " Строим метро ", "game": "Minecraft",
         "url": "https://twitch.tv/steve_tv", "nickname": "Steve",
         "uuids": ["0f8fad5b-d9cb-469f-a165-70867728950e", "junk"]},
        {"stream_id": "42", "login": "alex"},
        {"login": "no_id"},
        "junk"
    ]}"#;

    #[test]
    fn parses_streams_and_skips_bad_entries() {
        let streams = parse(ANSWER).expect("valid answer");
        assert_eq!(streams.len(), 2);
        assert_eq!(streams[0].nickname.as_deref(), Some("Steve"));
        assert_eq!(
            streams[0].uuids,
            vec![Uuid::from_u128(0x0f8f_ad5b_d9cb_469f_a165_7086_7728_950e)]
        );
        assert_eq!(streams[1].uuids, Vec::<Uuid>::new());
        assert!(parse("{}").is_ok_and(|streams| streams.is_empty()));
        assert!(parse("nope").is_err());
    }

    #[test]
    fn lines_name_the_player_and_the_title() {
        let config = TwitchConfig::default();
        let streams = parse(ANSWER).expect("valid answer");
        assert_eq!(
            line(&config, &config.announce, &streams[0]).to_plain(&NoResolutor),
            "▶ Steve в эфире на Twitch: Строим метро (нажми, чтобы открыть)"
        );
        // No nickname and no title: the channel and the stand-in.
        assert_eq!(
            list(&config, &streams[1..]).to_plain(&NoResolutor),
            "▶ Сейчас в эфире на Twitch:\n • alex — без названия (нажми, чтобы открыть)"
        );
        assert_eq!(
            list(&config, &[]).to_plain(&NoResolutor),
            render(&config.list_empty, &[]).to_plain(&NoResolutor)
        );
    }

    #[test]
    fn only_fresh_streams_are_announced_after_a_restart() {
        let now = DateTime::parse_from_rfc3339("2026-11-19T15:04:00Z")
            .expect("a date")
            .with_timezone(&Utc);
        assert!(started_within(Some("2026-11-19T15:00:00Z"), 5, now));
        assert!(!started_within(Some("2026-11-19T14:00:00Z"), 5, now));
        assert!(!started_within(Some("yesterday"), 5, now));
        assert!(!started_within(None, 5, now));
    }
}
