//! Vanilla `AllayAi`.

use std::sync::Arc;

use glam::DVec3;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::{sound_events, vanilla_blocks};
use steel_utils::BlockPos;
use steel_utils::types::{GameType, GlobalPos};

use super::AllayEntity;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::TimedBehavior;
use crate::entity::ai::brain::behaviors::utils::throw_item;
use crate::entity::ai::brain::behaviors::{
    AnimalPanic, CountDownCooldownTicks, DoNothing, GoAndGiveItemsToTarget, LookAtTargetSink,
    MoveToTargetSink, Swim, UniformInt, go_to_wanted_item, random_fly, run_one,
    set_entity_look_target_sometimes, set_walk_target_from_look_target, stay_close_to_target,
};
use crate::entity::ai::brain::memory_types::{
    ITEM_PICKUP_COOLDOWN_TICKS, LIKED_NOTEBLOCK_COOLDOWN_TICKS, LIKED_NOTEBLOCK_POSITION,
    LIKED_PLAYER, NEAREST_VISIBLE_WANTED_ITEM, PositionTracker,
};
use crate::entity::ai::brain::sensors::{
    HurtBySensor, NearestItemSensor, NearestLivingEntitySensor, PlayerSensor,
};
use crate::entity::ai::brain::{ActivityData, Sensor};
use crate::entity::{Entity, LivingEntity, SharedEntity};
use crate::player::advancement::criteria::ALLAY_DROP_ITEM_ON_BLOCK;
use crate::world::World;

const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
const SPEED_MULTIPLIER_WHEN_FOLLOWING_DEPOSIT_TARGET: f32 = 2.25;
const SPEED_MULTIPLIER_WHEN_RETRIEVING_ITEM: f32 = 1.75;
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 2.5;
const CLOSE_ENOUGH_TO_TARGET: i32 = 4;
const TOO_FAR_FROM_TARGET: i32 = 16;
const MAX_LOOK_DISTANCE: f32 = 6.0;
const LOOK_INTERVAL: UniformInt = UniformInt::of(30, 60);
const MIN_WAIT_DURATION: i32 = 30;
const MAX_WAIT_DURATION: i32 = 60;
/// Vanilla `TIME_TO_FORGET_NOTEBLOCK`.
pub(super) const TIME_TO_FORGET_NOTEBLOCK: i32 = 600;
const DISTANCE_TO_WANTED_ITEM: i32 = 32;
const GIVE_ITEM_TIMEOUT_DURATION: i32 = 20;
/// Vanilla `THROW_VELOCITY`: `0.2F, 0.3F, 0.2F` widened to doubles.
const THROW_VELOCITY: DVec3 = DVec3::new(
    0.200_000_002_980_232_24,
    0.300_000_011_920_928_96,
    0.200_000_002_980_232_24,
);
const ITEM_PICKUP_COOLDOWN_DURATION: i32 = 60;
/// Vanilla `Allay.MAX_NOTEBLOCK_DISTANCE`.
pub(super) const MAX_NOTEBLOCK_DISTANCE: i32 = 1024;
/// Vanilla `getLikedPlayer`: players farther than this are ignored.
const LIKED_PLAYER_MAX_DISTANCE: f64 = 64.0;

/// The sensors vanilla's allay `Brain.provider` lists.
pub(super) fn sensors() -> Vec<Box<dyn Sensor<AllayEntity>>> {
    vec![
        Box::new(NearestLivingEntitySensor),
        Box::new(PlayerSensor),
        Box::new(HurtBySensor),
        Box::new(NearestItemSensor),
    ]
}

/// Vanilla `AllayAi.getActivities`.
pub(super) fn activities() -> Vec<ActivityData<AllayEntity>> {
    vec![core_activity(), idle_activity()]
}

fn core_activity() -> ActivityData<AllayEntity> {
    ActivityData::with_first_priority(
        Activity::CORE,
        0,
        vec![
            Box::new(TimedBehavior::new(Swim::new(0.8))),
            Box::new(TimedBehavior::new(AnimalPanic::new(
                SPEED_MULTIPLIER_WHEN_PANICKING,
            ))),
            Box::new(TimedBehavior::new(LookAtTargetSink::new(45, 90))),
            Box::new(TimedBehavior::new(MoveToTargetSink::new())),
            Box::new(TimedBehavior::new(CountDownCooldownTicks::new(
                &LIKED_NOTEBLOCK_COOLDOWN_TICKS,
            ))),
            Box::new(TimedBehavior::new(CountDownCooldownTicks::new(
                &ITEM_PICKUP_COOLDOWN_TICKS,
            ))),
        ],
    )
}

