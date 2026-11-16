//! Vanilla `TransportItemsBetweenContainers`: a mob with an empty hand takes
//! up to 16 items from the nearest source chest and carries them to the
//! nearest destination chest that is empty or already holds that item.

use std::sync::Arc;

use glam::DVec3;
use rustc_hash::FxHashSet;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, ChestType, Direction};
use steel_registry::item_stack::ItemStack;
use steel_utils::types::GlobalPos;
use steel_utils::{BlockPos, BlockStateId, ChunkPos, Downcast as _, WorldAabb};

use crate::behavior::blocks::{chest_container, connected_direction, is_chest_blocked_at};
use crate::block_entity::SharedBlockEntity;
use crate::block_entity::entities::ChestBlockEntity;
use crate::entity::ai::brain::MemoryStatus;
use crate::entity::ai::brain::behavior::Behavior;
use crate::entity::ai::brain::behaviors::utils::set_walk_and_look_target_memories;
use crate::entity::ai::brain::memory::MemoryRequirement;
use crate::entity::ai::brain::memory_types::{
    IS_PANICKING, LOOK_TARGET, PositionTracker, TRANSPORT_ITEMS_COOLDOWN_TICKS,
    UNREACHABLE_TRANSPORT_BLOCK_POSITIONS, VISITED_BLOCK_POSITIONS, WALK_TARGET,
};
use crate::entity::ai::navigation::NavigationKind;
use crate::entity::ai::path::Path;
use crate::entity::leash::Leashable;
use crate::entity::{EquipmentSlot, LivingTravelInput, PathfinderMob};
use crate::inventory::container::Container;
use crate::inventory::lock::{ContainerLockGuard, ContainerRef};
use crate::world::World;
use crate::world::{ClipBlockShape, ClipFluid};

/// Vanilla `TARGET_INTERACTION_TIME`.
pub const TARGET_INTERACTION_TIME: i32 = 60;
/// Vanilla `VISITED_POSITIONS_MEMORY_TIME`.
const VISITED_POSITIONS_MEMORY_TIME: i64 = 6000;
/// Vanilla `TRANSPORTED_ITEM_MAX_STACK_SIZE`.
const TRANSPORTED_ITEM_MAX_STACK_SIZE: i32 = 16;
/// Vanilla `MAX_VISITED_POSITIONS`.
const MAX_VISITED_POSITIONS: usize = 10;
/// Vanilla `MAX_UNREACHABLE_POSITIONS`.
const MAX_UNREACHABLE_POSITIONS: usize = 50;
/// Vanilla `PASSENGER_MOB_TARGET_SEARCH_DISTANCE`.
const PASSENGER_MOB_TARGET_SEARCH_DISTANCE: i32 = 1;
/// Vanilla `IDLE_COOLDOWN`.
const IDLE_COOLDOWN: i32 = 140;
/// Vanilla `CLOSE_ENOUGH_TO_START_QUEUING_DISTANCE`.
const CLOSE_ENOUGH_TO_START_QUEUING_DISTANCE: f64 = 3.0;
/// Vanilla `CLOSE_ENOUGH_TO_START_INTERACTING_WITH_TARGET_DISTANCE`.
const CLOSE_ENOUGH_TO_START_INTERACTING_DISTANCE: f64 = 0.5;
/// Vanilla `CLOSE_ENOUGH_TO_START_INTERACTING_WITH_TARGET_PATH_END_DISTANCE`.
const CLOSE_ENOUGH_TO_START_INTERACTING_PATH_END_DISTANCE: f64 = 1.0;
/// Vanilla `CLOSE_ENOUGH_TO_CONTINUE_INTERACTING_WITH_TARGET`.
const CLOSE_ENOUGH_TO_CONTINUE_INTERACTING_DISTANCE: f64 = 2.0;

const ENTRY: [MemoryRequirement; 4] = [
    MemoryRequirement::new(&VISITED_BLOCK_POSITIONS, MemoryStatus::Registered),
    MemoryRequirement::new(
        &UNREACHABLE_TRANSPORT_BLOCK_POSITIONS,
        MemoryStatus::Registered,
    ),
    MemoryRequirement::new(&TRANSPORT_ITEMS_COOLDOWN_TICKS, MemoryStatus::ValueAbsent),
    MemoryRequirement::new(&IS_PANICKING, MemoryStatus::ValueAbsent),
];

