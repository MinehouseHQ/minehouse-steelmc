//! Vanilla `CustomBossEvent` and `CustomBossEvents`: the boss bars `/bossbar`
//! creates, saved with the world.
//!
//! Vanilla keeps one set per server. Steel keeps one per domain, like its
//! scoreboards and command storage, and saves it through the domain's default
//! world.
//!
//! Vanilla marks its set changed only for players, value and max, so a bar
//! renamed, recolored, restyled or hidden is not saved unless one of those
//! changes too. Steel marks every `/bossbar` change, so none is lost.

use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use steel_protocol::packets::game::{BossBarColor, BossBarFlags, BossBarOverlay};
use steel_utils::locks::{AsyncMutex, SyncMutex};
use steel_utils::saved_data::names as saved_data_names;
use steel_utils::{Identifier, translations};
use text_components::format::Color;
use text_components::interactivity::HoverEvent;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use super::ServerBossEvent;
use crate::player::Player;
use crate::server::worlds::WorldMap;
use crate::world::World;

/// Vanilla `CustomBossEvent.DEFAULT_MAX`.
const DEFAULT_MAX: i32 = 100;

/// Vanilla `BossBarColor.getFormatting`.
const fn chat_color(color: BossBarColor) -> Color {
    match color {
        BossBarColor::Pink => Color::LightPurple,
        BossBarColor::Blue => Color::Blue,
        BossBarColor::Red => Color::Red,
        BossBarColor::Green => Color::Green,
        BossBarColor::Yellow => Color::Yellow,
        BossBarColor::Purple => Color::DarkPurple,
        BossBarColor::White => Color::White,
    }
}

/// Vanilla `BossBarColor.getName`, the saved and command name.
const fn color_name(color: BossBarColor) -> &'static str {
    match color {
        BossBarColor::Pink => "pink",
        BossBarColor::Blue => "blue",
        BossBarColor::Red => "red",
        BossBarColor::Green => "green",
        BossBarColor::Yellow => "yellow",
        BossBarColor::Purple => "purple",
        BossBarColor::White => "white",
    }
}

fn color_by_name(name: &str) -> Option<BossBarColor> {
    [
        BossBarColor::Pink,
        BossBarColor::Blue,
        BossBarColor::Red,
        BossBarColor::Green,
        BossBarColor::Yellow,
        BossBarColor::Purple,
        BossBarColor::White,
    ]
    .into_iter()
    .find(|color| color_name(*color) == name)
}

/// Vanilla `BossBarOverlay.getName`, the saved and command name.
const fn overlay_name(overlay: BossBarOverlay) -> &'static str {
    match overlay {
        BossBarOverlay::Progress => "progress",
        BossBarOverlay::Notched6 => "notched_6",
        BossBarOverlay::Notched10 => "notched_10",
        BossBarOverlay::Notched12 => "notched_12",
        BossBarOverlay::Notched20 => "notched_20",
    }
}

fn overlay_by_name(name: &str) -> Option<BossBarOverlay> {
    [
        BossBarOverlay::Progress,
        BossBarOverlay::Notched6,
        BossBarOverlay::Notched10,
        BossBarOverlay::Notched12,
        BossBarOverlay::Notched20,
    ]
    .into_iter()
    .find(|overlay| overlay_name(*overlay) == name)
}

/// Vanilla `(float) value / max` clamped by `Mth.clamp`, which keeps NaN
/// (a zero `max` from `/execute store`) the way Java's `Math.min` does,
/// as `f32::clamp` does.
fn progress(value: i32, max: i32) -> f32 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "vanilla divides the int value as a float"
    )]
    let progress = value as f32 / max as f32;
    progress.clamp(0.0, 1.0)
}

struct CustomBossEventState {
    /// Every player the bar is for, online or not.
    players: FxHashSet<Uuid>,
    value: i32,
    max: i32,
}

/// Vanilla `CustomBossEvent`.
pub struct CustomBossEvent {
    custom_id: Identifier,
    event: ServerBossEvent,
    state: SyncMutex<CustomBossEventState>,
    /// The owning set's change counter, vanilla's `dirtyCallback`.
    revision: Arc<AtomicU64>,
}

impl CustomBossEvent {
    fn new(id: Uuid, custom_id: Identifier, name: TextComponent, revision: Arc<AtomicU64>) -> Self {
        let event = ServerBossEvent::new(id, name, BossBarColor::White, BossBarOverlay::Progress);
        event.set_progress(0.0);
        Self {
            custom_id,
            event,
            state: SyncMutex::new(CustomBossEventState {
                players: FxHashSet::default(),
                value: 0,
                max: DEFAULT_MAX,
            }),
            revision,
        }
    }

    fn set_dirty(&self) {
        self.revision.fetch_add(1, Ordering::Release);
    }

