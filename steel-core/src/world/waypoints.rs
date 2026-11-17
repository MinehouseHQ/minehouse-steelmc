//! Vanilla `ServerWaypointManager` and the entity side of
//! `WaypointTransmitter`: which players see which entities on their locator
//! bar, and how precisely.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use glam::DVec3;
use rustc_hash::FxHashMap;
use scc::hash_map::Entry;
use simdnbt::owned::NbtCompound;
use steel_protocol::packets::game::{
    CTrackedWaypoint, WaypointIcon, WaypointOperation, WaypointPosition,
};
use steel_registry::vanilla_attributes;
use steel_registry::vanilla_game_rules::LOCATOR_BAR;
use steel_utils::locks::SyncRwLock;
use steel_utils::{BlockPos, ChunkPos, Identifier};

use crate::chunk::player_chunk_view::PlayerChunkView;
use crate::entity::{Entity, LivingEntity, SharedEntity};
use crate::player::Player;
use crate::scoreboard::TeamColor;
use crate::world::particle_codec;

/// Vanilla `WaypointTransmitter.REALLY_FAR_DISTANCE`.
const REALLY_FAR_DISTANCE: f32 = 332.0;
/// Vanilla azimuth update threshold: half a degree, in radians.
const AZIMUTH_THRESHOLD: f32 = 0.008_726_646;
/// Vanilla `Waypoint.Icon.cloneAndAssignStyle` color for black teams.
const BLACK_TEAM_WAYPOINT_COLOR: i32 = -13_619_152;

/// Vanilla `WaypointTransmitter.Connection` state: what the receiver was last
/// told about the source.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ConnectionKind {
    /// `EntityAzimuthConnection`: only the direction.
    Azimuth(f32),
    /// `EntityBlockConnection`: the exact block.
    Block(BlockPos),
    /// `EntityChunkConnection`: the chunk.
    Chunk(ChunkPos),
}

/// One end of a connection, read once per update: each read locks the
/// entity, and every player's move otherwise read every other player's state
/// several times per pair.
#[derive(Clone, Copy)]
struct Endpoint {
    position: DVec3,
    spectator: bool,
    /// Transmit range for a source, receive range for a receiver.
    range: f64,
}

impl Endpoint {
    fn source(source: &SharedEntity) -> Self {
        Self {
            position: source.position(),
            spectator: source.is_spectator(),
            range: source.as_living_entity().map_or(0.0, |living| {
                living
                    .attributes()
                    .lock()
                    .get_value(vanilla_attributes::WAYPOINT_TRANSMIT_RANGE)
                    .unwrap_or(0.0)
            }),
        }
    }

    fn receiver(receiver: &Player) -> Self {
        Self {
            position: receiver.position(),
            spectator: receiver.is_spectator(),
            range: receiver
                .attributes()
                .lock()
                .get_value(vanilla_attributes::WAYPOINT_RECEIVE_RANGE)
                .unwrap_or(0.0),
        }
    }

    /// Vanilla `Entity.blockPosition`.
    const fn block_position(&self) -> BlockPos {
        BlockPos::new(
            self.position.x.floor() as i32,
            self.position.y.floor() as i32,
            self.position.z.floor() as i32,
        )
    }

    /// Vanilla `Entity.chunkPosition`.
    const fn chunk_position(&self) -> ChunkPos {
        ChunkPos::from_block_pos(self.block_position())
    }
}

/// A source and a receiver with their state read.
struct Pair<'a> {
    source: &'a SharedEntity,
    from: Endpoint,
    receiver: &'a Player,
    to: Endpoint,
}

#[derive(Clone, Debug)]
struct Connection {
    icon: WaypointIcon,
    kind: ConnectionKind,
}

/// Who transmits and who receives. Changes only when a waypoint or player
/// comes or goes, so moves read it without blocking each other.
#[derive(Default)]
struct Registry {
    waypoints: FxHashMap<i32, SharedEntity>,
    players: FxHashMap<i32, Arc<Player>>,
}