/// What the mob does at the chest it reached. Vanilla
/// `ContainerInteractionState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::enum_variant_names,
    reason = "the variants keep vanilla's PICKUP_ITEM / PLACE_NO_ITEM names"
)]
pub enum ContainerInteractionState {
    /// Taking items from a source chest with items.
    PickupItem,
    /// Reaching into an empty source chest.
    PickupNoItem,
    /// Putting items into a destination chest that takes them.
    PlaceItem,
    /// Holding items over a destination chest that does not take them.
    PlaceNoItem,
}

/// Vanilla `TransportItemState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransportItemState {
    Traveling,
    Queuing,
    Interacting,
}

/// A chest the mob walks to. Vanilla `TransportItemTarget`.
#[derive(Clone)]
pub struct TransportItemTarget {
    /// The chest's position.
    pub pos: BlockPos,
    /// The chest's containers, both halves of a double chest.
    pub containers: Vec<ContainerRef>,
    /// The chest's block entity when it was picked.
    pub block_entity: SharedBlockEntity,
    /// The chest's block state when it was picked.
    pub state: BlockStateId,
}

impl TransportItemTarget {
    /// Vanilla `tryCreatePossibleTarget(blockEntity, level)`: chests offer
    /// their double-chest container, other block entities their own.
    #[must_use]
    pub fn from_block_entity(block_entity: SharedBlockEntity, world: &World) -> Option<Self> {
        let pos = block_entity.get_block_pos();
        let state = block_entity.get_block_state();
        let containers = match chest_container(state, world, pos, false) {
            Some(containers) => containers,
            None => vec![block_entity.container_ref()?],
        };
        if containers.is_empty() {
            return None;
        }
        Some(Self {
            pos,
            containers,
            block_entity,
            state,
        })
    }

    /// Vanilla `tryCreatePossibleTarget(blockPos, level)`.
    #[must_use]
    pub fn at(pos: BlockPos, world: &World) -> Option<Self> {
        Self::from_block_entity(world.get_block_entity(pos)?, world)
    }

    /// The chest's block entity, if it is one. Vanilla
    /// `blockEntity() instanceof ChestBlockEntity`.
    #[must_use]
    pub fn chest(&self) -> Option<&ChestBlockEntity> {
        self.block_entity.downcast_ref::<ChestBlockEntity>()
    }
}

/// Reacts to the interaction with the reached chest each tick, with the
/// ticks spent at it so far.
pub type OnTargetReached<E> =
    fn(&E, &Arc<World>, &TransportItemTarget, ContainerInteractionState, i32);

/// Vanilla `TransportItemsBetweenContainers`.
pub struct TransportItemsBetweenContainers<E: ?Sized> {
    speed_modifier: f32,
    horizontal_search_distance: i32,
    vertical_search_distance: i32,
    source_block_type: fn(BlockStateId) -> bool,
    destination_block_type: fn(BlockStateId) -> bool,
    should_queue_for_target: fn(&World, &TransportItemTarget) -> bool,
    on_start_traveling: fn(&E),
    on_target_interaction: OnTargetReached<E>,
    target: Option<TransportItemTarget>,
    state: TransportItemState,
    interaction_state: Option<ContainerInteractionState>,
    ticks_since_reaching_target: i32,
}

impl<E: PathfinderMob + ?Sized> TransportItemsBetweenContainers<E> {
    /// Carries items from chests matching `source_block_type` to chests
    /// matching `destination_block_type` within the search distances.
    #[must_use]
    #[expect(clippy::too_many_arguments, reason = "mirrors the vanilla constructor")]
    pub const fn new(
        speed_modifier: f32,
        source_block_type: fn(BlockStateId) -> bool,
        destination_block_type: fn(BlockStateId) -> bool,
        horizontal_search_distance: i32,
        vertical_search_distance: i32,
        on_target_interaction: OnTargetReached<E>,
        on_start_traveling: fn(&E),
        should_queue_for_target: fn(&World, &TransportItemTarget) -> bool,
    ) -> Self {
        Self {
            speed_modifier,
            horizontal_search_distance,
            vertical_search_distance,
            source_block_type,
            destination_block_type,
            should_queue_for_target,
            on_start_traveling,
            on_target_interaction,
            target: None,
            state: TransportItemState::Traveling,
            interaction_state: None,
            ticks_since_reaching_target: 0,
        }
    }

