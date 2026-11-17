//! Entity tracking system for managing which players can see which entities.
//!
//! Keeps the vanilla visibility predicate in block space. Vanilla stores an
//! entity tracking range as client chunks, multiplies it by 16, caps it by the
//! player's view distance, and then checks horizontal squared distance.

use std::sync::Arc;

use glam::DVec3;
use rayon::iter::{
    IndexedParallelIterator as _, IntoParallelRefIterator as _, ParallelIterator as _,
};
use rustc_hash::FxHashSet;
use steel_protocol::packets::game::{
    AttributeSnapshot, CAddEntity, CRemoveEntities, CSetEntityData, CSetEntityLink,
    CSetEntityMotion, CSetEquipment, CSetPassengers, CUpdateAttributes, EquipmentSlotItem,
    to_angle_byte,
};
use steel_registry::entity_data::DataValue;
use steel_registry::{RegistryEntry, vanilla_entities};
use steel_utils::ChunkPos;
use steel_utils::locks::{SyncMutex, SyncRwLock};

use crate::chunk::player_chunk_view::PlayerChunkView;
use crate::entity::leash::Leashable;
use crate::entity::{
    Entity, EntityMovementSyncPacket, MobEffectSyncPacket, ServerEntityMovementSyncState,
    ServerEntityMovementSyncUpdate, SharedEntity, WeakEntity,
};
use crate::player::Player;
use crate::world::region::regionized_ticking_enabled;
use crate::world::tick_pool::{TICK_POOL_MIN_SPLIT, install_on_tick_pool};

const BLOCKS_PER_CHUNK: f64 = 16.0;

/// World-level entity tracker.
pub struct EntityTracker {
    /// Maps entity ID to its tracking data.
    entities: scc::HashMap<i32, TrackedEntity>,
    /// Tracked entity ids by their registered chunk.
    ///
    /// A player refresh only has to look at entities in its view and entities it already sees,
    /// so it reads these indexes instead of scanning every tracked entity in the world.
    chunk_entities: scc::HashMap<ChunkPos, FxHashSet<i32>>,
    /// Entity ids each player currently sees, mirroring every `seen_by` set.
    player_entities: scc::HashMap<i32, FxHashSet<i32>>,
}

/// Packet sinks used by [`EntityTracker::send_changes`].
pub struct EntityChangeSenders<
    Movement,
    SelfMovement,
    EntityData,
    Attributes,
    MobEffects,
    Equipment,
    Passengers,
    EntityLink,
> {
    /// Broadcasts movement and velocity sync packets.
    pub movement: Movement,
    /// Sends vanilla self-inclusive hurt motion sync to one player.
    pub self_movement: SelfMovement,
    /// Broadcasts entity data watcher changes.
    pub entity_data: EntityData,
    /// Broadcasts dirty syncable attributes.
    pub attributes: Attributes,
    /// Sends mob-effect add/remove packets to a specific player.
    pub mob_effects: MobEffects,
    /// Broadcasts dirty equipment slots.
    pub equipment: Equipment,
    /// Sends passenger updates to a specific player.
    pub passengers: Passengers,
    /// Broadcasts leash/link holder updates.
    pub entity_link: EntityLink,
}

/// Tracking data for a single entity.
struct TrackedEntity {
    /// Weak reference to the entity. When this fails to upgrade, entity is dead.
    entity: WeakEntity,
    /// Vanilla `ServerEntity` movement sync state owned by the tracker.
    server_entity: SyncMutex<ServerEntityMovementSyncState>,
    /// Last direct passenger ids sent to tracking clients.
    last_passenger_ids: SyncMutex<Vec<i32>>,
    /// Last leash holder id sent to tracking clients.
    last_leash_holder_id: SyncMutex<Option<i32>>,
    /// Vanilla client tracking range converted to blocks.
    tracking_range: EntityTrackingRange,
    /// Current chunk used by the player-view predicate.
    registered_chunk: ChunkPos,
    /// Players currently tracking this entity (interior mutable for concurrent access).
    seen_by: SyncRwLock<FxHashSet<i32>>,
}

/// What one tracked entity sends this tick, gathered by
/// [`EntityTracker::collect_entity_changes`].
#[derive(Default)]
struct EntityChanges {
    /// The entity is gone; its tracking entry is dropped.
    dead: bool,
    passenger_packets: Vec<(i32, CSetPassengers)>,
    /// Passengers changed, so the set of players tracking it is refreshed.
    refresh_players: bool,
    framed_map_sync: Option<SharedEntity>,
    movement: Vec<EntityMovementSyncPacket>,
    self_movement: Option<EntityMovementSyncPacket>,
    entity_data: Option<Vec<DataValue>>,
    attributes: Option<Vec<AttributeSnapshot>>,
    mob_effects: Vec<(i32, MobEffectSyncPacket)>,
    equipment: Option<Vec<EquipmentSlotItem>>,
    entity_link: Option<CSetEntityLink>,
}