/// Vanilla `ServerWaypointManager`, one per world.
///
/// Connections are keyed by `(receiver, source)`, like vanilla's
/// player-by-waypoint table. Each connection is locked on its own, so players
/// moving on different islands update their connections in parallel; the
/// packet for a connection is sent while it is locked, so the receiver sees
/// that connection's changes in order.
#[derive(Default)]
pub struct WaypointManager {
    registry: SyncRwLock<Registry>,
    connections: scc::HashMap<(i32, i32), Connection>,
    /// Tracked waypoints that are not players, so ordinary entity moves can
    /// skip the manager while no mob transmits.
    entity_waypoints: AtomicUsize,
}

impl WaypointManager {
    /// Creates an empty manager.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Vanilla `trackWaypoint`.
    pub fn track_waypoint(&self, waypoint: &SharedEntity) {
        let mut registry = self.registry.write();
        if registry
            .waypoints
            .insert(waypoint.id(), Arc::clone(waypoint))
            .is_none()
            && waypoint.as_player().is_none()
        {
            self.entity_waypoints.fetch_add(1, Ordering::Relaxed);
        }
        for player in registry.players.values() {
            self.create_connection(player, waypoint);
        }
    }

    /// Vanilla `updateWaypoint`, for an entity that moved.
    pub fn update_waypoint(&self, waypoint_id: i32) {
        let registry = self.registry.read();
        let Some(waypoint) = registry.waypoints.get(&waypoint_id) else {
            return;
        };
        let mut from = None;
        for player in registry.players.values() {
            if is_receiver(player, waypoint) {
                let from = *from.get_or_insert_with(|| Endpoint::source(waypoint));
                self.refresh_connection(&Pair {
                    source: waypoint,
                    from,
                    receiver: player,
                    to: Endpoint::receiver(player),
                });
            }
        }
    }

    /// Vanilla `untrackWaypoint`.
    pub fn untrack_waypoint(&self, waypoint_id: i32) {
        let mut registry = self.registry.write();
        self.untrack(&mut registry, waypoint_id);
    }

    /// Vanilla `addPlayer`: the player starts receiving, and transmitting when
    /// its range allows.
    pub fn add_player(&self, player: &Arc<Player>) {
        {
            let mut registry = self.registry.write();
            registry.players.insert(player.id(), Arc::clone(player));
            for waypoint in registry.waypoints.values() {
                self.create_connection(player, waypoint);
            }
        }
        if is_transmitting(&**player) {
            self.track_waypoint(&(Arc::clone(player) as SharedEntity));
        }
    }

    /// Vanilla `updatePlayer`, for a receiver that moved or whose view changed.
    pub fn update_player(&self, player_id: i32) {
        let registry = self.registry.read();
        let Some(player) = registry.players.get(&player_id) else {
            return;
        };
        if !is_locator_bar_enabled_for(player) {
            return;
        }
        let to = Endpoint::receiver(player);
        for waypoint in registry.waypoints.values() {
            if waypoint.id() != player.id() {
                self.refresh_connection(&Pair {
                    source: waypoint,
                    from: Endpoint::source(waypoint),
                    receiver: player,
                    to,
                });
            }
        }
    }

    /// Vanilla `removePlayer`: drops what the player sees and its own
    /// waypoint.
    pub fn remove_player(&self, player_id: i32) {
        let mut registry = self.registry.write();
        for (receiver_id, source_id) in
            self.remove_connections(|(receiver_id, _)| receiver_id == player_id)
        {
            disconnect(&registry, receiver_id, source_id);
        }
        self.untrack(&mut registry, player_id);
        registry.players.remove(&player_id);
    }

    /// Vanilla `breakAllConnections`, when the `locator_bar` rule turns off.
    pub fn break_all_connections(&self) {
        let registry = self.registry.read();
        for (receiver_id, source_id) in self.remove_connections(|_| true) {
            disconnect(&registry, receiver_id, source_id);
        }
    }

