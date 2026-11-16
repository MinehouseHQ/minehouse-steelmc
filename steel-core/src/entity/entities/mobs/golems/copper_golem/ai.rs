//! Vanilla `CopperGolemAi`.

use std::sync::Arc;

use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, ChestType};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::{sound_events, vanilla_blocks, vanilla_entities};
use steel_utils::{BlockStateId, Downcast as _};

use super::{CONTAINER_INTERACTION_RANGE, CopperGolemEntity, CopperGolemState};
use crate::behavior::blocks::connected_direction;
use crate::block_entity::SharedBlockEntity;
use crate::block_entity::entities::ChestBlockEntity;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::TimedBehavior;
use crate::entity::ai::brain::behaviors::{
    AnimalPanic, ContainerInteractionState, CountDownCooldownTicks, DoNothing, LookAtTargetSink,
    MoveToTargetSink, TARGET_INTERACTION_TIME, TransportItemTarget,
    TransportItemsBetweenContainers, UniformInt, interact_with_door, random_stroll_within, run_one,
    set_entity_look_target_sometimes,
};
use crate::entity::ai::brain::memory::MemoryRequirement;
use crate::entity::ai::brain::memory_types::{
    GAZE_COOLDOWN_TICKS, TRANSPORT_ITEMS_COOLDOWN_TICKS, UNREACHABLE_TRANSPORT_BLOCK_POSITIONS,
    VISITED_BLOCK_POSITIONS, WALK_TARGET,
};
use crate::entity::ai::brain::sensors::{HurtBySensor, NearestLivingEntitySensor};
use crate::entity::ai::brain::{ActivityData, AnyMemoryModuleType, MemoryStatus, Sensor};
use crate::entity::{Entity, LivingEntity};
use crate::world::World;

const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 1.5;
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
const TRANSPORT_ITEM_HORIZONTAL_SEARCH_RADIUS: i32 = 32;
const TRANSPORT_ITEM_VERTICAL_SEARCH_RADIUS: i32 = 8;
const TICK_TO_START_ON_REACHED_INTERACTION: i32 = 1;
const TICK_TO_PLAY_ON_REACHED_SOUND: i32 = 9;
const MAX_LOOK_DISTANCE: f32 = 6.0;
const LOOK_INTERVAL: UniformInt = UniformInt::of(40, 80);
const STROLL_XZ_DISTANCE: i32 = 2;
const STROLL_Y_DISTANCE: i32 = 2;
const MIN_WAIT_DURATION: i32 = 30;
const MAX_WAIT_DURATION: i32 = 60;

/// The memories the golem's behaviors use outside their entry conditions.
pub(super) fn memories() -> [&'static dyn AnyMemoryModuleType; 4] {
    [
        &GAZE_COOLDOWN_TICKS,
        &TRANSPORT_ITEMS_COOLDOWN_TICKS,
        &VISITED_BLOCK_POSITIONS,
        &UNREACHABLE_TRANSPORT_BLOCK_POSITIONS,
    ]
}

/// The sensors vanilla's copper golem `Brain.provider` lists.
pub(super) fn sensors() -> Vec<Box<dyn Sensor<CopperGolemEntity>>> {
    vec![Box::new(NearestLivingEntitySensor), Box::new(HurtBySensor)]
}

/// Vanilla `CopperGolemAi.getActivities`.
pub(super) fn activities() -> Vec<ActivityData<CopperGolemEntity>> {
    vec![core_activity(), idle_activity()]
}

/// Vanilla `CopperGolemAi.updateActivity`.
pub(super) fn update_activity(body: &CopperGolemEntity) {
    body.brain()
        .set_active_activity_to_first_valid(&[Activity::IDLE]);
}

fn core_activity() -> ActivityData<CopperGolemEntity> {
    ActivityData::with_first_priority(
        Activity::CORE,
        0,
        vec![
            Box::new(TimedBehavior::new(AnimalPanic::new(
                SPEED_MULTIPLIER_WHEN_PANICKING,
            ))),
            Box::new(TimedBehavior::new(LookAtTargetSink::new(45, 90))),
            Box::new(TimedBehavior::new(MoveToTargetSink::new())),
            Box::new(interact_with_door()),
            Box::new(TimedBehavior::new(CountDownCooldownTicks::new(
                &GAZE_COOLDOWN_TICKS,
            ))),
            Box::new(TimedBehavior::new(CountDownCooldownTicks::new(
                &TRANSPORT_ITEMS_COOLDOWN_TICKS,
            ))),
        ],
    )
}