#[derive(Debug, Clone, Copy)]
struct EntityTrackingRange {
    block_radius: f64,
}

impl EntityTrackingRange {
    fn from_client_chunk_range(client_chunk_range: i32) -> Self {
        Self {
            block_radius: f64::from(client_chunk_range) * BLOCKS_PER_CHUNK,
        }
    }

    fn is_disabled(self) -> bool {
        self.block_radius <= 0.0
    }

    fn visible_radius(self, player_view_distance: u8) -> f64 {
        self.block_radius
            .min(f64::from(player_view_distance) * BLOCKS_PER_CHUNK)
    }
}

impl Default for EntityTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl EntityTracker {
    /// Creates a new empty entity tracker.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entities: scc::HashMap::new(),
            chunk_entities: scc::HashMap::new(),
            player_entities: scc::HashMap::new(),
        }
    }

    /// Starts tracking an entity.
    ///
    /// Sends spawn packets to players already watching the entity chunk and
    /// inside the vanilla tracking range.
    ///
    /// The `get_players_in_chunk` callback should return player IDs in a given chunk
    /// (typically from `PlayerAreaMap::get_tracking_players`).
    /// The `get_player` callback should resolve a player ID to a `Player` reference.
    ///
    /// # Panics
    /// Panics if the entity is removed or is already tracked.
    pub fn add(
        &self,
        entity: &SharedEntity,
        get_players_in_chunk: impl Fn(ChunkPos) -> Vec<i32>,
        get_player: impl Fn(i32) -> Option<Arc<Player>>,
    ) {
        assert!(
            !entity.is_removed(),
            "cannot add removed entity {} to tracker",
            entity.id()
        );

        let entity_id = entity.id();
        let tracking_range = EntityTrackingRange::from_client_chunk_range(
            entity.entity_type().client_tracking_range,
        );
        if tracking_range.is_disabled() {
            return;
        }

        let pos = entity.position();
        let registered_chunk = ChunkPos::from_entity_pos(pos);

        let players_to_notify = Self::visible_players_for_entity(
            entity_id,
            entity.as_ref(),
            registered_chunk,
            tracking_range,
            &get_players_in_chunk,
            &get_player,
        );
        let player_ids_to_notify: Vec<i32> = players_to_notify.iter().copied().collect();
        for &player_id in &player_ids_to_notify {
            self.index_seen(player_id, entity_id);
        }

        let tracked_entity = TrackedEntity {
            entity: Arc::downgrade(entity),
            server_entity: SyncMutex::new(ServerEntityMovementSyncState::new(
                pos,
                entity.velocity(),
                entity.on_ground(),
                entity.rotation(),
                entity.head_yaw(),
                entity.entity_type().update_interval,
                entity.entity_type().track_deltas,
            )),
            last_passenger_ids: SyncMutex::new(self.direct_tracked_passenger_ids(entity.as_ref())),
            last_leash_holder_id: SyncMutex::new(leash_holder_id(entity.as_ref())),
            tracking_range,
            registered_chunk,
            seen_by: SyncRwLock::new(players_to_notify),
        };

        assert!(
            self.entities.insert_sync(entity_id, tracked_entity).is_ok(),
            "entity {entity_id} is already tracked"
        );
        self.index_chunk(registered_chunk, entity_id);

        // Send spawn packets to all nearby players
        for player_id in player_ids_to_notify {
            if let Some(player) = get_player(player_id) {
                self.send_spawn_packets(entity, &player);
            }
        }
    }

    /// Stops tracking an entity and sends despawn to all tracking players.
    pub fn remove(&self, entity_id: i32, get_player: impl Fn(i32) -> Option<Arc<Player>>) {
        if let Some((_, tracked)) = self.entities.remove_sync(&entity_id) {
            self.unindex_entry(entity_id, &tracked);
            let entity = tracked.entity.upgrade();
            // Send despawn to all tracking players
            for player_id in tracked.seen_by.read().iter() {
                if let Some(player) = get_player(*player_id) {
                    if let Some(entity) = &entity
                        && let Some(packet) =
                            self.vehicle_passenger_packet_for_player(entity.as_ref(), *player_id)
                    {
                        player.send_packet(packet);
                    }
                    player.send_packet(CRemoveEntities::single(entity_id));
                    if let Some(entity) = &entity {
                        entity.stop_seen_by_player(&player);
                    }
                }
            }
        }
    }

    /// Refreshes the tracked-entity set for one player.
    ///
    /// Mirrors vanilla `TrackedEntity.updatePlayer`: each tracked entity checks
    /// whether the player tracks the entity chunk, passes the entity-specific
    /// broadcast predicate, and is inside the effective horizontal range.
    pub fn update_player(
        &self,
        player: &Player,
        view: &PlayerChunkView,
        is_chunk_sent: impl Fn(ChunkPos) -> bool,
    ) {
        let player_id = player.id();
        let player_pos = player.position();
        let player_view_distance = view.view_distance;

        let mut entities_to_despawn = Vec::new();
        let mut entities_to_spawn = Vec::new();
        let mut dead_entities = Vec::new();

        // Only entities in this player's view can become visible, and only entities the player
        // already sees can be despawned, so those two sets are all the candidates.
        let mut candidates = FxHashSet::default();
        view.for_each(|chunk| {
            self.chunk_entities
                .read_sync(&chunk, |_, ids| candidates.extend(ids.iter().copied()));
        });
        self.player_entities
            .read_sync(&player_id, |_, ids| candidates.extend(ids.iter().copied()));

        for entity_id in candidates {
            self.entities.read_sync(&entity_id, |_, tracked| {
                let in_view = view.contains(tracked.registered_chunk);
                if !in_view && !tracked.seen_by.read().contains(&player_id) {
                    return;
                }

                let Some(entity) = tracked.entity.upgrade() else {
                    dead_entities.push(entity_id);
                    return;
                };

                let visible = !entity.is_removed()
                    && entity_id != player_id
                    && in_view
                    && is_chunk_sent(tracked.registered_chunk)
                    && entity.broadcast_to_player(player)
                    && is_within_tracking_distance(
                        entity.position(),
                        player_pos,
                        effective_tracking_range(entity.as_ref(), tracked.tracking_range),
                        player_view_distance,
                    );

                // Players move every tick, so membership rarely changes: check under the read
                // lock and only take the write lock to change it.
                if tracked.seen_by.read().contains(&player_id) == visible {
                    return;
                }

                let mut despawn = false;
                {
                    let mut seen_by = tracked.seen_by.write();
                    if visible {
                        if seen_by.insert(player_id) {
                            self.index_seen(player_id, entity_id);
                            entities_to_spawn.push(Arc::clone(&entity));
                        }
                    } else if seen_by.remove(&player_id) {
                        self.unindex_seen(player_id, entity_id);
                        despawn = true;
                    }
                }

                if despawn {
                    let vehicle_packet =
                        self.vehicle_passenger_packet_for_player(entity.as_ref(), player_id);
                    entities_to_despawn.push((entity, vehicle_packet));
                }
            });
        }

        for (entity, vehicle_packet) in entities_to_despawn {
            if let Some(packet) = vehicle_packet {
                player.send_packet(packet);
            }
            player.send_packet(CRemoveEntities::single(entity.id()));
            entity.stop_seen_by_player(player);
        }

        for entity in entities_to_spawn {
            self.send_spawn_packets(&entity, player);
        }

        // Clean up dead entities we encountered
        for entity_id in dead_entities {
            self.remove_dead_entity(entity_id);
        }
    }

    /// Sends tracker-owned movement changes for all tracked entities.
    ///
    /// Mirrors vanilla `ChunkMap.tick` driving `ServerEntity.sendChanges`. Returns the item frames
    /// whose framed map is due to sync this pass (every tenth pass of each frame), which the
    /// world handles because map data lives outside the tracker.
    pub fn send_changes<
        Movement,
        SelfMovement,
        EntityData,
        Attributes,
        MobEffects,
        Equipment,
        Passengers,
        EntityLink,
    >(
        &self,
        get_players_in_chunk: impl Fn(ChunkPos) -> Vec<i32>,
        get_player: impl Fn(i32) -> Option<Arc<Player>> + Sync,
        mut senders: EntityChangeSenders<
            Movement,
            SelfMovement,
            EntityData,
            Attributes,
            MobEffects,
            Equipment,
            Passengers,
            EntityLink,
        >,
    ) -> Vec<SharedEntity>
    where
        Movement: FnMut(i32, EntityMovementSyncPacket),
        SelfMovement: FnMut(i32, EntityMovementSyncPacket),
        EntityData: FnMut(i32, Vec<DataValue>),
        Attributes: FnMut(i32, Vec<AttributeSnapshot>),
        MobEffects: FnMut(i32, MobEffectSyncPacket),
        Equipment: FnMut(i32, CSetEquipment),
        Passengers: FnMut(i32, CSetPassengers),
        EntityLink: FnMut(i32, CSetEntityLink),
    {
        let mut dead_entities = Vec::new();
        let mut entities_to_refresh = Vec::new();
        let mut passenger_packets_to_send = Vec::new();
        let mut packets_to_broadcast = Vec::new();
        let mut self_movement_packets = Vec::new();
        let mut entity_data_to_broadcast = Vec::new();
        let mut attributes_to_broadcast = Vec::new();
        let mut mob_effect_packets_to_send = Vec::new();
        let mut equipment_to_broadcast = Vec::new();
        let mut entity_links_to_broadcast = Vec::new();
        let mut framed_map_syncs = Vec::new();

        let mut entity_ids = Vec::with_capacity(self.entities.len());
        self.entities.iter_sync(|entity_id, _| {
            entity_ids.push(*entity_id);
            true
        });
        let collect = |entity_id: &i32| {
            self.entities
                .read_sync(entity_id, |_, tracked| {
                    self.collect_entity_changes(*entity_id, tracked, &get_player)
                })
                .unwrap_or_default()
        };
        // Each entity's changes only touch that entity and its own tracking state, so with
        // regionized ticking they are gathered on the tick pool. The packets still go out
        // below in entity order, as the sequential pass sends them.
        let all_changes: Vec<EntityChanges> = if regionized_ticking_enabled() {
            install_on_tick_pool(|| {
                entity_ids
                    .par_iter()
                    .with_min_len(TICK_POOL_MIN_SPLIT)
                    .map(collect)
                    .collect()
            })
        } else {
            entity_ids.iter().map(collect).collect()
        };

        for (entity_id, changes) in entity_ids.into_iter().zip(all_changes) {
            if changes.dead {
                dead_entities.push(entity_id);
                continue;
            }
            passenger_packets_to_send.extend(changes.passenger_packets);
            if changes.refresh_players {
                entities_to_refresh.push(entity_id);
            }
            framed_map_syncs.extend(changes.framed_map_sync);
            packets_to_broadcast.extend(
                changes
                    .movement
                    .into_iter()
                    .map(|packet| (entity_id, packet)),
            );
            if let Some(packet) = changes.self_movement {
                self_movement_packets.push((entity_id, packet));
            }
            if let Some(dirty_entity_data) = changes.entity_data {
                entity_data_to_broadcast.push((entity_id, dirty_entity_data));
            }
            if let Some(dirty_attributes) = changes.attributes {
                attributes_to_broadcast.push((entity_id, dirty_attributes));
            }
            mob_effect_packets_to_send.extend(changes.mob_effects);
            if let Some(dirty_equipment) = changes.equipment {
                equipment_to_broadcast.push((entity_id, dirty_equipment));
            }
            if let Some(packet) = changes.entity_link {
                entity_links_to_broadcast.push((entity_id, packet));
            }
        }

        for entity_id in dead_entities {
            self.remove_dead_entity(entity_id);
        }

        for (player_id, packet) in passenger_packets_to_send {
            (senders.passengers)(player_id, packet);
        }

        for entity_id in entities_to_refresh {
            self.refresh_entity_players(entity_id, &get_players_in_chunk, &get_player);
        }

        for (entity_id, packet) in packets_to_broadcast {
            (senders.movement)(entity_id, packet);
        }

        for (player_id, packet) in self_movement_packets {
            (senders.self_movement)(player_id, packet);
        }

        for (entity_id, dirty_entity_data) in entity_data_to_broadcast {
            (senders.entity_data)(entity_id, dirty_entity_data);
        }

        for (entity_id, dirty_equipment) in equipment_to_broadcast {
            (senders.equipment)(entity_id, CSetEquipment::new(entity_id, dirty_equipment));
        }

        for (entity_id, dirty_attributes) in attributes_to_broadcast {
            (senders.attributes)(entity_id, dirty_attributes);
        }

        for (player_id, packet) in mob_effect_packets_to_send {
            (senders.mob_effects)(player_id, packet);
        }

        for (entity_id, packet) in entity_links_to_broadcast {
            (senders.entity_link)(entity_id, packet);
        }

        framed_map_syncs
    }

    /// One tracked entity's part of [`Self::send_changes`]: updates its sync state and
    /// returns the packets it needs, without sending anything.
    #[expect(
        clippy::too_many_lines,
        reason = "entity tracker fanout mirrors vanilla sendChanges ordering"
    )]
    fn collect_entity_changes(
        &self,
        entity_id: i32,
        tracked: &TrackedEntity,
        get_player: &impl Fn(i32) -> Option<Arc<Player>>,
    ) -> EntityChanges {
        let mut changes = EntityChanges::default();
        let Some(entity) = tracked.entity.upgrade() else {
            changes.dead = true;
            return changes;
        };

        if entity.is_removed() {
            return changes;
        }

        entity.update_data_before_sync();

        let passenger_ids = self.direct_tracked_passenger_ids(entity.as_ref());
        {
            let mut last_passenger_ids = tracked.last_passenger_ids.lock();
            if *last_passenger_ids != passenger_ids {
                let changed_player_passenger_ids =
                    direct_player_passenger_delta(&last_passenger_ids, &passenger_ids, &get_player);
                let seen_by = tracked.seen_by.read();
                for player_id in seen_by.iter() {
                    if changed_player_passenger_ids.contains(player_id) {
                        continue;
                    }
                    changes.passenger_packets.push((
                        *player_id,
                        CSetPassengers::new(
                            entity_id,
                            self.direct_passenger_ids_seen_by_player(entity.as_ref(), *player_id),
                        ),
                    ));
                }
                *last_passenger_ids = passenger_ids;
                changes.refresh_players = true;
            }
        }

        let mut server_entity = tracked.server_entity.lock();
        if entity.as_item_frame().is_some() && server_entity.tick_count() % 10 == 0 {
            changes.framed_map_sync = Some(Arc::clone(&entity));
        }
        let dirty_entity_data = entity.pack_dirty_entity_data();
        let has_dirty_entity_data = dirty_entity_data.is_some();
        let result = server_entity.record_send_changes(ServerEntityMovementSyncUpdate {
            entity_type: entity.entity_type(),
            vertical_collision: entity.vertical_collision(),
            horizontal_collision: entity.horizontal_collision(),
            requires_precise_position: entity.base().requires_precise_position(),
            entity_id,
            is_passenger: entity.is_passenger(),
            position: entity.position(),
            velocity: entity.velocity(),
            body_rotation: entity.rotation(),
            head_yaw: entity.head_yaw(),
            on_ground: entity.on_ground(),
            needs_velocity_sync: entity.needs_velocity_sync(),
            has_dirty_entity_data,
            force_velocity_sync: entity.forces_fall_flying_velocity_sync(),
        });
        drop(server_entity);
        if result.should_clear_velocity_sync() {
            entity.clear_velocity_sync();
        }
        result.for_each_packet(|packet| changes.movement.push(packet));
        if entity.hurt_marked() {
            let velocity = entity.velocity();
            changes
                .movement
                .push(EntityMovementSyncPacket::from(CSetEntityMotion::new(
                    entity_id, velocity,
                )));
            if entity.entity_type() == &vanilla_entities::PLAYER {
                changes.self_movement = Some(EntityMovementSyncPacket::from(
                    CSetEntityMotion::new(entity_id, velocity),
                ));
            }
            entity.clear_hurt_mark();
        }
        if let Some(dirty_entity_data) = dirty_entity_data {
            changes.entity_data = Some(dirty_entity_data);
        }
        if let Some(living) = entity.as_living_entity() {
            let dirty_attributes = living.drain_dirty_syncable_attributes();
            if !dirty_attributes.is_empty() {
                changes.attributes = Some(dirty_attributes);
            }
            let dirty_mob_effects = living.drain_dirty_mob_effects();
            if !dirty_mob_effects.is_empty() {
                let mut recipient_ids = FxHashSet::default();
                if entity.entity_type() == &vanilla_entities::PLAYER
                    && get_player(entity_id).is_some()
                {
                    recipient_ids.insert(entity_id);
                }
                for passenger in entity.passengers() {
                    let passenger_id = passenger.id();
                    if passenger.entity_type() == &vanilla_entities::PLAYER
                        && get_player(passenger_id).is_some()
                    {
                        recipient_ids.insert(passenger_id);
                    }
                }
                for recipient_id in recipient_ids {
                    for change in &dirty_mob_effects {
                        changes.mob_effects.push((
                            recipient_id,
                            change.packet(entity_id, recipient_id == entity_id),
                        ));
                    }
                }
            }
            let dirty_equipment = living.drain_dirty_equipment();
            if !dirty_equipment.is_empty() {
                changes.equipment = Some(dirty_equipment);
            }
        }

        let leash_holder_id = leash_holder_id(entity.as_ref());
        {
            let mut last_leash_holder_id = tracked.last_leash_holder_id.lock();
            if *last_leash_holder_id != leash_holder_id {
                changes.entity_link = Some(CSetEntityLink::new(
                    entity_id,
                    leash_holder_id.unwrap_or_default(),
                ));
                *last_leash_holder_id = leash_holder_id;
            }
        }

        changes
    }

    /// Removes a player from every entity pairing and despawns those entities from their client.
    ///
    /// Mirrors vanilla `ChunkMap.removeEntity` calling `TrackedEntity.removePlayer` for each
    /// tracked entity before the player leaves the world.
    pub fn on_player_leave(&self, player: &Player) {
        let player_id = player.id();
        let mut entities_to_despawn = Vec::new();
        let mut dead_entities = Vec::new();

        self.entities.iter_sync(|entity_id, tracked| {
            if tracked.seen_by.write().remove(&player_id) {
                entities_to_despawn.push((*entity_id, tracked.entity.upgrade()));
            }
            if tracked.entity.strong_count() == 0 {
                dead_entities.push(*entity_id);
            }
            true
        });
        let _ = self.player_entities.remove_sync(&player_id);

        for (entity_id, entity) in entities_to_despawn {
            player.send_packet(CRemoveEntities::single(entity_id));
            if let Some(entity) = entity {
                entity.stop_seen_by_player(player);
            }
        }

        for entity_id in dead_entities {
            self.remove_dead_entity(entity_id);
        }
    }

    /// Updates an entity's current chunk and visible players after a section move.
    ///
    /// Vanilla refreshes tracked players when an entity's section position changes.
    /// The old and new chunks may be the same for purely vertical section moves.
    pub fn on_entity_section_change(
        &self,
        entity_id: i32,
        _old_chunk: ChunkPos,
        new_chunk: ChunkPos,
        get_players_in_chunk: impl Fn(ChunkPos) -> Vec<i32>,
        get_player: impl Fn(i32) -> Option<Arc<Player>>,
    ) {
        let mut players_to_remove = Vec::new();
        let mut players_to_add = Vec::new();
        let mut entity_to_spawn = None;

        self.entities.update_sync(&entity_id, |_, tracked| {
            if tracked.registered_chunk != new_chunk {
                self.unindex_chunk(tracked.registered_chunk, entity_id);
                self.index_chunk(new_chunk, entity_id);
                tracked.registered_chunk = new_chunk;
            }

            let Some(entity) = tracked.entity.upgrade() else {
                return;
            };

            let new_seen_by = Self::visible_players_for_entity(
                entity_id,
                entity.as_ref(),
                new_chunk,
                tracked.tracking_range,
                &get_players_in_chunk,
                &get_player,
            );

            let mut seen_by = tracked.seen_by.write();
            for &player_id in seen_by.difference(&new_seen_by) {
                self.unindex_seen(player_id, entity_id);
                players_to_remove.push(player_id);
            }
            for &player_id in new_seen_by.difference(&seen_by) {
                self.index_seen(player_id, entity_id);
                players_to_add.push(player_id);
            }
            *seen_by = new_seen_by;
            entity_to_spawn = Some(entity);
        });

        let Some(entity) = entity_to_spawn else {
            return;
        };

        for player_id in players_to_remove {
            if let Some(player) = get_player(player_id) {
                if let Some(packet) =
                    self.vehicle_passenger_packet_for_player(entity.as_ref(), player_id)
                {
                    player.send_packet(packet);
                }
                player.send_packet(CRemoveEntities::single(entity_id));
                entity.stop_seen_by_player(&player);
            }
        }

        for player_id in players_to_add {
            if let Some(player) = get_player(player_id) {
                self.send_spawn_packets(&entity, &player);
            }
        }
    }

    fn refresh_entity_players(
        &self,
        entity_id: i32,
        get_players_in_chunk: &impl Fn(ChunkPos) -> Vec<i32>,
        get_player: &impl Fn(i32) -> Option<Arc<Player>>,
    ) {
        let mut players_to_remove = Vec::new();
        let mut players_to_add = Vec::new();
        let mut entity_to_spawn = None;

        self.entities.update_sync(&entity_id, |_, tracked| {
            let Some(entity) = tracked.entity.upgrade() else {
                return;
            };

            let new_seen_by = Self::visible_players_for_entity(
                entity_id,
                entity.as_ref(),
                tracked.registered_chunk,
                tracked.tracking_range,
                get_players_in_chunk,
                get_player,
            );

            let mut seen_by = tracked.seen_by.write();
            for &player_id in seen_by.difference(&new_seen_by) {
                self.unindex_seen(player_id, entity_id);
                players_to_remove.push(player_id);
            }
            for &player_id in new_seen_by.difference(&seen_by) {
                self.index_seen(player_id, entity_id);
                players_to_add.push(player_id);
            }
            *seen_by = new_seen_by;
            entity_to_spawn = Some(entity);
        });

        let Some(entity) = entity_to_spawn else {
            return;
        };

        for player_id in players_to_remove {
            if let Some(player) = get_player(player_id) {
                if let Some(packet) =
                    self.vehicle_passenger_packet_for_player(entity.as_ref(), player_id)
                {
                    player.send_packet(packet);
                }
                player.send_packet(CRemoveEntities::single(entity_id));
                entity.stop_seen_by_player(&player);
            }
        }

        for player_id in players_to_add {
            if let Some(player) = get_player(player_id) {
                self.send_spawn_packets(&entity, &player);
            }
        }
    }

    /// Gets the number of tracked entities.
    #[must_use]
    pub fn count(&self) -> usize {
        self.entities.len()
    }

    /// Vanilla `Entity.syncPacketPositionCodec` for a tracked entity: the
    /// next delta packets are measured from `position`.
    pub fn sync_position_codec(&self, entity_id: i32, position: DVec3) {
        self.entities.read_sync(&entity_id, |_, tracked| {
            tracked.server_entity.lock().sync_position_codec(position);
        });
    }

    /// Returns players currently tracking an entity.
    #[must_use]
    pub fn tracking_player_ids(&self, entity_id: i32) -> Vec<i32> {
        self.entities
            .read_sync(&entity_id, |_, tracked| {
                tracked.seen_by.read().iter().copied().collect()
            })
            .unwrap_or_default()
    }

    fn remove_dead_entity(&self, entity_id: i32) {
        // Note: We don't send despawn packets here because the players
        // will get updated via player view changes or explicit removals.
        if let Some((_, tracked)) = self.entities.remove_sync(&entity_id) {
            self.unindex_entry(entity_id, &tracked);
        }
    }

    fn unindex_entry(&self, entity_id: i32, tracked: &TrackedEntity) {
        self.unindex_chunk(tracked.registered_chunk, entity_id);
        for &player_id in tracked.seen_by.read().iter() {
            self.unindex_seen(player_id, entity_id);
        }
    }

    fn index_chunk(&self, chunk: ChunkPos, entity_id: i32) {
        self.chunk_entities
            .entry_sync(chunk)
            .or_default()
            .get_mut()
            .insert(entity_id);
    }

    fn unindex_chunk(&self, chunk: ChunkPos, entity_id: i32) {
        let _ = self.chunk_entities.remove_if_sync(&chunk, |ids| {
            ids.remove(&entity_id);
            ids.is_empty()
        });
    }

    fn index_seen(&self, player_id: i32, entity_id: i32) {
        self.player_entities
            .entry_sync(player_id)
            .or_default()
            .get_mut()
            .insert(entity_id);
    }

    fn unindex_seen(&self, player_id: i32, entity_id: i32) {
        let _ = self.player_entities.remove_if_sync(&player_id, |ids| {
            ids.remove(&entity_id);
            ids.is_empty()
        });
    }

    fn visible_players_for_entity(
        entity_id: i32,
        entity: &dyn Entity,
        entity_chunk: ChunkPos,
        tracking_range: EntityTrackingRange,
        get_players_in_chunk: &impl Fn(ChunkPos) -> Vec<i32>,
        get_player: &impl Fn(i32) -> Option<Arc<Player>>,
    ) -> FxHashSet<i32> {
        let entity_pos = entity.position();
        let tracking_range = effective_tracking_range(entity, tracking_range);
        let mut players = FxHashSet::default();
        if entity.is_removed() {
            return players;
        }

        for player_id in get_players_in_chunk(entity_chunk) {
            if player_id == entity_id {
                continue;
            }

            let Some(player) = get_player(player_id) else {
                continue;
            };

            if entity.broadcast_to_player(&player)
                && is_within_tracking_distance(
                    entity_pos,
                    player.position(),
                    tracking_range,
                    player.view_distance(),
                )
            {
                players.insert(player_id);
            }
        }

        players
    }

    fn send_spawn_packets(&self, entity: &SharedEntity, player: &Player) {
        let entity_id = entity.id();
        self.spawn_pairing(entity, player.id())
            .send_to(entity_id, player);
        entity.start_seen_by_player(player);
    }

    fn spawn_pairing(&self, entity: &SharedEntity, player_id: i32) -> EntitySpawnPairing {
        EntitySpawnPairing::from_entity(
            entity,
            self.passenger_pairing_packets_for_player(entity.as_ref(), player_id),
        )
    }

    fn passenger_pairing_packets_for_player(
        &self,
        entity: &dyn Entity,
        player_id: i32,
    ) -> Vec<CSetPassengers> {
        let mut packets = Vec::new();
        let passenger_ids = self.direct_passenger_ids_seen_by_player(entity, player_id);
        if !passenger_ids.is_empty() {
            packets.push(CSetPassengers::new(entity.id(), passenger_ids));
        }

        if let Some(vehicle) = entity.vehicle()
            && self.entity_seen_by_player(vehicle.id(), player_id)
        {
            packets.push(CSetPassengers::new(
                vehicle.id(),
                self.direct_passenger_ids_seen_by_player(vehicle.as_ref(), player_id),
            ));
        }

        packets
    }

    fn vehicle_passenger_packet_for_player(
        &self,
        entity: &dyn Entity,
        player_id: i32,
    ) -> Option<CSetPassengers> {
        let vehicle = entity.vehicle()?;
        if !self.entity_seen_by_player(vehicle.id(), player_id) {
            return None;
        }
        Some(CSetPassengers::new(
            vehicle.id(),
            self.direct_passenger_ids_seen_by_player(vehicle.as_ref(), player_id),
        ))
    }

    fn direct_tracked_passenger_ids(&self, entity: &dyn Entity) -> Vec<i32> {
        entity
            .passengers()
            .into_iter()
            .filter(|passenger| self.is_entity_tracked(passenger.id()))
            .map(|passenger| passenger.id())
            .collect()
    }

    fn direct_passenger_ids_seen_by_player(&self, entity: &dyn Entity, player_id: i32) -> Vec<i32> {
        entity
            .passengers()
            .into_iter()
            .filter(|passenger| {
                passenger.id() == player_id || self.entity_seen_by_player(passenger.id(), player_id)
            })
            .map(|passenger| passenger.id())
            .collect()
    }

    fn is_entity_tracked(&self, entity_id: i32) -> bool {
        self.entities.read_sync(&entity_id, |_, _| ()).is_some()
    }

    fn entity_seen_by_player(&self, entity_id: i32, player_id: i32) -> bool {
        self.entities
            .read_sync(&entity_id, |_, tracked| {
                tracked.seen_by.read().contains(&player_id)
            })
            .unwrap_or(false)
    }
}