fn idle_activity() -> ActivityData<AllayEntity> {
    ActivityData::with_first_priority(
        Activity::IDLE,
        0,
        vec![
            Box::new(go_to_wanted_item(
                |_: &AllayEntity| true,
                SPEED_MULTIPLIER_WHEN_RETRIEVING_ITEM,
                true,
                DISTANCE_TO_WANTED_ITEM,
            )),
            Box::new(TimedBehavior::new(GoAndGiveItemsToTarget::new(
                item_deposit_position,
                SPEED_MULTIPLIER_WHEN_FOLLOWING_DEPOSIT_TARGET,
                GIVE_ITEM_TIMEOUT_DURATION,
                throw_item_to,
                &ITEM_PICKUP_COOLDOWN_TICKS,
                ITEM_PICKUP_COOLDOWN_DURATION,
                has_item_to_throw,
            ))),
            Box::new(stay_close_to_target(
                item_deposit_position,
                |body: &AllayEntity| !has_wanted_item(body),
                CLOSE_ENOUGH_TO_TARGET,
                TOO_FAR_FROM_TARGET,
                SPEED_MULTIPLIER_WHEN_FOLLOWING_DEPOSIT_TARGET,
            )),
            Box::new(set_entity_look_target_sometimes(
                MAX_LOOK_DISTANCE,
                LOOK_INTERVAL,
                |_: &dyn LivingEntity| true,
            )),
            Box::new(run_one(
                Vec::new(),
                vec![
                    (Box::new(random_fly(SPEED_MULTIPLIER_WHEN_IDLING)), 2),
                    (
                        Box::new(set_walk_target_from_look_target(
                            |_: &AllayEntity| true,
                            |_: &AllayEntity| SPEED_MULTIPLIER_WHEN_IDLING,
                            3,
                        )),
                        2,
                    ),
                    (
                        Box::new(DoNothing::new(MIN_WAIT_DURATION, MAX_WAIT_DURATION)),
                        1,
                    ),
                ],
            )),
        ],
    )
}

fn has_item_to_throw(body: &AllayEntity) -> bool {
    !body.inventory_is_empty()
}

/// Vanilla `AllayAi.updateActivity`.
pub(super) fn update_activity(body: &AllayEntity) {
    body.brain()
        .set_active_activity_to_first_valid(&[Activity::IDLE]);
}

/// Vanilla `AllayAi.hearNoteblock`: the first note block heard becomes the
/// liked one, and hearing it again refreshes the memory.
pub fn hear_noteblock(body: &AllayEntity, world: &World, pos: BlockPos) {
    let global_pos = GlobalPos::new(world.key.clone(), pos);
    let mut memories = body.brain().memories().lock();
    match memories.get_ref(&LIKED_NOTEBLOCK_POSITION) {
        None => {
            memories.set(&LIKED_NOTEBLOCK_POSITION, Some(global_pos));
            memories.set(
                &LIKED_NOTEBLOCK_COOLDOWN_TICKS,
                Some(TIME_TO_FORGET_NOTEBLOCK),
            );
        }
        Some(liked) if *liked == global_pos => {
            memories.set(
                &LIKED_NOTEBLOCK_COOLDOWN_TICKS,
                Some(TIME_TO_FORGET_NOTEBLOCK),
            );
        }
        Some(_) => {}
    }
}

/// Vanilla `GlobalPos.isCloseEnough`: same dimension, chessboard distance.
pub(super) fn is_close_enough(
    global_pos: &GlobalPos,
    world: &World,
    pos: BlockPos,
    max_distance: i32,
) -> bool {
    let chessboard = (global_pos.pos.x() - pos.x())
        .abs()
        .max((global_pos.pos.y() - pos.y()).abs())
        .max((global_pos.pos.z() - pos.z()).abs());
    global_pos.dimension == world.key && chessboard <= max_distance
}

/// Vanilla `getItemDepositPosition`: the liked note block while it still
/// plays, otherwise the liked player.
fn item_deposit_position(body: &AllayEntity) -> Option<PositionTracker> {
    let world = body.level()?;
    let liked_noteblock = body
        .brain()
        .memories()
        .lock()
        .get(&LIKED_NOTEBLOCK_POSITION);
    if let Some(position) = liked_noteblock {
        if should_deposit_items_at_liked_noteblock(body, &world, &position) {
            return Some(PositionTracker::block(position.pos.above()));
        }
        body.brain()
            .memories()
            .lock()
            .erase(&LIKED_NOTEBLOCK_POSITION);
    }
    liked_player(body).map(|player| PositionTracker::entity(player, true))
}

fn has_wanted_item(body: &AllayEntity) -> bool {
    body.brain()
        .memories()
        .lock()
        .has_value(&NEAREST_VISIBLE_WANTED_ITEM)
}

fn should_deposit_items_at_liked_noteblock(
    body: &AllayEntity,
    world: &World,
    liked_noteblock: &GlobalPos,
) -> bool {
    let has_cooldown = body
        .brain()
        .memories()
        .lock()
        .has_value(&LIKED_NOTEBLOCK_COOLDOWN_TICKS);
    is_close_enough(
        liked_noteblock,
        world,
        body.block_position(),
        MAX_NOTEBLOCK_DISTANCE,
    ) && world.get_block_state(liked_noteblock.pos).get_block() == &vanilla_blocks::NOTE_BLOCK
        && has_cooldown
}