    /// The `/bossbar` id.
    #[must_use]
    pub const fn custom_id(&self) -> &Identifier {
        &self.custom_id
    }

    /// The boss bar sent to players.
    #[must_use]
    pub const fn event(&self) -> &ServerBossEvent {
        &self.event
    }

    /// Vanilla `value`.
    #[must_use]
    pub fn value(&self) -> i32 {
        self.state.lock().value
    }

    /// Vanilla `max`.
    #[must_use]
    pub fn max(&self) -> i32 {
        self.state.lock().max
    }

    /// Vanilla `setValue`.
    pub fn set_value(&self, value: i32) {
        let mut state = self.state.lock();
        state.value = value;
        self.event.set_progress(progress(value, state.max));
        drop(state);
        self.set_dirty();
    }

    /// Vanilla `setMax`.
    pub fn set_max(&self, max: i32) {
        let mut state = self.state.lock();
        state.max = max;
        self.event.set_progress(progress(state.value, max));
        drop(state);
        self.set_dirty();
    }

    /// Vanilla `setName`.
    pub fn set_name(&self, name: TextComponent) {
        self.event.set_name(name);
        self.set_dirty();
    }

    /// Vanilla `setColor`.
    pub fn set_color(&self, color: BossBarColor) {
        self.event.set_color(color);
        self.set_dirty();
    }

    /// Vanilla `setOverlay`.
    pub fn set_overlay(&self, overlay: BossBarOverlay) {
        self.event.set_overlay(overlay);
        self.set_dirty();
    }

    /// Vanilla `setVisible`.
    pub fn set_visible(&self, visible: bool) {
        self.event.set_visible(visible);
        self.set_dirty();
    }

    /// Vanilla `getDisplayName`: the name in square brackets, in the bar's
    /// color, showing and inserting the id.
    #[must_use]
    pub fn display_name(&self) -> TextComponent {
        let id = self.custom_id.to_string();
        translations::CHAT_SQUARE_BRACKETS
            .message([self.event.name()])
            .component()
            .color(chat_color(self.event.color()))
            .hover_event(HoverEvent::show_text(TextComponent::plain(id.clone())))
            .insertion(id)
    }

    /// Vanilla `addPlayer`.
    pub fn add_player(&self, player: &Arc<Player>) {
        self.event.add_player(player);
        if self.state.lock().players.insert(player.gameprofile.id()) {
            self.set_dirty();
        }
    }

    /// Vanilla `removePlayer`.
    pub fn remove_player(&self, player: &Player) {
        self.event.remove_player(player);
        if self.state.lock().players.remove(&player.gameprofile.id()) {
            self.set_dirty();
        }
    }

    /// Vanilla `removeAllPlayers`.
    pub fn remove_all_players(&self) {
        self.event.remove_all_players();
        let mut state = self.state.lock();
        if !state.players.is_empty() {
            state.players.clear();
            drop(state);
            self.set_dirty();
        }
    }

    /// Vanilla `setPlayers`: shows the bar to exactly `players`, returning
    /// whether that changed anything.
    pub fn set_players(&self, players: &[Arc<Player>]) -> bool {
        let targets = players
            .iter()
            .map(|player| player.gameprofile.id())
            .collect::<FxHashSet<_>>();
        let (to_remove, to_add) = {
            let state = self.state.lock();
            let to_remove = state
                .players
                .iter()
                .filter(|uuid| !targets.contains(uuid))
                .copied()
                .collect::<Vec<_>>();
            let to_add = players
                .iter()
                .filter(|player| !state.players.contains(&player.gameprofile.id()))
                .cloned()
                .collect::<Vec<_>>();
            (to_remove, to_add)
        };

        for uuid in &to_remove {
            if let Some(player) = self
                .event
                .players()
                .into_iter()
                .find(|player| player.gameprofile.id() == *uuid)
            {
                self.remove_player(&player);
            }
            self.state.lock().players.remove(uuid);
        }
        for player in &to_add {
            self.add_player(player);
        }

        let changed = !to_remove.is_empty() || !to_add.is_empty();
        if changed {
            self.set_dirty();
        }
        changed
    }

    /// Vanilla `onPlayerConnect`: a player the bar is for sees it again.
    pub fn on_player_connect(&self, player: &Arc<Player>) {
        let tracked = self.state.lock().players.contains(&player.gameprofile.id());
        if tracked {
            self.add_player(player);
        }
    }

    /// Vanilla `onPlayerDisconnect`: the player stops seeing the bar but stays
    /// one of its players.
    pub fn on_player_disconnect(&self, player: &Player) {
        self.event.remove_player(player);
    }