fn leash_holder_id(entity: &dyn Entity) -> Option<i32> {
    entity
        .as_leashable()
        .and_then(Leashable::leash_holder)
        .map(|holder| holder.id())
}

fn is_within_tracking_distance(
    entity_pos: DVec3,
    player_pos: DVec3,
    tracking_range: EntityTrackingRange,
    player_view_distance: u8,
) -> bool {
    let visible_radius = tracking_range.visible_radius(player_view_distance);
    let x = player_pos.x - entity_pos.x;
    let z = player_pos.z - entity_pos.z;
    x * x + z * z <= visible_radius * visible_radius
}

fn effective_tracking_range(
    entity: &dyn Entity,
    base_range: EntityTrackingRange,
) -> EntityTrackingRange {
    let mut range = base_range;
    let mut visited = FxHashSet::default();
    visited.insert(entity.id());
    add_passenger_tracking_ranges(entity, &mut range, &mut visited);
    range
}

fn add_passenger_tracking_ranges(
    entity: &dyn Entity,
    range: &mut EntityTrackingRange,
    visited: &mut FxHashSet<i32>,
) {
    for passenger in entity.passengers() {
        if !visited.insert(passenger.id()) {
            continue;
        }
        let passenger_range = EntityTrackingRange::from_client_chunk_range(
            passenger.entity_type().client_tracking_range,
        );
        range.block_radius = range.block_radius.max(passenger_range.block_radius);
        add_passenger_tracking_ranges(passenger.as_ref(), range, visited);
    }
}