    /// Vanilla `updateInvalidTarget`: picks a new target when the current one
    /// is gone; returns whether it had to.
    fn update_invalid_target(&mut self, world: &Arc<World>, body: &E) -> bool {
        if self.has_valid_target(world, body) {
            return false;
        }
        self.stop_targeting_current_target(body);
        if let Some(target) = self.transport_target(world, body) {
            let pos = target.pos;
            self.target = Some(target);
            self.on_start_traveling(body);
            self.set_visited_block_pos(body, world, pos);
        } else {
            self.enter_cooldown_after_no_matching_target_found(body);
        }
        true
    }

    /// Vanilla `onQueuingForTarget`.
    fn on_queuing_for_target(&mut self, target: &TransportItemTarget, world: &World, body: &E) {
        if !self.is_another_mob_interacting_with_target(target, world) {
            self.resume_traveling(body);
        }
    }

    /// Vanilla `onTravelToTarget`.
    fn on_travel_to_target(&mut self, target: &TransportItemTarget, world: &World, body: &E) {
        let center = center_pos(body);
        if is_within_target_distance(CLOSE_ENOUGH_TO_START_QUEUING_DISTANCE, target, body, center)
            && self.is_another_mob_interacting_with_target(target, world)
        {
            self.start_queuing(body);
        } else if is_within_target_distance(interaction_range(body), target, body, center) {
            self.start_on_reached_target_interaction(target, body);
        } else {
            self.walk_towards_target(body);
        }
    }

    /// Vanilla `onReachedTarget`.
    fn on_reached_target(&mut self, target: &TransportItemTarget, world: &Arc<World>, body: &E) {
        if !is_within_target_distance(
            CLOSE_ENOUGH_TO_CONTINUE_INTERACTING_DISTANCE,
            target,
            body,
            center_pos(body),
        ) {
            self.on_start_traveling(body);
            return;
        }
        self.ticks_since_reaching_target += 1;
        self.on_target_interaction(target, world, body);
        if self.ticks_since_reaching_target >= TARGET_INTERACTION_TIME {
            unpack_loot(&target.containers);
            if is_picking_up_items(body) {
                if matches_getting_items_requirement(&target.containers) {
                    self.pick_up_items(body, &target.containers);
                } else {
                    self.stop_targeting_current_target(body);
                }
            } else if matches_leaving_items_requirement(body, &target.containers) {
                self.put_down_item(body, &target.containers);
            } else {
                self.stop_targeting_current_target(body);
            }
            self.on_start_traveling(body);
        }
    }

    /// Vanilla `startQueuing`.
    fn start_queuing(&mut self, body: &E) {
        stop_in_place(body);
        self.state = TransportItemState::Queuing;
    }

    /// Vanilla `resumeTraveling`.
    fn resume_traveling(&mut self, body: &E) {
        self.state = TransportItemState::Traveling;
        self.walk_towards_target(body);
    }

    /// Vanilla `walkTowardsTarget`.
    fn walk_towards_target(&self, body: &E) {
        if let Some(target) = &self.target {
            set_walk_and_look_target_memories(
                body,
                PositionTracker::block(target.pos),
                self.speed_modifier,
                0,
            );
        }
    }

    /// Vanilla `startOnReachedTargetInteraction`: decides what happens at the
    /// chest from what it holds right now.
    fn start_on_reached_target_interaction(&mut self, target: &TransportItemTarget, body: &E) {
        unpack_loot(&target.containers);
        self.interaction_state = Some(if is_picking_up_items(body) {
            if matches_getting_items_requirement(&target.containers) {
                ContainerInteractionState::PickupItem
            } else {
                ContainerInteractionState::PickupNoItem
            }
        } else if matches_leaving_items_requirement(body, &target.containers) {
            ContainerInteractionState::PlaceItem
        } else {
            ContainerInteractionState::PlaceNoItem
        });
        self.state = TransportItemState::Interacting;
    }

    /// Vanilla `onStartTraveling`.
    fn on_start_traveling(&mut self, body: &E) {
        (self.on_start_traveling)(body);
        self.state = TransportItemState::Traveling;
        self.interaction_state = None;
        self.ticks_since_reaching_target = 0;
    }

    /// Vanilla `onTargetInteraction`: looks at the chest, stands still and
    /// lets the mob react.
    fn on_target_interaction(&self, target: &TransportItemTarget, world: &Arc<World>, body: &E) {
        if let Some(memories) = body.brain_memories() {
            memories
                .lock()
                .set(&LOOK_TARGET, Some(PositionTracker::block(target.pos)));
        }
        stop_in_place(body);
        if let Some(state) = self.interaction_state {
            (self.on_target_interaction)(
                body,
                world,
                target,
                state,
                self.ticks_since_reaching_target,
            );
        }
    }