fn idle_activity() -> ActivityData<CopperGolemEntity> {
    ActivityData::new(
        Activity::IDLE,
        vec![
            (
                0,
                Box::new(TimedBehavior::new(TransportItemsBetweenContainers::new(
                    SPEED_MULTIPLIER_WHEN_IDLING,
                    is_source_chest,
                    is_destination_chest,
                    TRANSPORT_ITEM_HORIZONTAL_SEARCH_RADIUS,
                    TRANSPORT_ITEM_VERTICAL_SEARCH_RADIUS,
                    on_target_reached,
                    on_travelling,
                    should_queue_for_target,
                ))),
            ),
            (
                1,
                Box::new(set_entity_look_target_sometimes(
                    MAX_LOOK_DISTANCE,
                    LOOK_INTERVAL,
                    |entity: &dyn LivingEntity| entity.entity_type() == &vanilla_entities::PLAYER,
                )),
            ),
            (
                2,
                Box::new(run_one(
                    vec![
                        MemoryRequirement::new(&WALK_TARGET, MemoryStatus::ValueAbsent),
                        MemoryRequirement::new(
                            &TRANSPORT_ITEMS_COOLDOWN_TICKS,
                            MemoryStatus::ValuePresent,
                        ),
                    ],
                    vec![
                        (
                            Box::new(random_stroll_within(
                                SPEED_MULTIPLIER_WHEN_IDLING,
                                STROLL_XZ_DISTANCE,
                                STROLL_Y_DISTANCE,
                            )),
                            1,
                        ),
                        (
                            Box::new(DoNothing::new(MIN_WAIT_DURATION, MAX_WAIT_DURATION)),
                            1,
                        ),
                    ],
                )),
            ),
        ],
    )
}

/// Vanilla `TRANSPORT_ITEM_SOURCE_BLOCK`.
fn is_source_chest(state: BlockStateId) -> bool {
    state.get_block().has_tag(&BlockTag::COPPER_CHESTS)
}

/// Vanilla `TRANSPORT_ITEM_DESTINATION_BLOCK`.
fn is_destination_chest(state: BlockStateId) -> bool {
    let block: BlockRef = state.get_block();
    block == &vanilla_blocks::CHEST || block == &vanilla_blocks::TRAPPED_CHEST
}

/// What the golem shows and says at the chest for each interaction.
const fn interaction_look(state: ContainerInteractionState) -> (CopperGolemState, SoundEventRef) {
    match state {
        ContainerInteractionState::PickupItem => (
            CopperGolemState::GettingItem,
            &sound_events::ENTITY_COPPER_GOLEM_NO_ITEM_GET,
        ),
        ContainerInteractionState::PickupNoItem => (
            CopperGolemState::GettingNoItem,
            &sound_events::ENTITY_COPPER_GOLEM_NO_ITEM_NO_GET,
        ),
        ContainerInteractionState::PlaceItem => (
            CopperGolemState::DroppingItem,
            &sound_events::ENTITY_COPPER_GOLEM_ITEM_DROP,
        ),
        ContainerInteractionState::PlaceNoItem => (
            CopperGolemState::DroppingNoItem,
            &sound_events::ENTITY_COPPER_GOLEM_ITEM_NO_DROP,
        ),
    }
}

/// The block entities of the target chest, both halves of a double chest,
/// which vanilla's `CompoundContainer` opens and closes together.
fn chest_halves(target: &TransportItemTarget, world: &World) -> Vec<SharedBlockEntity> {
    let mut halves = vec![Arc::clone(&target.block_entity)];
    let chest_type = target
        .state
        .try_get_value(&BlockStateProperties::CHEST_TYPE)
        .unwrap_or(ChestType::Single);
    if chest_type != ChestType::Single
        && let Some(partner) =
            world.get_block_entity(target.pos.relative(connected_direction(target.state)))
    {
        halves.push(partner);
    }
    halves
}

/// Vanilla `onReachedTargetInteraction`: opens the chest, shows the
/// interaction, says it, and closes the chest after three seconds.
fn on_target_reached(
    body: &CopperGolemEntity,
    world: &Arc<World>,
    target: &TransportItemTarget,
    interaction: ContainerInteractionState,
    ticks_since_reaching_target: i32,
) {
    let (state, sound) = interaction_look(interaction);
    let halves = chest_halves(target, world);
    let chests = halves
        .iter()
        .filter_map(|half| half.downcast_ref::<ChestBlockEntity>());
    if ticks_since_reaching_target == TICK_TO_START_ON_REACHED_INTERACTION {
        for chest in chests.clone() {
            chest.start_open_by(body, CONTAINER_INTERACTION_RANGE);
        }
        body.set_opened_chest_pos(Some(target.pos));
        body.set_state(state);
    }
    if ticks_since_reaching_target == TICK_TO_PLAY_ON_REACHED_SOUND {
        body.play_sound(sound, 1.0, 1.0);
    }
    if ticks_since_reaching_target == TARGET_INTERACTION_TIME {
        if chests
            .clone()
            .any(|chest| chest.is_open_by(world, body.id()))
        {
            for chest in chests {
                chest.stop_open_by(body);
            }
        }
        body.set_opened_chest_pos(None);
    }
}

/// Vanilla `onTravelling`.
fn on_travelling(body: &CopperGolemEntity) {
    body.set_opened_chest_pos(None);
    body.set_state(CopperGolemState::Idle);
}

/// Vanilla `shouldQueueForTarget`: waits while someone has the chest open.
fn should_queue_for_target(world: &World, target: &TransportItemTarget) -> bool {
    target.chest().is_some_and(|chest| chest.has_viewers(world))
}