struct EntitySpawnPairing {
    spawn_packet: CAddEntity,
    entity_data: Vec<DataValue>,
    attributes: Vec<AttributeSnapshot>,
    equipment: Vec<EquipmentSlotItem>,
    passenger_packets: Vec<CSetPassengers>,
    entity_link_packet: Option<CSetEntityLink>,
}

impl EntitySpawnPairing {
    fn from_entity(entity: &SharedEntity, passenger_packets: Vec<CSetPassengers>) -> Self {
        entity.update_data_before_sync();

        let pos = entity.spawn_position();
        let vel = entity.velocity();
        let (yaw, pitch) = entity.rotation();
        let head_yaw = entity.head_yaw();
        let entity_type_id = entity.entity_type().id() as i32;

        // Convert rotation from degrees to protocol byte format (256th of a full rotation)
        // Uses to_angle_byte which matches vanilla's Mth.packDegrees
        let x_rot = to_angle_byte(pitch);
        let y_rot = to_angle_byte(yaw);
        let head_y_rot = to_angle_byte(head_yaw);

        let (attributes, equipment) = entity.as_living_entity().map_or_else(
            || (Vec::new(), Vec::new()),
            |living| {
                (
                    living.pack_syncable_attributes(),
                    living.pack_all_equipment(),
                )
            },
        );

        Self {
            spawn_packet: CAddEntity {
                id: entity.id(),
                uuid: entity.uuid(),
                entity_type: entity_type_id,
                position: pos,
                velocity: vel,
                x_rot,
                y_rot,
                head_y_rot,
                data: entity.spawn_data(),
            },
            entity_data: entity.pack_all_entity_data(),
            attributes,
            equipment,
            passenger_packets,
            entity_link_packet: leash_holder_id(entity.as_ref())
                .map(|holder_id| CSetEntityLink::new(entity.id(), holder_id)),
        }
    }

