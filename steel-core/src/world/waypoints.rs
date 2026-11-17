//! Vanilla `ServerWaypointManager` and the entity side of
//! `WaypointTransmitter`: which players see which entities on their locator
//! bar, and how precisely.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rustc_hash::FxHashMap;
use simdnbt::owned::NbtCompound;
use steel_protocol::packets::game::{
    CTrackedWaypoint, WaypointIcon, WaypointOperation, WaypointPosition,
};
use steel_registry::vanilla_attributes;
use steel_registry::vanilla_game_rules::LOCATOR_BAR;
use steel_utils::locks::SyncMutex;
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

#[derive(Clone, Debug)]
struct Connection {
    icon: WaypointIcon,
    kind: ConnectionKind,
}

#[derive(Default)]
struct State {
    waypoints: FxHashMap<i32, SharedEntity>,
    players: FxHashMap<i32, Arc<Player>>,
    /// Keyed by `(receiver, source)`, like vanilla's player-by-waypoint table.
    connections: FxHashMap<(i32, i32), Connection>,
}

/// Vanilla `ServerWaypointManager`, one per world.
#[derive(Default)]
pub struct WaypointManager {
    state: SyncMutex<State>,
    /// Tracked waypoints that are not players, so ordinary entity moves can
    /// skip the lock while no mob transmits.
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
        let mut state = self.state.lock();
        if state
            .waypoints
            .insert(waypoint.id(), Arc::clone(waypoint))
            .is_none()
            && waypoint.as_player().is_none()
        {
            self.entity_waypoints.fetch_add(1, Ordering::Relaxed);
        }
        let players: Vec<_> = state.players.values().cloned().collect();
        for player in players {
            state.create_connection(&player, waypoint);
        }
    }

    /// Vanilla `updateWaypoint`, for an entity that moved.
    pub fn update_waypoint(&self, waypoint_id: i32) {
        let mut state = self.state.lock();
        let Some(waypoint) = state.waypoints.get(&waypoint_id).cloned() else {
            return;
        };
        let players: Vec<_> = state.players.values().cloned().collect();
        for player in players {
            if let Some(connection) = state.connections.get(&(player.id(), waypoint_id)).cloned() {
                state.update_connection(&player, &waypoint, connection);
            } else {
                state.create_connection(&player, &waypoint);
            }
        }
    }

    /// Vanilla `untrackWaypoint`.
    pub fn untrack_waypoint(&self, waypoint_id: i32) {
        let mut state = self.state.lock();
        state.untrack(waypoint_id, &self.entity_waypoints);
    }

    /// Vanilla `addPlayer`: the player starts receiving, and transmitting when
    /// its range allows.
    pub fn add_player(&self, player: &Arc<Player>) {
        {
            let mut state = self.state.lock();
            state.players.insert(player.id(), Arc::clone(player));
            let waypoints: Vec<_> = state.waypoints.values().cloned().collect();
            for waypoint in waypoints {
                state.create_connection(player, &waypoint);
            }
        }
        if is_transmitting(&**player) {
            self.track_waypoint(&(Arc::clone(player) as SharedEntity));
        }
    }

    /// Vanilla `updatePlayer`, for a receiver that moved or whose view changed.
    pub fn update_player(&self, player_id: i32) {
        let mut state = self.state.lock();
        let Some(player) = state.players.get(&player_id).cloned() else {
            return;
        };
        let waypoints: Vec<_> = state.waypoints.values().cloned().collect();
        for waypoint in waypoints {
            if let Some(connection) = state.connections.get(&(player_id, waypoint.id())).cloned() {
                state.update_connection(&player, &waypoint, connection);
            } else {
                state.create_connection(&player, &waypoint);
            }
        }
    }

    /// Vanilla `removePlayer`: drops what the player sees and its own
    /// waypoint.
    pub fn remove_player(&self, player_id: i32) {
        let mut state = self.state.lock();
        let keys: Vec<_> = state
            .connections
            .keys()
            .filter(|(receiver_id, _)| *receiver_id == player_id)
            .copied()
            .collect();
        for (receiver_id, source_id) in keys {
            state.connections.remove(&(receiver_id, source_id));
            state.disconnect(receiver_id, source_id);
        }
        state.untrack(player_id, &self.entity_waypoints);
        state.players.remove(&player_id);
    }

    /// Vanilla `breakAllConnections`, when the `locator_bar` rule turns off.
    pub fn break_all_connections(&self) {
        let mut state = self.state.lock();
        let connections: Vec<_> = state.connections.drain().map(|(key, _)| key).collect();
        for (receiver_id, source_id) in connections {
            state.disconnect(receiver_id, source_id);
        }
    }

    /// Vanilla `remakeConnections`, after a team color change.
    pub fn remake_connections(&self, waypoint_id: i32) {
        let mut state = self.state.lock();
        let Some(waypoint) = state.waypoints.get(&waypoint_id).cloned() else {
            return;
        };
        let players: Vec<_> = state.players.values().cloned().collect();
        for player in players {
            state.create_connection(&player, &waypoint);
        }
    }

    /// Vanilla `transmitters`: every tracked waypoint.
    #[must_use]
    pub fn transmitters(&self) -> Vec<SharedEntity> {
        self.state.lock().waypoints.values().cloned().collect()
    }

    /// Whether any entity other than a player transmits.
    #[must_use]
    pub fn has_entity_waypoints(&self) -> bool {
        self.entity_waypoints.load(Ordering::Relaxed) > 0
    }
}