    /// Vanilla `getTransportTarget`: the closest wanted chest in loaded
    /// chunks around the mob.
    fn transport_target(&self, world: &World, body: &E) -> Option<TransportItemTarget> {
        let search_area = self.target_search_area(body);
        let (visited, unreachable) = remembered_positions(body);
        let center = ChunkPos::from_block_pos(body.block_position());
        let radius = self.horizontal_search_distance(body).div_euclid(16) + 1;
        let position = body.position();
        let mut target = None;
        let mut closest_distance = f64::from(f32::MAX);
        // Vanilla `ChunkPos.rangeClosed` walks x fastest.
        for z in center.0.y - radius..=center.0.y + radius {
            for x in center.0.x - radius..=center.0.x + radius {
                let block_entities = world
                    .chunk_map
                    .with_full_chunk(ChunkPos::new(x, z), |chunk| chunk.get_block_entities())
                    .unwrap_or_default();
                for block_entity in block_entities {
                    if block_entity.downcast_ref::<ChestBlockEntity>().is_none() {
                        continue;
                    }
                    let distance = dist_to_center_sqr(block_entity.get_block_pos(), position);
                    if distance < closest_distance
                        && let Some(valid) = self.target_valid_to_pick(
                            body,
                            world,
                            block_entity,
                            &visited,
                            &unreachable,
                            search_area,
                        )
                    {
                        target = Some(valid);
                        closest_distance = distance;
                    }
                }
            }
        }
        target
    }

    /// Vanilla `isTargetValidToPick`.
    fn target_valid_to_pick(
        &self,
        body: &E,
        world: &World,
        block_entity: SharedBlockEntity,
        visited: &FxHashSet<GlobalPos>,
        unreachable: &FxHashSet<GlobalPos>,
        search_area: WorldAabb,
    ) -> Option<TransportItemTarget> {
        let pos = block_entity.get_block_pos();
        if !search_area.contains_xyz(f64::from(pos.x()), f64::from(pos.y()), f64::from(pos.z())) {
            return None;
        }
        let target = TransportItemTarget::from_block_entity(block_entity, world)?;
        let valid = self.is_wanted_block(body, target.state)
            && !is_position_already_visited(visited, unreachable, &target, world)
            && !target.chest().is_some_and(ChestBlockEntity::is_locked);
        valid.then_some(target)
    }

    /// Vanilla `hasValidTarget`.
    fn has_valid_target(&mut self, world: &World, body: &E) -> bool {
        let Some(target) = &self.target else {
            return false;
        };
        let of_valid_type =
            self.is_wanted_block(body, target.state) && target_has_not_changed(world, target);
        if !of_valid_type || is_chest_blocked_at(world, target.pos) {
            return false;
        }
        if self.state != TransportItemState::Traveling
            || has_valid_traveling_path(world, target, body)
        {
            return true;
        }
        let pos = target.pos;
        self.mark_visited_block_pos_as_unreachable(body, world, pos);
        false
    }

    /// Vanilla `getTargetSearchArea`.
    fn target_search_area(&self, body: &E) -> WorldAabb {
        let horizontal = f64::from(self.horizontal_search_distance(body));
        let vertical = f64::from(self.vertical_search_distance(body));
        let pos = body.block_position();
        let x = f64::from(pos.x());
        let y = f64::from(pos.y());
        let z = f64::from(pos.z());
        WorldAabb::new(x, y, z, x + 1.0, y + 1.0, z + 1.0)
            .inflate_xyz(horizontal, vertical, horizontal)
    }

    /// Vanilla `getHorizontalSearchDistance`.
    fn horizontal_search_distance(&self, body: &E) -> i32 {
        if body.is_passenger() {
            PASSENGER_MOB_TARGET_SEARCH_DISTANCE
        } else {
            self.horizontal_search_distance
        }
    }

    /// Vanilla `getVerticalSearchDistance`.
    fn vertical_search_distance(&self, body: &E) -> i32 {
        if body.is_passenger() {
            PASSENGER_MOB_TARGET_SEARCH_DISTANCE
        } else {
            self.vertical_search_distance
        }
    }