    fn send_to(self, entity_id: i32, player: &Player) {
        player.send_bundle(|bundle| {
            bundle.add(self.spawn_packet);

            if !self.entity_data.is_empty() {
                bundle.add(CSetEntityData::new(entity_id, self.entity_data));
            }

            if !self.attributes.is_empty() {
                bundle.add(CUpdateAttributes::new(entity_id, self.attributes));
            }

            if !self.equipment.is_empty() {
                bundle.add(CSetEquipment::new(entity_id, self.equipment));
            }

            for packet in self.passenger_packets {
                bundle.add(packet);
            }

            if let Some(packet) = self.entity_link_packet {
                bundle.add(packet);
            }
        });
    }
}

fn direct_player_passenger_delta(
    old_passenger_ids: &[i32],
    new_passenger_ids: &[i32],
    get_player: &impl Fn(i32) -> Option<Arc<Player>>,
) -> Vec<i32> {
    let old_passenger_ids = old_passenger_ids.iter().copied().collect::<FxHashSet<_>>();
    let new_passenger_ids = new_passenger_ids.iter().copied().collect::<FxHashSet<_>>();
    old_passenger_ids
        .symmetric_difference(&new_passenger_ids)
        .copied()
        .filter(|entity_id| get_player(*entity_id).is_some())
        .collect()
}

#[cfg(test)]
mod tests;