    /// Vanilla `load`.
    fn load(
        custom_id: Identifier,
        packed: PersistentCustomBossEvent,
        revision: Arc<AtomicU64>,
    ) -> io::Result<Self> {
        let name = serde_json::from_str(&packed.name).map_err(|error| {
            invalid_data(format!("invalid name of boss bar '{custom_id}': {error}"))
        })?;
        let color = color_by_name(&packed.color).ok_or_else(|| {
            invalid_data(format!(
                "unknown color '{}' of boss bar '{custom_id}'",
                packed.color
            ))
        })?;
        let overlay = overlay_by_name(&packed.overlay).ok_or_else(|| {
            invalid_data(format!(
                "unknown style '{}' of boss bar '{custom_id}'",
                packed.overlay
            ))
        })?;
        // Vanilla gives a loaded bar a fresh random network id.
        let event = Self::new(Uuid::new_v4(), custom_id, name, revision);
        event.event.set_visible(packed.visible);
        {
            let mut state = event.state.lock();
            state.value = packed.value;
            state.max = packed.max;
            state.players.extend(packed.players);
        }
        event.event.set_progress(progress(packed.value, packed.max));
        event.event.set_color(color);
        event.event.set_overlay(overlay);
        event.event.set_flags(BossBarFlags {
            darken_screen: packed.darken_screen,
            play_boss_music: packed.play_boss_music,
            create_world_fog: packed.create_world_fog,
        });
        Ok(event)
    }

    /// Vanilla `pack`.
    fn pack(&self) -> io::Result<PersistentCustomBossEvent> {
        let name = serde_json::to_string(&self.event.name()).map_err(|error| {
            invalid_data(format!(
                "cannot serialize name of boss bar '{}': {error}",
                self.custom_id
            ))
        })?;
        let flags = self.event.flags();
        let state = self.state.lock();
        let mut players = state.players.iter().copied().collect::<Vec<_>>();
        players.sort_unstable();
        Ok(PersistentCustomBossEvent {
            name,
            visible: self.event.is_visible(),
            value: state.value,
            max: state.max,
            color: color_name(self.event.color()).to_owned(),
            overlay: overlay_name(self.event.overlay()).to_owned(),
            darken_screen: flags.darken_screen,
            play_boss_music: flags.play_boss_music,
            create_world_fog: flags.create_world_fog,
            players,
        })
    }
}

/// Vanilla `CustomBossEvent.Packed` with its codec defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PersistentCustomBossEvent {
    /// The name as JSON text.
    name: String,
    visible: bool,
    value: i32,
    max: i32,
    color: String,
    overlay: String,
    darken_screen: bool,
    play_boss_music: bool,
    create_world_fog: bool,
    players: Vec<Uuid>,
}