impl State {
    /// Vanilla `untrackWaypoint`.
    fn untrack(&mut self, waypoint_id: i32, entity_waypoints: &AtomicUsize) {
        let keys: Vec<_> = self
            .connections
            .keys()
            .filter(|(_, source_id)| *source_id == waypoint_id)
            .copied()
            .collect();
        for (receiver_id, source_id) in keys {
            self.connections.remove(&(receiver_id, source_id));
            self.disconnect(receiver_id, source_id);
        }
        if let Some(waypoint) = self.waypoints.remove(&waypoint_id)
            && waypoint.as_player().is_none()
        {
            entity_waypoints.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Vanilla `Connection.disconnect`: the receiver forgets the source.
    fn disconnect(&self, receiver_id: i32, source_id: i32) {
        if let (Some(receiver), Some(source)) = (
            self.players.get(&receiver_id),
            self.waypoints.get(&source_id),
        ) {
            receiver.send_packet(CTrackedWaypoint::remove(source.uuid()));
        }
    }

    /// Vanilla `createConnection`.
    fn create_connection(&mut self, player: &Arc<Player>, waypoint: &SharedEntity) {
        if player.id() == waypoint.id() || !is_locator_bar_enabled_for(player) {
            return;
        }
        let key = (player.id(), waypoint.id());
        if let Some(connection) = make_connection(waypoint, player) {
            self.connections.insert(key, connection.clone());
            send(player, waypoint, &connection, WaypointOperation::Track);
        } else if self.connections.remove(&key).is_some() {
            player.send_packet(CTrackedWaypoint::remove(waypoint.uuid()));
        }
    }

    /// Vanilla `updateConnection`.
    fn update_connection(
        &mut self,
        player: &Arc<Player>,
        waypoint: &SharedEntity,
        connection: Connection,
    ) {
        if player.id() == waypoint.id() || !is_locator_bar_enabled_for(player) {
            return;
        }
        let key = (player.id(), waypoint.id());
        if !is_broken(&connection, waypoint, player) {
            if let Some(updated) = update(&connection, waypoint, player) {
                self.connections.insert(key, updated);
            }
            return;
        }
        if let Some(replacement) = make_connection(waypoint, player) {
            send(player, waypoint, &replacement, WaypointOperation::Track);
            self.connections.insert(key, replacement);
        } else {
            player.send_packet(CTrackedWaypoint::remove(waypoint.uuid()));
            self.connections.remove(&key);
        }
    }
}

fn is_locator_bar_enabled_for(player: &Player) -> bool {
    player.get_world().get_game_rule(&LOCATOR_BAR)
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
fn update(connection: &Connection, source: &SharedEntity, receiver: &Player) -> Option<Connection> {
    let kind = match connection.kind {
        ConnectionKind::Azimuth(last) => {
            let angle = azimuth(source, receiver);
            ((angle - last).abs() > AZIMUTH_THRESHOLD).then_some(ConnectionKind::Azimuth(angle))
        }
        ConnectionKind::Block(last) => {
            let current = source.block_position();
            (manhattan(current, last) > 0).then_some(ConnectionKind::Block(current))
        }
        ConnectionKind::Chunk(last) => {
            let current = chunk_position(source);
            (chessboard(current, last) > 0).then_some(ConnectionKind::Chunk(current))
        }
    }?;
    let updated = Connection {
        icon: connection.icon.clone(),
        kind,
    };
    send(receiver, source, &updated, WaypointOperation::Update);
    Some(updated)
}

/// Vanilla `Connection.isBroken` for each connection kind.
fn is_broken(connection: &Connection, source: &SharedEntity, receiver: &Player) -> bool {
    match connection.kind {
        ConnectionKind::Azimuth(_) => {
            does_source_ignore_receiver(source, receiver)
                || is_chunk_visible(chunk_position(source), receiver)
                || !is_really_far(source, receiver)
        }
        ConnectionKind::Block(last) => {
            manhattan(last, source.block_position()) > 1
                || does_source_ignore_receiver(source, receiver)
        }
        ConnectionKind::Chunk(last) => {
            chessboard(last, chunk_position(source)) > 1
                || does_source_ignore_receiver(source, receiver)
                || is_chunk_visible(last, receiver)
        }
    }
}

/// Vanilla `LivingEntity.makeWaypointConnectionWith`.
fn make_connection(source: &SharedEntity, receiver: &Player) -> Option<Connection> {
    if source.is_first_tick()
        || source.id() == receiver.id()
        || does_source_ignore_receiver(source, receiver)
    {
        return None;
    }
    let icon = clone_and_assign_style(source);
    let kind = if is_really_far(source, receiver) {
        ConnectionKind::Azimuth(azimuth(source, receiver))
    } else if is_chunk_visible(chunk_position(source), receiver) {
        ConnectionKind::Block(source.block_position())
    } else {
        ConnectionKind::Chunk(chunk_position(source))
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
fn does_source_ignore_receiver(source: &SharedEntity, receiver: &Player) -> bool {
    if receiver.is_spectator() {
        return false;
    }
    if source.is_spectator() || source.has_indirect_passenger(receiver) {
        return true;
    }
    let transmit = source.as_living_entity().map_or(0.0, |living| {
        living
            .attributes()
            .lock()
            .get_value(vanilla_attributes::WAYPOINT_TRANSMIT_RANGE)
            .unwrap_or(0.0)
    });
    let receive = receiver
        .attributes()
        .lock()
        .get_value(vanilla_attributes::WAYPOINT_RECEIVE_RANGE)
        .unwrap_or(0.0);
    f64::from(distance(source, receiver)) >= transmit.min(receive)
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
fn is_really_far(source: &SharedEntity, receiver: &Player) -> bool {
    distance(source, receiver) > REALLY_FAR_DISTANCE
}

/// Vanilla `Entity.distanceTo`, in float precision.
fn distance(source: &SharedEntity, receiver: &Player) -> f32 {
    let delta = (source.position() - receiver.position()).as_vec3();
    delta.length()
}

/// The receiver's direction as seen from the source, rotated clockwise by 90
/// degrees like vanilla.
fn azimuth(source: &SharedEntity, receiver: &Player) -> f32 {
    let delta = receiver.position() - source.position();
    // Vanilla `Vec3.rotateClockwise90`: (x, y, z) -> (-z, y, x).
    delta.x.atan2(-delta.z) as f32
}

/// Vanilla `Entity.chunkPosition`.
fn chunk_position(entity: &SharedEntity) -> ChunkPos {
    ChunkPos::from_block_pos(entity.block_position())
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