/// Vanilla `AllayAi.getLikedPlayer`: the liked player while they are nearby
/// in survival or creative.
pub(super) fn liked_player(body: &AllayEntity) -> Option<SharedEntity> {
    let world = body.level()?;
    let uuid = body.brain().memories().lock().get(&LIKED_PLAYER)?;
    let entity = world.get_entity_by_uuid(&uuid)?;
    let player = entity.as_player()?;
    let game_mode = player.game_mode();
    let in_range = player.position().distance_squared(body.position())
        < LIKED_PLAYER_MAX_DISTANCE * LIKED_PLAYER_MAX_DISTANCE;
    ((game_mode == GameType::Survival || game_mode == GameType::Creative) && in_range)
        .then_some(entity)
}

/// Vanilla `AllayAi.throwItem`.
fn throw_item_to(world: &Arc<World>, thrower: &AllayEntity, target_pos: DVec3) {
    let item = thrower.remove_one_from_inventory();
    if item.is_empty() {
        return;
    }
    let thrown = item.clone();
    throw_item(
        thrower,
        item,
        target_pos + DVec3::new(0.0, 1.0, 0.0),
        THROW_VELOCITY,
        0.2,
    );
    if let Some(liked) = liked_player(thrower)
        && let Some(player) = liked.as_player()
    {
        let below = BlockPos::containing(target_pos.x, target_pos.y, target_pos.z).below();
        player.trigger_item_used_on_location(&ALLAY_DROP_ITEM_ON_BLOCK, below, &thrown);
    }
    if world.game_time() % 7 == 0 && rand::random::<f64>() < 0.9 {
        let pitches = &super::THROW_SOUND_PITCHES;
        let pitch = pitches[rand::random_range(0..pitches.len())];
        world.play_sound_at(
            &sound_events::ENTITY_ALLAY_ITEM_THROWN,
            SoundSource::Neutral,
            thrower.position(),
            1.0,
            pitch,
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{REGISTRY, vanilla_blocks, vanilla_entities, vanilla_items};
    use steel_utils::types::UpdateFlags;
    use steel_utils::{BlockPos, ChunkPos, Identifier};

    use super::throw_item_to;
    use crate::bootstrap::init_globals;
    use crate::entity::Entity as _;
    use crate::entity::ai::brain::memory_types::LIKED_PLAYER;
    use crate::entity::entities::mobs::passive::allay::AllayEntity;
    use crate::entity::{init_entities, next_entity_id};
    use crate::player::advancement::criteria::init_criteria;
    use crate::player::{Player, ResetReason};
    use crate::test_support::{TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk};

    fn cake_delivery_done(player: &Player) -> bool {
        let key = Identifier::vanilla_static("husbandry/allay_deliver_cake_to_note_block");
        let advancement = REGISTRY
            .advancements
            .value_by_key(&key)
            .expect("vanilla advancement exists");
        player
            .advancements
            .lock()
            .progress
            .get_progress(advancement)
            .is_some_and(|progress| progress.is_criterion_done("allay_deliver_cake_to_note_block"))
    }

    #[test]
    fn an_allay_dropping_cake_on_a_note_block_credits_its_liked_player() {
        init_globals();
        init_entities();
        init_criteria();
        let fixture = fresh_test_world("allay_cake_note_block");
        let world = &fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let note_block = BlockPos::new(8, 64, 8);
        assert!(world.set_block(
            note_block,
            vanilla_blocks::NOTE_BLOCK.default_state(),
            UpdateFlags::UPDATE_ALL,
        ));
        let player =
            TestPlayerBuilder::new(Arc::clone(world), "Friend".to_owned(), next_entity_id())
                .build();
        assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));
        player
            .teleport_to(DVec3::new(10.5, 65.0, 10.5))
            .expect("player teleport");
        let allay = Arc::new(AllayEntity::new(
            &vanilla_entities::ALLAY,
            next_entity_id(),
            DVec3::new(8.5, 66.0, 10.5),
            Arc::downgrade(world),
        ));
        world
            .try_add_entity(Arc::clone(&allay) as _)
            .expect("allay added");
        allay
            .brain()
            .memories()
            .lock()
            .set(&LIKED_PLAYER, Some(player.uuid()));
        let above_note_block = DVec3::new(8.5, 65.5, 8.5);

        allay
            .inventory
            .lock()
            .add_item(&ItemStack::new(&vanilla_items::DIAMOND));
        throw_item_to(world, &allay, above_note_block);
        assert!(!cake_delivery_done(&player), "only cake counts");

        allay
            .inventory
            .lock()
            .add_item(&ItemStack::new(&vanilla_items::CAKE));
        throw_item_to(world, &allay, above_note_block);
        assert!(cake_delivery_done(&player));
    }
}