impl Default for PersistentCustomBossEvent {
    fn default() -> Self {
        Self {
            name: String::new(),
            visible: false,
            value: 0,
            max: DEFAULT_MAX,
            color: color_name(BossBarColor::White).to_owned(),
            overlay: overlay_name(BossBarOverlay::Progress).to_owned(),
            darken_screen: false,
            play_boss_music: false,
            create_world_fog: false,
            players: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PersistentCustomBossEvents {
    events: BTreeMap<String, PersistentCustomBossEvent>,
}

/// Vanilla `CustomBossEvents` for one Steel domain.
pub struct CustomBossEvents {
    events: SyncMutex<FxHashMap<Identifier, Arc<CustomBossEvent>>>,
    revision: Arc<AtomicU64>,
    saved_revision: AtomicU64,
}

impl Default for CustomBossEvents {
    fn default() -> Self {
        Self::new()
    }
}

impl CustomBossEvents {
    /// Creates an empty, saved set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: SyncMutex::new(FxHashMap::default()),
            revision: Arc::new(AtomicU64::new(0)),
            saved_revision: AtomicU64::new(0),
        }
    }

    fn set_dirty(&self) {
        self.revision.fetch_add(1, Ordering::Release);
    }

    fn from_persistent(persistent: PersistentCustomBossEvents) -> io::Result<Self> {
        let events = Self::new();
        {
            let mut map = events.events.lock();
            for (raw_id, packed) in persistent.events {
                let id = raw_id.parse::<Identifier>().map_err(|error| {
                    invalid_data(format!("invalid boss bar id '{raw_id}': {error}"))
                })?;
                let event =
                    CustomBossEvent::load(id.clone(), packed, Arc::clone(&events.revision))?;
                map.insert(id, Arc::new(event));
            }
        }
        // Loading replays the setters; what was just read is already saved.
        events
            .saved_revision
            .store(events.revision.load(Ordering::Acquire), Ordering::Release);
        Ok(events)
    }

    /// Vanilla `get`.
    #[must_use]
    pub fn get(&self, id: &Identifier) -> Option<Arc<CustomBossEvent>> {
        self.events.lock().get(id).cloned()
    }

    /// Vanilla `create`. The caller has checked that `id` is free.
    pub fn create(&self, id: Identifier, name: TextComponent) -> Arc<CustomBossEvent> {
        // A random network id, like vanilla's `Mth.createInsecureUUID`.
        let event = Arc::new(CustomBossEvent::new(
            Uuid::new_v4(),
            id.clone(),
            name,
            Arc::clone(&self.revision),
        ));
        self.events.lock().insert(id, Arc::clone(&event));
        self.set_dirty();
        event
    }

    /// Vanilla `remove`.
    pub fn remove(&self, event: &CustomBossEvent) {
        if self.events.lock().remove(event.custom_id()).is_some() {
            self.set_dirty();
        }
    }

    /// Vanilla `getIds`, sorted.
    #[must_use]
    pub fn ids(&self) -> Vec<Identifier> {
        let mut ids = self.events.lock().keys().cloned().collect::<Vec<_>>();
        ids.sort_by_cached_key(ToString::to_string);
        ids
    }

    /// Vanilla `getEvents`, sorted by id.
    #[must_use]
    pub fn events(&self) -> Vec<Arc<CustomBossEvent>> {
        let mut events = self.events.lock().values().cloned().collect::<Vec<_>>();
        events.sort_by_cached_key(|event| event.custom_id().to_string());
        events
    }

    /// Vanilla `onPlayerConnect`.
    pub fn on_player_connect(&self, player: &Arc<Player>) {
        for event in self.events() {
            event.on_player_connect(player);
        }
    }

    /// Vanilla `onPlayerDisconnect`.
    pub fn on_player_disconnect(&self, player: &Player) {
        for event in self.events() {
            event.on_player_disconnect(player);
        }
    }

    fn pending_save(&self) -> io::Result<Option<(u64, PersistentCustomBossEvents)>> {
        let events = self.events.lock();
        let revision = self.revision.load(Ordering::Acquire);
        if revision == self.saved_revision.load(Ordering::Acquire) {
            return Ok(None);
        }
        let mut persistent = PersistentCustomBossEvents::default();
        for (id, event) in events.iter() {
            persistent.events.insert(id.to_string(), event.pack()?);
        }
        Ok(Some((revision, persistent)))
    }

    fn mark_saved(&self, revision: u64) {
        self.saved_revision.fetch_max(revision, Ordering::Release);
    }
}

/// Every domain's [`CustomBossEvents`].
pub struct DomainCustomBossEvents {
    domains: BTreeMap<String, CustomBossEvents>,
    save_lock: AsyncMutex<()>,
}

impl DomainCustomBossEvents {
    /// Loads each domain's boss bars through its default world.
    pub(crate) async fn load(worlds: &WorldMap) -> io::Result<Self> {
        let mut domains = BTreeMap::new();
        for domain in worlds.domain_names() {
            let world = domain_default_world(worlds, domain)?;
            let persistent: PersistentCustomBossEvents = world
                .saved_data
                .load_or_default(saved_data_names::CUSTOM_BOSS_EVENTS)
                .await
                .map_err(|error| boss_events_io_error(domain, &error))?;
            let events = CustomBossEvents::from_persistent(persistent)
                .map_err(|error| boss_events_io_error(domain, &error))?;
            domains.insert(domain.to_owned(), events);
        }
        Ok(Self {
            domains,
            save_lock: AsyncMutex::new(()),
        })
    }

    /// Returns a domain's boss bars.
    #[must_use]
    pub fn get(&self, domain: &str) -> Option<&CustomBossEvents> {
        self.domains.get(domain)
    }

    /// Saves every changed domain's boss bars, returning how many were written.
    pub async fn save(&self, worlds: &WorldMap) -> io::Result<usize> {
        let _save_guard = self.save_lock.lock().await;
        let mut saved = 0;
        for (domain, events) in &self.domains {
            let Some((revision, persistent)) = events
                .pending_save()
                .map_err(|error| boss_events_io_error(domain, &error))?
            else {
                continue;
            };
            let world = domain_default_world(worlds, domain)?;
            world
                .saved_data
                .save(saved_data_names::CUSTOM_BOSS_EVENTS, &persistent)
                .await
                .map_err(|error| boss_events_io_error(domain, &error))?;
            events.mark_saved(revision);
            saved += 1;
        }
        Ok(saved)
    }
}

fn domain_default_world<'a>(worlds: &'a WorldMap, domain: &str) -> io::Result<&'a World> {
    worlds
        .default_world(domain)
        .map(AsRef::as_ref)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("domain '{domain}' has no loaded default world"),
            )
        })
}

fn invalid_data(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn boss_events_io_error(domain: &str, error: &io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("boss bar I/O failed for domain '{domain}': {error}"),
    )
}

#[cfg(test)]
mod tests;