    /// Vanilla `remakeConnections`, after a team color change.
    pub fn remake_connections(&self, waypoint_id: i32) {
        let registry = self.registry.read();
        let Some(waypoint) = registry.waypoints.get(&waypoint_id) else {
            return;
        };
        for player in registry.players.values() {
            self.create_connection(player, waypoint);
        }
    }

    /// Vanilla `transmitters`: every tracked waypoint.
    #[must_use]
    pub fn transmitters(&self) -> Vec<SharedEntity> {
        self.registry.read().waypoints.values().cloned().collect()
    }

    /// Whether any entity other than a player transmits.
    #[must_use]
    pub fn has_entity_waypoints(&self) -> bool {
        self.entity_waypoints.load(Ordering::Relaxed) > 0
    }

    /// Removes the connections whose `(receiver, source)` key matches,
    /// returning their keys.
    fn remove_connections(&self, mut matches: impl FnMut((i32, i32)) -> bool) -> Vec<(i32, i32)> {
        let mut removed = Vec::new();
        self.connections.retain_sync(|&key, _| {
            if matches(key) {
                removed.push(key);
                false
            } else {
                true
            }
        });
        removed
    }

    /// Vanilla `untrackWaypoint`.
    fn untrack(&self, registry: &mut Registry, waypoint_id: i32) {
        for (receiver_id, source_id) in
            self.remove_connections(|(_, source_id)| source_id == waypoint_id)
        {
            disconnect(registry, receiver_id, source_id);
        }
        if let Some(waypoint) = registry.waypoints.remove(&waypoint_id)
            && waypoint.as_player().is_none()
        {
            self.entity_waypoints.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Vanilla `createConnection`.
    fn create_connection(&self, player: &Arc<Player>, waypoint: &SharedEntity) {
        if !is_receiver(player, waypoint) {
            return;
        }
        let pair = Pair {
            source: waypoint,
            from: Endpoint::source(waypoint),
            receiver: player,
            to: Endpoint::receiver(player),
        };
        let entry = self.connections.entry_sync((player.id(), waypoint.id()));
        match (make_connection(&pair), entry) {
            (Some(connection), Entry::Occupied(mut occupied)) => {
                send(player, waypoint, &connection, WaypointOperation::Track);
                occupied.insert(connection);
            }
            (Some(connection), Entry::Vacant(vacant)) => {
                send(player, waypoint, &connection, WaypointOperation::Track);
                vacant.insert_entry(connection);
            }
            (None, Entry::Occupied(occupied)) => {
                player.send_packet(CTrackedWaypoint::remove(waypoint.uuid()));
                let _ = occupied.remove();
            }
            (None, Entry::Vacant(_)) => {}
        }
    }

    /// Vanilla `updateWaypoint`/`updatePlayer` for one pair: updates the
    /// connection when there is one (vanilla `updateConnection`), creates it
    /// otherwise.
    fn refresh_connection(&self, pair: &Pair<'_>) {
        let (player, waypoint) = (pair.receiver, pair.source);
        match self.connections.entry_sync((player.id(), waypoint.id())) {
            Entry::Vacant(vacant) => {
                if let Some(connection) = make_connection(pair) {
                    send(player, waypoint, &connection, WaypointOperation::Track);
                    vacant.insert_entry(connection);
                }
            }
            Entry::Occupied(mut occupied) => {
                if !is_broken(occupied.get(), pair) {
                    if let Some(updated) = update(occupied.get(), pair) {
                        occupied.insert(updated);
                    }
                } else if let Some(replacement) = make_connection(pair) {
                    send(player, waypoint, &replacement, WaypointOperation::Track);
                    occupied.insert(replacement);
                } else {
                    player.send_packet(CTrackedWaypoint::remove(waypoint.uuid()));
                    let _ = occupied.remove();
                }
            }
        }
    }
}

/// Vanilla `Connection.disconnect`: the receiver forgets the source.
fn disconnect(registry: &Registry, receiver_id: i32, source_id: i32) {
    if let (Some(receiver), Some(source)) = (
        registry.players.get(&receiver_id),
        registry.waypoints.get(&source_id),
    ) {
        receiver.send_packet(CTrackedWaypoint::remove(source.uuid()));
    }
}

fn is_locator_bar_enabled_for(player: &Player) -> bool {
    player.get_world().get_game_rule(&LOCATOR_BAR)
}

/// Whether `player` may hold a connection to `waypoint` at all.
fn is_receiver(player: &Player, waypoint: &SharedEntity) -> bool {
    player.id() != waypoint.id() && is_locator_bar_enabled_for(player)
}

fn send(
    player: &Player,
    waypoint: &SharedEntity,
    connection: &Connection,
    operation: WaypointOperation,
) {
    let position = match connection.kind {
        ConnectionKind::Azimuth(angle) => WaypointPosition::Azimuth(angle),
        ConnectionKind::Block(pos) => WaypointPosition::Block(pos),
        ConnectionKind::Chunk(pos) => WaypointPosition::Chunk(pos),
    };
    player.send_packet(CTrackedWaypoint::entity(
        operation,
        waypoint.uuid(),
        connection.icon.clone(),
        position,
    ));
}

/// Vanilla `Connection.update`: tells the receiver about a move worth
/// showing, and returns the new state when it did.
fn update(connection: &Connection, pair: &Pair<'_>) -> Option<Connection> {
    let kind = match connection.kind {
        ConnectionKind::Azimuth(last) => {
            let angle = azimuth(pair);
            ((angle - last).abs() > AZIMUTH_THRESHOLD).then_some(ConnectionKind::Azimuth(angle))
        }
        ConnectionKind::Block(last) => {
            let current = pair.from.block_position();
            (manhattan(current, last) > 0).then_some(ConnectionKind::Block(current))
        }
        ConnectionKind::Chunk(last) => {
            let current = pair.from.chunk_position();
            (chessboard(current, last) > 0).then_some(ConnectionKind::Chunk(current))
        }
    }?;
    let updated = Connection {
        icon: connection.icon.clone(),
        kind,
    };
    send(
        pair.receiver,
        pair.source,
        &updated,
        WaypointOperation::Update,
    );
    Some(updated)
}

/// Vanilla `Connection.isBroken` for each connection kind.
fn is_broken(connection: &Connection, pair: &Pair<'_>) -> bool {
    match connection.kind {
        ConnectionKind::Azimuth(_) => {
            does_source_ignore_receiver(pair)
                || is_chunk_visible(pair.from.chunk_position(), pair.receiver)
                || !is_really_far(pair)
        }
        ConnectionKind::Block(last) => {
            manhattan(last, pair.from.block_position()) > 1 || does_source_ignore_receiver(pair)
        }
        ConnectionKind::Chunk(last) => {
            chessboard(last, pair.from.chunk_position()) > 1
                || does_source_ignore_receiver(pair)
                || is_chunk_visible(last, pair.receiver)
        }
    }
}

/// Vanilla `LivingEntity.makeWaypointConnectionWith`.
fn make_connection(pair: &Pair<'_>) -> Option<Connection> {
    if pair.source.is_first_tick()
        || pair.source.id() == pair.receiver.id()
        || does_source_ignore_receiver(pair)
    {
        return None;
    }
    let icon = clone_and_assign_style(pair.source);
    let kind = if is_really_far(pair) {
        ConnectionKind::Azimuth(azimuth(pair))
    } else if is_chunk_visible(pair.from.chunk_position(), pair.receiver) {
        ConnectionKind::Block(pair.from.block_position())
    } else {
        ConnectionKind::Chunk(pair.from.chunk_position())
    };
    Some(Connection { icon, kind })
}

/// Vanilla `Waypoint.Icon.cloneAndAssignStyle`: an unset color falls back to
/// the source's team color.
fn clone_and_assign_style(source: &SharedEntity) -> WaypointIcon {
    let mut icon = source
        .as_living_entity()
        .map_or_else(WaypointIcon::default, LivingEntity::waypoint_icon);
    if icon.color.is_none() {
        icon.color = source
            .team()
            .and_then(|team| team.color())
            .map(|color| match color {
                TeamColor::Black => BLACK_TEAM_WAYPOINT_COLOR,
                color => color.rgb(),
            });
    }
    icon
}

/// Vanilla `LivingEntity.isTransmittingWaypoint`.
#[must_use]
pub fn is_transmitting(entity: &dyn Entity) -> bool {
    entity.as_living_entity().is_some_and(|living| {
        living
            .attributes()
            .lock()
            .get_value(vanilla_attributes::WAYPOINT_TRANSMIT_RANGE)
            .unwrap_or(0.0)
            > 0.0
    })
}

/// Vanilla `ServerPlayer.isReceivingWaypoints`.
#[must_use]
pub fn is_receiving(player: &Player) -> bool {
    player
        .attributes()
        .lock()
        .get_value(vanilla_attributes::WAYPOINT_RECEIVE_RANGE)
        .unwrap_or(0.0)
        > 0.0
}

/// Vanilla `WaypointTransmitter.doesSourceIgnoreReceiver`.
fn does_source_ignore_receiver(pair: &Pair<'_>) -> bool {
    if pair.to.spectator {
        return false;
    }
    if pair.from.spectator || pair.source.has_indirect_passenger(pair.receiver) {
        return true;
    }
    f64::from(distance(pair)) >= pair.from.range.min(pair.to.range)
}

/// Vanilla `WaypointTransmitter.isChunkVisible`: inside the receiver's view
/// distance, not counting the border ring.
fn is_chunk_visible(chunk: ChunkPos, receiver: &Player) -> bool {
    receiver.last_tracking_view.lock().is_some_and(|view| {
        PlayerChunkView::is_within_distance(
            view.center.0.x,
            view.center.0.y,
            i32::from(view.view_distance),
            chunk.0.x,
            chunk.0.y,
            false,
        )
    })
}

/// Vanilla `WaypointTransmitter.isReallyFar`.
fn is_really_far(pair: &Pair<'_>) -> bool {
    distance(pair) > REALLY_FAR_DISTANCE
}

/// Vanilla `Entity.distanceTo`, in float precision.
fn distance(pair: &Pair<'_>) -> f32 {
    (pair.from.position - pair.to.position).as_vec3().length()
}

/// The receiver's direction as seen from the source, rotated clockwise by 90
/// degrees like vanilla.
fn azimuth(pair: &Pair<'_>) -> f32 {
    let delta = pair.to.position - pair.from.position;
    // Vanilla `Vec3.rotateClockwise90`: (x, y, z) -> (-z, y, x).
    delta.x.atan2(-delta.z) as f32
}

const fn manhattan(a: BlockPos, b: BlockPos) -> i32 {
    (a.x() - b.x()).abs() + (a.y() - b.y()).abs() + (a.z() - b.z()).abs()
}

fn chessboard(a: ChunkPos, b: ChunkPos) -> i32 {
    (a.0.x - b.0.x).abs().max((a.0.y - b.0.y).abs())
}

/// Vanilla `Waypoint.Icon.CODEC` encoding, saved as `locator_bar_icon`.
#[must_use]
pub fn icon_to_nbt(icon: &WaypointIcon) -> NbtCompound {
    let mut nbt = NbtCompound::new();
    nbt.insert("style", icon.style.to_string());
    if let Some(color) = icon.color {
        nbt.insert("color", color);
    }
    nbt
}

/// Vanilla `Waypoint.Icon.CODEC` decoding; `None` when the style is missing
/// or invalid, which vanilla answers with a default icon.
#[must_use]
pub fn icon_from_nbt(nbt: &NbtCompound) -> Option<WaypointIcon> {
    let style = nbt.string("style")?.to_str().parse::<Identifier>().ok()?;
    let color = match nbt.get("color") {
        None => None,
        Some(tag) => Some(particle_codec::rgb_color(tag).ok()?.raw()),
    };
    Some(WaypointIcon { style, color })
}