    /// Vanilla `setVisitedBlockPos`.
    fn set_visited_block_pos(&mut self, body: &E, world: &World, target: BlockPos) {
        let (mut visited, _) = remembered_positions(body);
        visited.insert(GlobalPos::new(world.key.clone(), target));
        if visited.len() > MAX_VISITED_POSITIONS {
            self.enter_cooldown_after_no_matching_target_found(body);
        } else if let Some(memories) = body.brain_memories() {
            memories.lock().set_with_expiry(
                &VISITED_BLOCK_POSITIONS,
                visited,
                VISITED_POSITIONS_MEMORY_TIME,
            );
        }
    }

    /// Vanilla `markVisitedBlockPosAsUnreachable`.
    fn mark_visited_block_pos_as_unreachable(&mut self, body: &E, world: &World, target: BlockPos) {
        let global = GlobalPos::new(world.key.clone(), target);
        let (mut visited, mut unreachable) = remembered_positions(body);
        visited.remove(&global);
        unreachable.insert(global);
        if unreachable.len() > MAX_UNREACHABLE_POSITIONS {
            self.enter_cooldown_after_no_matching_target_found(body);
        } else if let Some(memories) = body.brain_memories() {
            let mut memories = memories.lock();
            memories.set_with_expiry(
                &VISITED_BLOCK_POSITIONS,
                visited,
                VISITED_POSITIONS_MEMORY_TIME,
            );
            memories.set_with_expiry(
                &UNREACHABLE_TRANSPORT_BLOCK_POSITIONS,
                unreachable,
                VISITED_POSITIONS_MEMORY_TIME,
            );
        }
    }

    /// Vanilla `isWantedBlock`: a source chest with an empty hand, a
    /// destination chest otherwise.
    fn is_wanted_block(&self, body: &E, state: BlockStateId) -> bool {
        if is_picking_up_items(body) {
            (self.source_block_type)(state)
        } else {
            (self.destination_block_type)(state)
        }
    }

    /// Vanilla `isAnotherMobInteractingWithTarget`.
    fn is_another_mob_interacting_with_target(
        &self,
        target: &TransportItemTarget,
        world: &World,
    ) -> bool {
        connected_targets(target, world)
            .iter()
            .any(|connected| (self.should_queue_for_target)(world, connected))
    }

    /// Vanilla `pickUpItems`.
    fn pick_up_items(&mut self, body: &E, containers: &[ContainerRef]) {
        let taken = pickup_item_from_container(containers);
        body.set_item_slot(EquipmentSlot::MainHand, taken);
        body.set_guaranteed_drop(EquipmentSlot::MainHand);
        set_containers_changed(containers);
        self.clear_memories_after_matching_target_found(body);
    }

    /// Vanilla `putDownItem`.
    fn put_down_item(&mut self, body: &E, containers: &[ContainerRef]) {
        let left = add_items_to_container(body, containers);
        set_containers_changed(containers);
        let emptied = left.is_empty();
        body.set_item_slot(EquipmentSlot::MainHand, left);
        if emptied {
            self.clear_memories_after_matching_target_found(body);
        } else {
            self.stop_targeting_current_target(body);
        }
    }

    /// Vanilla `stopTargetingCurrentTarget`.
    fn stop_targeting_current_target(&mut self, body: &E) {
        self.ticks_since_reaching_target = 0;
        self.target = None;
        body.mob_base().navigation().lock().stop();
        if let Some(memories) = body.brain_memories() {
            memories.lock().erase(&WALK_TARGET);
        }
    }

    /// Vanilla `clearMemoriesAfterMatchingTargetFound`.
    fn clear_memories_after_matching_target_found(&mut self, body: &E) {
        self.stop_targeting_current_target(body);
        if let Some(memories) = body.brain_memories() {
            let mut memories = memories.lock();
            memories.erase(&VISITED_BLOCK_POSITIONS);
            memories.erase(&UNREACHABLE_TRANSPORT_BLOCK_POSITIONS);
        }
    }

    /// Vanilla `enterCooldownAfterNoMatchingTargetFound`.
    fn enter_cooldown_after_no_matching_target_found(&mut self, body: &E) {
        self.stop_targeting_current_target(body);
        if let Some(memories) = body.brain_memories() {
            let mut memories = memories.lock();
            memories.set(&TRANSPORT_ITEMS_COOLDOWN_TICKS, Some(IDLE_COOLDOWN));
            memories.erase(&VISITED_BLOCK_POSITIONS);
            memories.erase(&UNREACHABLE_TRANSPORT_BLOCK_POSITIONS);
        }
    }

    /// Vanilla `stop`, also run from `tick` once no target is left.
    fn stop_behavior(&mut self, body: &E) {
        self.on_start_traveling(body);
        set_can_path_to_targets_below_surface(body, false);
    }
}

impl<E: PathfinderMob + ?Sized> Behavior<E> for TransportItemsBetweenContainers<E> {
    fn entry_condition(&self) -> &[MemoryRequirement] {
        &ENTRY
    }

    fn check_extra_start_conditions(&mut self, _world: &Arc<World>, body: &E) -> bool {
        !is_leashed(body)
    }

    fn start(&mut self, _world: &Arc<World>, body: &E, _time: i64) {
        set_can_path_to_targets_below_surface(body, true);
    }

    fn can_still_use(&mut self, _world: &Arc<World>, body: &E, _time: i64) -> bool {
        let cooling_down = body
            .brain_memories()
            .is_some_and(|memories| memories.lock().has_value(&TRANSPORT_ITEMS_COOLDOWN_TICKS));
        !cooling_down && !body.is_panicking() && !is_leashed(body)
    }

    fn timed_out(&self, _time: i64, _end_time: i64) -> bool {
        false
    }

    fn tick(&mut self, world: &Arc<World>, body: &E, _time: i64) {
        let updated_invalid_target = self.update_invalid_target(world, body);
        let Some(target) = self.target.clone() else {
            // Vanilla calls `stop` here while the behavior keeps running.
            self.stop_behavior(body);
            return;
        };
        if updated_invalid_target {
            return;
        }
        if self.state == TransportItemState::Queuing {
            self.on_queuing_for_target(&target, world, body);
        }
        if self.state == TransportItemState::Traveling {
            self.on_travel_to_target(&target, world, body);
        }
        if self.state == TransportItemState::Interacting {
            self.on_reached_target(&target, world, body);
        }
    }

    fn stop(&mut self, _world: &Arc<World>, body: &E, _time: i64) {
        self.stop_behavior(body);
    }
}

/// Vanilla `body.isLeashed()`.
fn is_leashed<E: PathfinderMob + ?Sized>(body: &E) -> bool {
    body.as_leashable().is_some_and(Leashable::is_leashed)
}

/// Vanilla `GroundPathNavigation.setCanPathToTargetsBelowSurface`, only for
/// ground navigation.
fn set_can_path_to_targets_below_surface<E: PathfinderMob + ?Sized>(body: &E, value: bool) {
    let mut navigation = body.mob_base().navigation().lock();
    if navigation.kind() == NavigationKind::Ground {
        navigation.set_can_path_to_targets_below_surface(value);
    }
}

/// The visited and unreachable chest positions the brain remembers.
fn remembered_positions<E: PathfinderMob + ?Sized>(
    body: &E,
) -> (FxHashSet<GlobalPos>, FxHashSet<GlobalPos>) {
    let Some(memories) = body.brain_memories() else {
        return (FxHashSet::default(), FxHashSet::default());
    };
    let memories = memories.lock();
    (
        memories.get(&VISITED_BLOCK_POSITIONS).unwrap_or_default(),
        memories
            .get(&UNREACHABLE_TRANSPORT_BLOCK_POSITIONS)
            .unwrap_or_default(),
    )
}

/// Vanilla `getConnectedTargets`: both halves of a double chest.
fn connected_targets(target: &TransportItemTarget, world: &World) -> Vec<TransportItemTarget> {
    let chest_type = target
        .state
        .try_get_value(&BlockStateProperties::CHEST_TYPE)
        .unwrap_or(ChestType::Single);
    let mut targets = vec![target.clone()];
    if chest_type != ChestType::Single
        && let Some(connected) = TransportItemTarget::at(
            target.pos.relative(connected_direction(target.state)),
            world,
        )
    {
        targets.push(connected);
    }
    targets
}

/// Vanilla `isPositionAlreadyVisited`.
fn is_position_already_visited(
    visited: &FxHashSet<GlobalPos>,
    unreachable: &FxHashSet<GlobalPos>,
    target: &TransportItemTarget,
    world: &World,
) -> bool {
    connected_targets(target, world).iter().any(|connected| {
        let global = GlobalPos::new(world.key.clone(), connected.pos);
        visited.contains(&global) || unreachable.contains(&global)
    })
}

/// Vanilla `targetHasNotChanged`: the same block entity still stands there.
fn target_has_not_changed(world: &World, target: &TransportItemTarget) -> bool {
    world
        .get_block_entity(target.pos)
        .is_some_and(|current| Arc::ptr_eq(&current, &target.block_entity))
}

/// Vanilla `hasFinishedPath`.
fn has_finished_path<E: PathfinderMob + ?Sized>(body: &E) -> bool {
    body.mob_base()
        .navigation()
        .lock()
        .path()
        .is_some_and(Path::is_done)
}

/// Vanilla `getInteractionRange`.
fn interaction_range<E: PathfinderMob + ?Sized>(body: &E) -> f64 {
    if has_finished_path(body) {
        CLOSE_ENOUGH_TO_START_INTERACTING_PATH_END_DISTANCE
    } else {
        CLOSE_ENOUGH_TO_START_INTERACTING_DISTANCE
    }
}

/// Vanilla `setMiddleYPosition`.
fn middle_y<E: PathfinderMob + ?Sized>(body: &E, pos: DVec3) -> DVec3 {
    pos + DVec3::new(0.0, body.bounding_box().height() / 2.0, 0.0)
}

/// Vanilla `getCenterPos`.
fn center_pos<E: PathfinderMob + ?Sized>(body: &E) -> DVec3 {
    middle_y(body, body.position())
}

/// Vanilla `hasValidTravelingPath`.
fn has_valid_traveling_path<E: PathfinderMob + ?Sized>(
    world: &World,
    target: &TransportItemTarget,
    body: &E,
) -> bool {
    let current = body.mob_base().navigation().lock().path().cloned();
    let path = current.or_else(|| body.create_path_to(target.pos, 0));
    let end = path.as_ref().and_then(Path::end_node).map_or_else(
        || body.position(),
        |node| {
            let pos = node.as_block_pos();
            DVec3::new(
                f64::from(pos.x()) + 0.5,
                f64::from(pos.y()),
                f64::from(pos.z()) + 0.5,
            )
        },
    );
    let from = middle_y(body, end);
    let can_reach_target = is_within_target_distance(interaction_range(body), target, body, from);
    let has_not_yet_created_path = path.is_none() && !can_reach_target;
    has_not_yet_created_path || (can_reach_target && can_see_any_target_side(target, world, from))
}

/// Vanilla `isWithinTargetDistance`: the mob's box moved to `from` touches
/// the chest's collision box grown by `distance` sideways and half a block up
/// and down.
fn is_within_target_distance<E: PathfinderMob + ?Sized>(
    distance: f64,
    target: &TransportItemTarget,
    body: &E,
    from: DVec3,
) -> bool {
    let bounding_box = body.bounding_box();
    let moved = WorldAabb::of_size(
        from,
        bounding_box.width(),
        bounding_box.height(),
        bounding_box.depth(),
    );
    target
        .state
        .get_static_collision_shape()
        .bounds()
        .is_some_and(|bounds| {
            bounds
                .inflate_xyz(distance, 0.5, distance)
                .at_block(target.pos)
                .intersects(moved)
        })
}

/// Vanilla `canSeeAnyTargetSide`: a collider ray from `eye` reaches the
/// chest on one of its six face centers.
fn can_see_any_target_side(target: &TransportItemTarget, world: &World, eye: DVec3) -> bool {
    let (x, y, z) = target.pos.get_center();
    let center = DVec3::new(x, y, z);
    Direction::ALL.iter().any(|direction| {
        let (step_x, step_y, step_z) = direction.offset();
        let hit_target = center
            + DVec3::new(
                0.5 * f64::from(step_x),
                0.5 * f64::from(step_y),
                0.5 * f64::from(step_z),
            );
        let hit = world.clip(eye, hit_target, ClipBlockShape::Collider, ClipFluid::None);
        !hit.miss && hit.block_pos == target.pos
    })
}

/// Vanilla `BlockPos.distToCenterSqr(Position)`.
fn dist_to_center_sqr(pos: BlockPos, position: DVec3) -> f64 {
    let (x, y, z) = pos.get_center();
    DVec3::new(x, y, z).distance_squared(position)
}

/// Vanilla `isPickingUpItems`: the mob's main hand is empty.
fn is_picking_up_items<E: PathfinderMob + ?Sized>(body: &E) -> bool {
    body.equipment_in_slot(EquipmentSlot::MainHand).is_empty()
}

/// Vanilla `RandomizableContainerBlockEntity` reads unpack pending loot
/// before any slot is seen.
fn unpack_loot(containers: &[ContainerRef]) {
    for container in containers {
        let _ = container.unpack_loot_table(None);
    }
}

/// Every slot of the (double) chest in vanilla `CompoundContainer` order.
fn slots(containers: &[ContainerRef]) -> Vec<(usize, usize)> {
    let guard = ContainerLockGuard::lock_all(containers);
    containers
        .iter()
        .enumerate()
        .flat_map(|(index, container)| {
            let size = guard
                .get(container.container_id())
                .map_or(0, Container::get_container_size);
            (0..size).map(move |slot| (index, slot))
        })
        .collect()
}

/// Vanilla `matchesGettingItemsRequirement`: the source holds anything.
fn matches_getting_items_requirement(containers: &[ContainerRef]) -> bool {
    let guard = ContainerLockGuard::lock_all(containers);
    containers.iter().any(|container| {
        guard
            .get(container.container_id())
            .is_some_and(|locked| !locked.is_empty())
    })
}

/// Vanilla `matchesLeavingItemsRequirement`: the destination is empty or
/// already holds the carried item.
fn matches_leaving_items_requirement<E: PathfinderMob + ?Sized>(
    body: &E,
    containers: &[ContainerRef],
) -> bool {
    let held = body.equipment_in_slot(EquipmentSlot::MainHand);
    let guard = ContainerLockGuard::lock_all(containers);
    let mut lockeds = containers
        .iter()
        .filter_map(|container| guard.get(container.container_id()));
    let all_empty = containers.iter().all(|container| {
        guard
            .get(container.container_id())
            .is_none_or(Container::is_empty)
    });
    all_empty
        || lockeds.any(|locked| {
            locked
                .iter()
                .any(|stack| ItemStack::is_same_item(stack, &held))
        })
}

/// Vanilla `pickupItemFromContainer`: up to 16 items of the first filled slot.
fn pickup_item_from_container(containers: &[ContainerRef]) -> ItemStack {
    let slots = slots(containers);
    let mut guard = ContainerLockGuard::lock_all(containers);
    for (index, slot) in slots {
        let id = containers[index].container_id();
        let Some(count) = guard
            .get(id)
            .map(|locked| locked.get_item(slot))
            .filter(|stack| !stack.is_empty())
            .map(ItemStack::count)
        else {
            continue;
        };
        let amount = count.min(TRANSPORTED_ITEM_MAX_STACK_SIZE);
        return guard.remove_item(id, slot, amount).unwrap_or_default();
    }
    ItemStack::empty()
}

/// Vanilla `addItemsToContainer`: fills the first empty slot, topping up
/// matching stacks on the way; returns what did not fit.
fn add_items_to_container<E: PathfinderMob + ?Sized>(
    body: &E,
    containers: &[ContainerRef],
) -> ItemStack {
    let mut held = body.equipment_in_slot(EquipmentSlot::MainHand);
    let slots = slots(containers);
    let mut guard = ContainerLockGuard::lock_all(containers);
    for (index, slot) in slots {
        let id = containers[index].container_id();
        let Some(mut in_slot) = guard.get(id).map(|locked| locked.get_item(slot).clone()) else {
            continue;
        };
        if in_slot.is_empty() {
            guard.set_item(id, slot, held);
            return ItemStack::empty();
        }
        if ItemStack::is_same_item_same_components(&in_slot, &held)
            && in_slot.count() < in_slot.max_stack_size()
        {
            let can_be_added = in_slot.max_stack_size() - in_slot.count();
            let to_add = can_be_added.min(held.count());
            in_slot.set_count(in_slot.count() + to_add);
            // Vanilla subtracts the room in the slot, not what moved.
            held.set_count(held.count() - can_be_added);
            guard.set_item(id, slot, in_slot);
            if held.is_empty() {
                return ItemStack::empty();
            }
        }
    }
    held
}

/// Vanilla `container.setChanged()`.
fn set_containers_changed(containers: &[ContainerRef]) {
    let mut guard = ContainerLockGuard::lock_all(containers);
    for container in containers {
        guard.set_changed(container.container_id());
    }
}

/// The behavior's own `stopInPlace`: unlike `Mob.stopInPlace`, it keeps
/// falling.
fn stop_in_place<E: PathfinderMob + ?Sized>(body: &E) {
    body.mob_base().navigation().lock().stop();
    let forward = body.travel_input().forward();
    body.set_travel_input(LivingTravelInput::new(0.0, 0.0, forward));
    body.set_speed(0.0);
    let velocity = body.velocity();
    body.set_velocity(DVec3::new(0.0, velocity.y, 0.0));
}
