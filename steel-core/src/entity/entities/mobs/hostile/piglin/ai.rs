//! The piglin half of vanilla `PiglinAi`: activities, bartering, admiring
//! gold, item pickup and reactions to being hurt.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::data_components::vanilla_components::SwingAnimation;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{sound_events, vanilla_entities, vanilla_items, vanilla_loot_tables};
use steel_utils::random::Random as _;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::InteractionHand;
use steel_utils::{ChunkPos, Downcast as _};

use super::anger::{
    anger_target, broadcast_anger_target, dont_kill_any_more_hoglins_for_a_while, is_loved_item,
    is_player_holding_loved_item, is_zombified, maybe_retaliate,
};
use super::behaviors::{
    remember_if_hoglin_was_killed, start_admiring_item_if_seen, start_hunting_hoglin,
    stop_admiring_if_item_too_far_away, stop_admiring_if_tired_of_trying_to_reach_item,
    stop_holding_item_if_no_longer_admiring,
};
use super::{AbstractPiglin as _, PiglinEntity};
use crate::behavior::InteractionResult;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::{OneShot, TimedBehavior};
use crate::entity::ai::brain::behaviors::utils::throw_item;
use crate::entity::ai::brain::behaviors::{
    CrossbowAttack, DoNothing, LookAtTargetSink, MoveToTargetSink, SpearApproach, SpearAttack,
    SpearRetreat, Ticker, UniformInt, back_up_if_too_close, copy_memory_with_expiry,
    dismount_or_skip_mounting, erase_memory_if, get_nearest_target, go_to_target_location,
    go_to_wanted_item, interact_with_door, interact_with_type, melee_attack, mount, random_stroll,
    random_stroll_within, run_one, set_entity_look_target, set_look_and_interact,
    set_walk_target_away_from_entity, set_walk_target_away_from_pos,
    set_walk_target_from_attack_target_if_target_out_of_reach, set_walk_target_from_look_target,
    start_attacking, start_celebrating_if_target_dead, stop_attacking_if_target_invalid_when,
    stop_being_angry_if_target_dead, trigger_if, trigger_one_shuffled,
};
use crate::entity::ai::brain::memory_types::{
    ADMIRING_DISABLED, ADMIRING_ITEM, ANGRY_AT, ATE_RECENTLY, ATTACK_TARGET, AVOID_TARGET,
    CELEBRATE_LOCATION, DANCING, HURT_BY, INTERACTION_TARGET, NEAREST_PLAYER_HOLDING_WANTED_ITEM,
    NEAREST_REPELLENT, NEAREST_TARGETABLE_PLAYER_NOT_WEARING_GOLD, NEAREST_VISIBLE_ADULT_PIGLINS,
    NEAREST_VISIBLE_ATTACKABLE_PLAYER, NEAREST_VISIBLE_BABY_HOGLIN, NEAREST_VISIBLE_NEMESIS,
    NEAREST_VISIBLE_PLAYER, NEAREST_VISIBLE_ZOMBIFIED, RIDE_TARGET,
    TIME_TRYING_TO_REACH_ADMIRE_ITEM, UNIVERSAL_ANGER, VISIBLE_ADULT_HOGLIN_COUNT,
    VISIBLE_ADULT_PIGLIN_COUNT, WALK_TARGET,
};
use crate::entity::ai::brain::nearest_visible::{
    is_entity_attackable, is_entity_attackable_ignoring_line_of_sight,
};
use crate::entity::ai::brain::{ActivityData, AnyMemoryModuleType, BehaviorControl};
use crate::entity::ai::goal::random_pos::land_random_pos;
use crate::entity::entities::{HoglinEntity, ItemEntity};
use crate::entity::{
    Entity, LivingEntity, Mob, RemovalReason, SharedEntity, entity_loot_ref, living_loot_equipment,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::player::Player;
use crate::world::World;
use steel_protocol::packets::game::CTakeItemEntity;
use steel_registry::loot_table::LootContext;

/// Vanilla `PiglinAi.ADMIRE_DURATION`.
const ADMIRE_DURATION: i64 = 119;
/// Vanilla `PiglinAi.MAX_DISTANCE_TO_WALK_TO_ITEM`.
const MAX_DISTANCE_TO_WALK_TO_ITEM: i32 = 9;
/// Vanilla `PiglinAi.MAX_TIME_TO_WALK_TO_ITEM`.
const MAX_TIME_TO_WALK_TO_ITEM: i32 = 200;
/// Vanilla `PiglinAi.HOW_LONG_TIME_TO_DISABLE_ADMIRE_WALKING_IF_CANT_REACH_ITEM`.
const DISABLE_ADMIRE_WALKING_TIME: i64 = 200;
/// Vanilla `PiglinAi.CELEBRATION_TIME`.
const CELEBRATION_TIME: i64 = 300;
/// Vanilla `PiglinAi.BABY_FLEE_DURATION_AFTER_GETTING_HIT`.
const BABY_FLEE_DURATION_AFTER_GETTING_HIT: i64 = 100;
/// Vanilla `PiglinAi.HIT_BY_PLAYER_MEMORY_TIMEOUT`.
const HIT_BY_PLAYER_MEMORY_TIMEOUT: i64 = 400;
/// Vanilla `PiglinAi.MAX_WALK_DISTANCE_TO_START_RIDING`.
const MAX_WALK_DISTANCE_TO_START_RIDING: i32 = 8;
/// Vanilla `PiglinAi.RIDE_START_INTERVAL` (10 to 40 seconds).
const RIDE_START_INTERVAL: UniformInt = UniformInt::of(200, 800);
/// Vanilla `PiglinAi.RIDE_DURATION` (10 to 30 seconds).
const RIDE_DURATION: UniformInt = UniformInt::of(200, 600);
/// Vanilla `PiglinAi.RETREAT_DURATION` (5 to 20 seconds).
const RETREAT_DURATION: UniformInt = UniformInt::of(100, 400);
/// Vanilla `PiglinAi.MELEE_ATTACK_COOLDOWN`.
const MELEE_ATTACK_COOLDOWN: i32 = 20;
/// Vanilla `PiglinAi.EAT_COOLDOWN`.
const EAT_COOLDOWN: i64 = 200;
/// Vanilla `PiglinAi.DESIRED_DISTANCE_FROM_ENTITY_WHEN_AVOIDING`.
const DESIRED_DISTANCE_FROM_ENTITY_WHEN_AVOIDING: i32 = 12;
/// Vanilla `PiglinAi.MAX_LOOK_DIST`.
const MAX_LOOK_DIST: f32 = 8.0;
/// Vanilla `PiglinAi.MAX_LOOK_DIST_FOR_PLAYER_HOLDING_LOVED_ITEM`.
const MAX_LOOK_DIST_FOR_PLAYER_HOLDING_LOVED_ITEM: f32 = 14.0;
/// Vanilla `PiglinAi.INTERACTION_RANGE`.
const INTERACTION_RANGE: i32 = 8;
/// Vanilla `PiglinAi.MIN_DESIRED_DIST_FROM_TARGET_WHEN_HOLDING_CROSSBOW`.
const MIN_DESIRED_DIST_FROM_TARGET_WHEN_HOLDING_CROSSBOW: i32 = 5;
/// Vanilla `PiglinAi.SPEED_WHEN_STRAFING_BACK_FROM_TARGET`.
const SPEED_WHEN_STRAFING_BACK_FROM_TARGET: f32 = 0.75;
/// Vanilla `PiglinAi.DESIRED_DISTANCE_FROM_ZOMBIFIED`.
const DESIRED_DISTANCE_FROM_ZOMBIFIED: f64 = 6.0;
/// Vanilla `PiglinAi.AVOID_ZOMBIFIED_DURATION` (5 to 7 seconds).
const AVOID_ZOMBIFIED_DURATION: UniformInt = UniformInt::of(100, 140);
/// Vanilla `PiglinAi.BABY_AVOID_NEMESIS_DURATION` (5 to 7 seconds).
const BABY_AVOID_NEMESIS_DURATION: UniformInt = UniformInt::of(100, 140);
/// Vanilla `PiglinAi.PROBABILITY_OF_CELEBRATION_DANCE`.
const PROBABILITY_OF_CELEBRATION_DANCE: f32 = 0.1;
/// Vanilla `PiglinAi.isNearAvoidTarget` range.
const NEAR_AVOID_TARGET_DISTANCE: f64 = 12.0;
/// Vanilla `PiglinAi.avoidRepellent` distance.
const DESIRED_DISTANCE_FROM_REPELLENT: i32 = 8;
const SPEED_MULTIPLIER_WHEN_AVOIDING: f32 = 1.0;
const SPEED_MULTIPLIER_WHEN_RETREATING: f32 = 1.0;
const SPEED_MULTIPLIER_WHEN_MOUNTING: f32 = 0.8;
const SPEED_MULTIPLIER_WHEN_GOING_TO_WANTED_ITEM: f32 = 1.0;
const SPEED_MULTIPLIER_WHEN_GOING_TO_CELEBRATE_LOCATION: f32 = 1.0;
const SPEED_MULTIPLIER_WHEN_DANCING: f32 = 0.6;
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 0.6;
/// Vanilla `BehaviorUtils.throwItem` default throw velocity and hand offset.
const THROW_VELOCITY: f64 = 0.3;
const THROW_HAND_Y_DISTANCE_FROM_EYE: f32 = 0.3;

fn is_player(entity: &dyn LivingEntity) -> bool {
    entity.entity_type() == &vanilla_entities::PLAYER
}

fn has_memory(body: &dyn LivingEntity, memory: &'static dyn AnyMemoryModuleType) -> bool {
    body.brain_memories()
        .is_some_and(|memories| memories.lock().has_value(memory))
}

/// Vanilla `PiglinAi.wasHurtRecently`.
fn was_hurt_recently(body: &dyn LivingEntity) -> bool {
    has_memory(body, &HURT_BY)
}

/// Vanilla `PiglinAi.wantsToDance`: one in ten hoglin kills, seeded by the
/// game time.
fn wants_to_dance(world: &Arc<World>, _body: &PiglinEntity, killed_target: &SharedEntity) -> bool {
    killed_target.entity_type() == &vanilla_entities::HOGLIN
        && LegacyRandom::from_seed(world.game_time() as u64).next_f32()
            < PROBABILITY_OF_CELEBRATION_DANCE
}

/// Vanilla `PiglinAi.wantsToStopRiding`.
fn wants_to_stop_riding(body: &PiglinEntity, vehicle: &SharedEntity) -> bool {
    let Some(mob) = vehicle.as_mob() else {
        return false;
    };
    !mob.is_baby()
        || !LivingEntity::is_alive(mob)
        || was_hurt_recently(body)
        || was_hurt_recently(mob)
        || vehicle.downcast_ref::<PiglinEntity>().is_some() && mob.vehicle().is_none()
}

/// Vanilla `PiglinAi.createLookBehaviors` plus the trailing extra choice.
fn look_behaviors(
    extra: (Box<dyn BehaviorControl<PiglinEntity>>, i32),
) -> Vec<(Box<dyn BehaviorControl<PiglinEntity>>, i32)> {
    vec![
        (
            Box::new(set_entity_look_target(MAX_LOOK_DIST, is_player)),
            1,
        ),
        (
            Box::new(set_entity_look_target(MAX_LOOK_DIST, |entity| {
                entity.entity_type() == &vanilla_entities::PIGLIN
            })),
            1,
        ),
        (Box::new(set_entity_look_target(MAX_LOOK_DIST, |_| true)), 1),
        extra,
    ]
}

impl PiglinEntity {
    /// Vanilla `PiglinAi.getActivities`.
    pub(super) fn activities() -> Vec<ActivityData<Self>> {
        vec![
            Self::core_activity(),
            Self::idle_activity(),
            Self::admire_item_activity(),
            Self::fight_activity(),
            Self::celebrate_activity(),
            Self::retreat_activity(),
            Self::ride_hoglin_activity(),
        ]
    }

    fn core_activity() -> ActivityData<Self> {
        ActivityData::with_first_priority(
            Activity::CORE,
            0,
            vec![
                Box::new(TimedBehavior::new(LookAtTargetSink::new(45, 90))),
                Box::new(TimedBehavior::new(MoveToTargetSink::new())),
                Box::new(interact_with_door()),
                // Vanilla `babyAvoidNemesis`.
                Box::new(copy_memory_with_expiry(
                    |piglin: &Self| piglin.is_baby(),
                    &NEAREST_VISIBLE_NEMESIS,
                    &AVOID_TARGET,
                    BABY_AVOID_NEMESIS_DURATION,
                )),
                // Vanilla `avoidZombified`.
                Box::new(copy_memory_with_expiry(
                    Self::is_near_zombified,
                    &NEAREST_VISIBLE_ZOMBIFIED,
                    &AVOID_TARGET,
                    AVOID_ZOMBIFIED_DURATION,
                )),
                Box::new(stop_holding_item_if_no_longer_admiring()),
                Box::new(start_admiring_item_if_seen(ADMIRE_DURATION)),
                Box::new(start_celebrating_if_target_dead(
                    CELEBRATION_TIME,
                    wants_to_dance,
                )),
                Box::new(stop_being_angry_if_target_dead()),
            ],
        )
    }

    fn idle_activity() -> ActivityData<Self> {
        ActivityData::with_first_priority(
            Activity::IDLE,
            10,
            vec![
                Box::new(set_entity_look_target(
                    MAX_LOOK_DIST_FOR_PLAYER_HOLDING_LOVED_ITEM,
                    is_player_holding_loved_item,
                )),
                Box::new(Self::start_attacking_if_adult()),
                Box::new(trigger_if(
                    |piglin: &Self| piglin.can_hunt(),
                    start_hunting_hoglin(),
                )),
                Box::new(Self::avoid_repellent()),
                Box::new(Self::baby_sometimes_ride_baby_hoglin()),
                Box::new(Self::idle_look_behaviors()),
                Box::new(Self::idle_movement_behaviors()),
                Box::new(set_look_and_interact(
                    |entity: &SharedEntity| entity.entity_type() == &vanilla_entities::PLAYER,
                    4,
                )),
            ],
        )
    }

    fn fight_activity() -> ActivityData<Self> {
        ActivityData::with_first_priority(
            Activity::FIGHT,
            10,
            vec![
                Box::new(stop_attacking_if_target_invalid_when(
                    |world: &Arc<World>, piglin: &Self, target: &dyn LivingEntity| {
                        !piglin.is_nearest_valid_attack_target(world, target)
                    },
                    |_, _, _| {},
                )),
                Box::new(trigger_if(
                    Self::has_crossbow,
                    back_up_if_too_close(
                        MIN_DESIRED_DIST_FROM_TARGET_WHEN_HOLDING_CROSSBOW,
                        SPEED_WHEN_STRAFING_BACK_FROM_TARGET,
                    ),
                )),
                Box::new(set_walk_target_from_attack_target_if_target_out_of_reach(
                    |_| 1.0,
                )),
                Box::new(TimedBehavior::new(SpearApproach::new(1.0, 10.0))),
                Box::new(TimedBehavior::new(SpearAttack::new(1.0, 1.0, 2.0))),
                Box::new(TimedBehavior::new(SpearRetreat::new(1.0))),
                Box::new(melee_attack(MELEE_ATTACK_COOLDOWN)),
                Box::new(TimedBehavior::new(CrossbowAttack::new())),
                Box::new(remember_if_hoglin_was_killed()),
                Box::new(erase_memory_if(Self::is_near_zombified, &ATTACK_TARGET)),
            ],
        )
        .while_memory_present(&ATTACK_TARGET)
    }

    fn celebrate_activity() -> ActivityData<Self> {
        let wander: Vec<(Box<dyn BehaviorControl<Self>>, i32)> = vec![
            (
                Box::new(set_entity_look_target(MAX_LOOK_DIST, |entity| {
                    entity.entity_type() == &vanilla_entities::PIGLIN
                })),
                1,
            ),
            (
                Box::new(random_stroll_within(SPEED_MULTIPLIER_WHEN_IDLING, 2, 1)),
                1,
            ),
            (Box::new(DoNothing::new(10, 20)), 1),
        ];
        ActivityData::with_first_priority(
            Activity::CELEBRATE,
            10,
            vec![
                Box::new(Self::avoid_repellent()),
                Box::new(set_entity_look_target(
                    MAX_LOOK_DIST_FOR_PLAYER_HOLDING_LOVED_ITEM,
                    is_player_holding_loved_item,
                )),
                Box::new(Self::start_attacking_if_adult()),
                Box::new(trigger_if(
                    |piglin: &Self| !piglin.is_dancing(),
                    go_to_target_location(
                        &CELEBRATE_LOCATION,
                        2,
                        SPEED_MULTIPLIER_WHEN_GOING_TO_CELEBRATE_LOCATION,
                    ),
                )),
                Box::new(trigger_if(
                    Self::is_dancing,
                    go_to_target_location(&CELEBRATE_LOCATION, 4, SPEED_MULTIPLIER_WHEN_DANCING),
                )),
                Box::new(run_one(Vec::new(), wander)),
            ],
        )
        .while_memory_present(&CELEBRATE_LOCATION)
    }

    fn admire_item_activity() -> ActivityData<Self> {
        ActivityData::with_first_priority(
            Activity::ADMIRE_ITEM,
            10,
            vec![
                Box::new(go_to_wanted_item(
                    Self::is_not_holding_loved_item_in_off_hand,
                    SPEED_MULTIPLIER_WHEN_GOING_TO_WANTED_ITEM,
                    true,
                    MAX_DISTANCE_TO_WALK_TO_ITEM,
                )),
                Box::new(stop_admiring_if_item_too_far_away(
                    MAX_DISTANCE_TO_WALK_TO_ITEM,
                )),
                Box::new(stop_admiring_if_tired_of_trying_to_reach_item(
                    MAX_TIME_TO_WALK_TO_ITEM,
                    DISABLE_ADMIRE_WALKING_TIME,
                )),
            ],
        )
        .while_memory_present(&ADMIRING_ITEM)
    }

    fn retreat_activity() -> ActivityData<Self> {
        ActivityData::with_first_priority(
            Activity::AVOID,
            10,
            vec![
                Box::new(set_walk_target_away_from_entity(
                    &AVOID_TARGET,
                    SPEED_MULTIPLIER_WHEN_RETREATING,
                    DESIRED_DISTANCE_FROM_ENTITY_WHEN_AVOIDING,
                    true,
                )),
                Box::new(Self::idle_look_behaviors()),
                Box::new(Self::idle_movement_behaviors()),
                Box::new(erase_memory_if(Self::wants_to_stop_fleeing, &AVOID_TARGET)),
            ],
        )
        .while_memory_present(&AVOID_TARGET)
    }

    fn ride_hoglin_activity() -> ActivityData<Self> {
        let keep_still: Box<dyn BehaviorControl<Self>> = Box::new(OneShot::new(
            Vec::new(),
            |_: &Arc<World>, _: &Self, _: i64| true,
        ));
        ActivityData::with_first_priority(
            Activity::RIDE,
            10,
            vec![
                Box::new(mount(SPEED_MULTIPLIER_WHEN_MOUNTING)),
                Box::new(set_entity_look_target(
                    MAX_LOOK_DIST,
                    is_player_holding_loved_item,
                )),
                Box::new(trigger_if(
                    |piglin: &Self| piglin.is_passenger(),
                    trigger_one_shuffled(look_behaviors((keep_still, 1))),
                )),
                Box::new(dismount_or_skip_mounting(
                    MAX_WALK_DISTANCE_TO_START_RIDING,
                    wants_to_stop_riding,
                )),
            ],
        )
        .while_memory_present(&RIDE_TARGET)
    }

    /// Vanilla `StartAttacking.create((level, piglin) -> piglin.isAdult(),
    /// PiglinAi::findNearestValidAttackTarget)`.
    fn start_attacking_if_adult() -> impl BehaviorControl<Self> {
        start_attacking(|world: &Arc<World>, piglin: &Self| {
            if piglin.is_baby() {
                return None;
            }
            piglin.find_nearest_valid_attack_target(world)
        })
    }

    /// Vanilla `PiglinAi.avoidRepellent`.
    fn avoid_repellent() -> impl BehaviorControl<Self> {
        set_walk_target_away_from_pos(
            &NEAREST_REPELLENT,
            SPEED_MULTIPLIER_WHEN_AVOIDING,
            DESIRED_DISTANCE_FROM_REPELLENT,
            false,
        )
    }

    /// Vanilla `PiglinAi.babySometimesRideBabyHoglin`.
    fn baby_sometimes_ride_baby_hoglin() -> impl BehaviorControl<Self> {
        let mut ticker = Ticker::new(RIDE_START_INTERVAL);
        copy_memory_with_expiry(
            move |piglin: &Self| piglin.is_baby() && ticker.tick_down_and_check(),
            &NEAREST_VISIBLE_BABY_HOGLIN,
            &RIDE_TARGET,
            RIDE_DURATION,
        )
    }

    /// Vanilla `PiglinAi.createIdleLookBehaviors`.
    fn idle_look_behaviors() -> impl BehaviorControl<Self> {
        run_one(
            Vec::new(),
            look_behaviors((Box::new(DoNothing::new(30, 60)), 1)),
        )
    }

    /// Vanilla `PiglinAi.createIdleMovementBehaviors`.
    fn idle_movement_behaviors() -> impl BehaviorControl<Self> {
        let behaviors: Vec<(Box<dyn BehaviorControl<Self>>, i32)> = vec![
            (
                Box::new(random_stroll(SPEED_MULTIPLIER_WHEN_IDLING, true)),
                2,
            ),
            (
                Box::new(interact_with_type(
                    |entity: &SharedEntity| entity.entity_type() == &vanilla_entities::PIGLIN,
                    INTERACTION_RANGE,
                    &INTERACTION_TARGET,
                    SPEED_MULTIPLIER_WHEN_IDLING,
                    2,
                )),
                2,
            ),
            (
                Box::new(trigger_if(
                    |piglin: &Self| !piglin.sees_player_holding_loved_item(),
                    set_walk_target_from_look_target(|_| true, |_| SPEED_MULTIPLIER_WHEN_IDLING, 3),
                )),
                2,
            ),
            (Box::new(DoNothing::new(30, 60)), 1),
        ];
        run_one(Vec::new(), behaviors)
    }

    /// Vanilla `PiglinAi.updateActivity`.
    pub(super) fn update_activity(&self) {
        let old_activity = self.brain.active_non_core_activity();
        self.brain.set_active_activity_to_first_valid(&[
            Activity::ADMIRE_ITEM,
            Activity::FIGHT,
            Activity::AVOID,
            Activity::CELEBRATE,
            Activity::RIDE,
            Activity::IDLE,
        ]);
        let new_activity = self.brain.active_non_core_activity();
        if old_activity != new_activity
            && let Some(sound) = self.sound_for_current_activity()
        {
            self.make_sound(Some(sound));
        }
        let (has_target, has_ride_target, celebrating) = {
            let memories = self.memories().lock();
            (
                memories.has_value(&ATTACK_TARGET),
                memories.has_value(&RIDE_TARGET),
                memories.has_value(&CELEBRATE_LOCATION),
            )
        };
        self.set_aggressive(has_target);
        if !has_ride_target && self.is_baby_riding_baby() {
            self.stop_riding();
        }
        if !celebrating {
            self.memories().lock().erase(&DANCING);
        }
        let dancing = self.memories().lock().has_value(&DANCING);
        self.set_dancing(dancing);
    }

    /// Vanilla `PiglinAi.isBabyRidingBaby`.
    fn is_baby_riding_baby(&self) -> bool {
        if !self.is_baby() {
            return false;
        }
        self.vehicle().is_some_and(|vehicle| {
            let baby = vehicle
                .as_living_entity()
                .is_some_and(LivingEntity::is_baby);
            baby && (vehicle.downcast_ref::<Self>().is_some()
                || vehicle.downcast_ref::<HoglinEntity>().is_some())
        })
    }

    /// Vanilla `PiglinAi.pickUpItem`.
    pub(super) fn pick_up_item_piglin(&self, world: &Arc<World>, item_entity: &ItemEntity) {
        self.stop_walking();
        let mut source = item_entity.get_item();
        let taken = if source.is(&vanilla_items::GOLD_NUGGET) {
            self.take(world, item_entity, source.count());
            item_entity.set_removed(RemovalReason::Discarded);
            source
        } else {
            self.take(world, item_entity, 1);
            let taken = source.split(1);
            if source.is_empty() {
                item_entity.set_removed(RemovalReason::Discarded);
            } else {
                item_entity.set_item(source);
            }
            taken
        };

        if is_loved_item(&taken) {
            self.memories()
                .lock()
                .erase(&TIME_TRYING_TO_REACH_ADMIRE_ITEM);
            self.hold_in_off_hand_dropping_old(taken);
            self.admire_gold_item();
        } else if is_food(&taken) && !self.has_eaten_recently() {
            self.eat();
        } else if self.equip_item_if_possible(taken.clone()).is_empty() {
            self.put_in_inventory(taken);
        }
    }

    /// Vanilla `LivingEntity.take`: shows the item flying into the piglin.
    fn take(&self, world: &World, item_entity: &ItemEntity, count: i32) {
        if item_entity.is_removed() {
            return;
        }
        world.broadcast_to_nearby(
            ChunkPos::from_entity_pos(item_entity.position()),
            CTakeItemEntity::new(item_entity.id(), self.id(), count),
            None,
        );
    }

    /// Vanilla `PiglinAi.holdInOffhand`: drops what was in the off hand first.
    fn hold_in_off_hand_dropping_old(&self, item_stack: ItemStack) {
        let off_hand = self.equipment_in_slot(EquipmentSlot::OffHand);
        if !off_hand.is_empty() {
            let _ = self.spawn_at_location(off_hand, 0.0);
        }
        self.hold_in_off_hand(item_stack);
    }

    /// Vanilla `PiglinAi.stopWalking`.
    fn stop_walking(&self) {
        self.memories().lock().erase(&WALK_TARGET);
        self.mob_base().navigation().lock().stop();
    }

    /// Vanilla `PiglinAi.putInInventory`: what does not fit is thrown away.
    fn put_in_inventory(&self, item_stack: ItemStack) {
        let leftover = self.inventory().lock().add_item(&item_stack);
        self.throw_items_toward_pos(vec![leftover], self.random_nearby_pos());
    }

    /// Vanilla `PiglinAi.throwItems`: at the nearest visible player, or
    /// somewhere nearby.
    fn throw_items(&self, item_stacks: Vec<ItemStack>) {
        let player = self.memories().lock().get(&NEAREST_VISIBLE_PLAYER);
        let target = player.map_or_else(|| self.random_nearby_pos(), |player| player.position());
        self.throw_items_toward_pos(item_stacks, target);
    }

    /// Vanilla `PiglinAi.throwItemsTowardPos`.
    fn throw_items_toward_pos(&self, item_stacks: Vec<ItemStack>, target_pos: DVec3) {
        if item_stacks.is_empty() {
            return;
        }
        self.swing(InteractionHand::OffHand, SwingAnimation::DEFAULT, false);
        for item_stack in item_stacks {
            // Vanilla also spawns an empty item entity for an empty leftover,
            // which discards itself on its first tick.
            if item_stack.is_empty() {
                continue;
            }
            throw_item(
                self,
                item_stack,
                target_pos + DVec3::Y,
                DVec3::splat(THROW_VELOCITY),
                THROW_HAND_Y_DISTANCE_FROM_EYE,
            );
        }
    }

    /// Vanilla `PiglinAi.getRandomNearbyPos`.
    fn random_nearby_pos(&self) -> DVec3 {
        land_random_pos(self, 4, 2).unwrap_or_else(|| self.position())
    }

    /// Vanilla `PiglinAi.getBarterResponseItems`.
    fn barter_response_items(&self) -> Vec<ItemStack> {
        let mut rng = rand::rng();
        let equipment = living_loot_equipment(self);
        // Vanilla `LootContextParamSets.PIGLIN_BARTER`: THIS_ENTITY only.
        let mut context =
            LootContext::new(&mut rng).with_this_entity(entity_loot_ref(self, Some(&equipment)));
        vanilla_loot_tables::GAMEPLAY_PIGLIN_BARTERING.get_random_items(&mut context)
    }

    /// Vanilla `PiglinAi.cancelAdmiring`: drops the admired item.
    pub(super) fn cancel_admiring(&self) {
        let off_hand = self.equipment_in_slot(EquipmentSlot::OffHand);
        if self.is_admiring_item() && !off_hand.is_empty() {
            let _ = self.spawn_at_location(off_hand, 0.0);
            self.set_item_slot(EquipmentSlot::OffHand, ItemStack::empty());
        }
    }

    /// Vanilla `PiglinAi.mobInteract`: an adult takes one gold ingot to
    /// admire and barter for.
    pub(super) fn mob_interact_piglin(
        &self,
        player: &Player,
        hand: InteractionHand,
    ) -> InteractionResult {
        let held = player.inventory.lock().get_item_in_hand(hand).clone();
        if !self.can_admire(&held) {
            return InteractionResult::Pass;
        }
        let taken = held.copy_with_count(1);
        self.use_player_item(player, hand);
        self.hold_in_off_hand_dropping_old(taken);
        self.admire_gold_item();
        self.stop_walking();
        InteractionResult::Success
    }

    /// Vanilla `PiglinAi.canAdmire`.
    fn can_admire(&self, item_stack: &ItemStack) -> bool {
        !self.is_admiring_disabled()
            && !self.is_admiring_item()
            && !self.is_baby()
            && is_barter_currency(item_stack)
    }

    /// Vanilla `PiglinAi.wasHurtBy`.
    pub(super) fn was_hurt_by(&self, world: &World, attacker: &SharedEntity) {
        if attacker.downcast_ref::<Self>().is_some() {
            return;
        }
        if !self.equipment_in_slot(EquipmentSlot::OffHand).is_empty() {
            stop_holding_off_hand_item(world, self, false);
        }
        {
            let mut memories = self.memories().lock();
            memories.erase(&CELEBRATE_LOCATION);
            memories.erase(&DANCING);
            memories.erase(&ADMIRING_ITEM);
            if attacker.as_player().is_some() {
                memories.set_with_expiry(&ADMIRING_DISABLED, true, HIT_BY_PLAYER_MEMORY_TIMEOUT);
            }
            let other_kind = memories
                .get_ref(&AVOID_TARGET)
                .is_some_and(|avoid| avoid.entity_type() != attacker.entity_type());
            if other_kind {
                memories.erase(&AVOID_TARGET);
            }
        }
        if self.is_baby() {
            self.memories().lock().set_with_expiry(
                &AVOID_TARGET,
                Arc::clone(attacker),
                BABY_FLEE_DURATION_AFTER_GETTING_HIT,
            );
            let attackable = attacker.as_living_entity().is_some_and(|living| {
                is_entity_attackable_ignoring_line_of_sight(world, self, living)
            });
            if attackable {
                broadcast_anger_target(world, self, attacker);
            }
        } else if attacker.entity_type() == &vanilla_entities::HOGLIN
            && self.hoglins_outnumber_piglins()
        {
            self.set_avoid_target_and_dont_hunt_for_a_while(Arc::clone(attacker));
            self.broadcast_retreat(attacker);
        } else {
            maybe_retaliate(world, self, attacker);
        }
    }

    /// Vanilla `PiglinAi.getSoundForCurrentActivity`.
    pub(super) fn sound_for_current_activity(&self) -> Option<SoundEventRef> {
        let activity = self.brain.active_non_core_activity()?;
        Some(if activity == Activity::FIGHT {
            &sound_events::ENTITY_PIGLIN_ANGRY
        } else if self.is_converting() || activity == Activity::AVOID && self.is_near_avoid_target()
        {
            &sound_events::ENTITY_PIGLIN_RETREAT
        } else if activity == Activity::ADMIRE_ITEM {
            &sound_events::ENTITY_PIGLIN_ADMIRING_ITEM
        } else if activity == Activity::CELEBRATE {
            &sound_events::ENTITY_PIGLIN_CELEBRATE
        } else if self.sees_player_holding_loved_item() {
            &sound_events::ENTITY_PIGLIN_JEALOUS
        } else if has_memory(self, &NEAREST_REPELLENT) {
            &sound_events::ENTITY_PIGLIN_RETREAT
        } else {
            &sound_events::ENTITY_PIGLIN_AMBIENT
        })
    }

    fn is_near_avoid_target(&self) -> bool {
        self.memories()
            .lock()
            .get_ref(&AVOID_TARGET)
            .is_some_and(|avoid| {
                avoid.position().distance_squared(self.position())
                    < NEAR_AVOID_TARGET_DISTANCE * NEAR_AVOID_TARGET_DISTANCE
            })
    }

    /// Vanilla `PiglinAi.wantsToPickup`.
    pub(super) fn wants_to_pickup(&self, item_stack: &ItemStack) -> bool {
        let item = item_stack.item();
        if self.is_baby() && item.has_tag(&ItemTag::IGNORED_BY_PIGLIN_BABIES)
            || item.has_tag(&ItemTag::PIGLIN_REPELLENTS)
            || self.is_admiring_disabled() && has_memory(self, &ATTACK_TARGET)
        {
            return false;
        }
        if is_barter_currency(item_stack) {
            return self.is_not_holding_loved_item_in_off_hand();
        }
        let has_space = self.inventory().lock().can_add_item(item_stack);
        if item_stack.is(&vanilla_items::GOLD_NUGGET) {
            has_space
        } else if is_food(item_stack) {
            !self.has_eaten_recently() && has_space
        } else if is_loved_item(item_stack) {
            self.is_not_holding_loved_item_in_off_hand() && has_space
        } else {
            self.can_replace_current_item_in_its_slot(item_stack)
        }
    }

    /// Vanilla `PiglinAi.isNearZombified`.
    fn is_near_zombified(&self) -> bool {
        self.memories()
            .lock()
            .get_ref(&NEAREST_VISIBLE_ZOMBIFIED)
            .is_some_and(|zombified| {
                zombified.position().distance_squared(self.position())
                    < DESIRED_DISTANCE_FROM_ZOMBIFIED * DESIRED_DISTANCE_FROM_ZOMBIFIED
            })
    }

    /// Vanilla `PiglinAi.findNearestValidAttackTarget`.
    fn find_nearest_valid_attack_target(&self, world: &World) -> Option<SharedEntity> {
        if self.is_near_zombified() {
            return None;
        }
        if let Some(angry_at) = anger_target(world, self)
            && angry_at.as_living_entity().is_some_and(|living| {
                is_entity_attackable_ignoring_line_of_sight(world, self, living)
            })
        {
            return Some(angry_at);
        }
        let memories = self.memories().lock();
        if memories.has_value(&UNIVERSAL_ANGER)
            && let Some(player) = memories.get(&NEAREST_VISIBLE_ATTACKABLE_PLAYER)
        {
            return Some(player);
        }
        if let Some(nemesis) = memories.get(&NEAREST_VISIBLE_NEMESIS) {
            return Some(nemesis);
        }
        let player = memories.get(&NEAREST_TARGETABLE_PLAYER_NOT_WEARING_GOLD)?;
        drop(memories);
        player
            .as_living_entity()
            .is_some_and(|living| is_entity_attackable(world, self, living))
            .then_some(player)
    }

    /// Vanilla `PiglinAi.isNearestValidAttackTarget`.
    fn is_nearest_valid_attack_target(&self, world: &World, target: &dyn LivingEntity) -> bool {
        self.find_nearest_valid_attack_target(world)
            .is_some_and(|nearest| nearest.id() == target.id())
    }

    /// Vanilla `PiglinAi.broadcastRetreat`.
    fn broadcast_retreat(&self, target: &SharedEntity) {
        let piglins = self
            .memories()
            .lock()
            .get(&NEAREST_VISIBLE_ADULT_PIGLINS)
            .unwrap_or_default();
        for piglin in piglins {
            if let Some(piglin) = piglin.downcast_ref::<Self>() {
                piglin.retreat_from_nearest_target(Arc::clone(target));
            }
        }
    }

    /// Vanilla `PiglinAi.retreatFromNearestTarget`.
    fn retreat_from_nearest_target(&self, new_avoid_target: SharedEntity) {
        let (avoid_target, attack_target) = {
            let memories = self.memories().lock();
            (memories.get(&AVOID_TARGET), memories.get(&ATTACK_TARGET))
        };
        let nearest = get_nearest_target(self, avoid_target, new_avoid_target);
        let nearest = get_nearest_target(self, attack_target, nearest);
        self.set_avoid_target_and_dont_hunt_for_a_while(nearest);
    }

    /// Vanilla `PiglinAi.wantsToStopFleeing`.
    fn wants_to_stop_fleeing(&self) -> bool {
        let memories = self.memories().lock();
        let Some(avoided) = memories.get_ref(&AVOID_TARGET) else {
            return true;
        };
        if avoided.entity_type() == &vanilla_entities::HOGLIN {
            drop(memories);
            return !self.hoglins_outnumber_piglins();
        }
        let still_seen = memories
            .get_ref(&NEAREST_VISIBLE_ZOMBIFIED)
            .is_some_and(|zombified| zombified.id() == avoided.id());
        is_zombified(avoided.as_ref()) && !still_seen
    }

    /// Vanilla `PiglinAi.hoglinsOutnumberPiglins`.
    fn hoglins_outnumber_piglins(&self) -> bool {
        let memories = self.memories().lock();
        let piglin_count = memories.get(&VISIBLE_ADULT_PIGLIN_COUNT).unwrap_or(0) + 1;
        let hoglin_count = memories.get(&VISIBLE_ADULT_HOGLIN_COUNT).unwrap_or(0);
        hoglin_count > piglin_count
    }

    /// Vanilla `PiglinAi.setAvoidTargetAndDontHuntForAWhile`.
    fn set_avoid_target_and_dont_hunt_for_a_while(&self, target: SharedEntity) {
        {
            let mut memories = self.memories().lock();
            memories.erase(&ANGRY_AT);
            memories.erase(&ATTACK_TARGET);
            memories.erase(&WALK_TARGET);
            memories.set_with_expiry(&AVOID_TARGET, target, i64::from(RETREAT_DURATION.sample()));
        }
        dont_kill_any_more_hoglins_for_a_while(self);
    }

    /// Vanilla `PiglinAi.eat`.
    fn eat(&self) {
        self.memories()
            .lock()
            .set_with_expiry(&ATE_RECENTLY, true, EAT_COOLDOWN);
    }

    fn has_eaten_recently(&self) -> bool {
        has_memory(self, &ATE_RECENTLY)
    }

    /// Vanilla `PiglinAi.admireGoldItem`.
    fn admire_gold_item(&self) {
        self.memories()
            .lock()
            .set_with_expiry(&ADMIRING_ITEM, true, ADMIRE_DURATION);
    }

    fn is_admiring_item(&self) -> bool {
        has_memory(self, &ADMIRING_ITEM)
    }

    fn is_admiring_disabled(&self) -> bool {
        has_memory(self, &ADMIRING_DISABLED)
    }

    fn sees_player_holding_loved_item(&self) -> bool {
        has_memory(self, &NEAREST_PLAYER_HOLDING_WANTED_ITEM)
    }

    /// Vanilla `PiglinAi.hasCrossbow`.
    fn has_crossbow(&self) -> bool {
        self.is_holding(&mut |stack| stack.is(&vanilla_items::CROSSBOW))
    }

    /// Vanilla `PiglinAi.isNotHoldingLovedItemInOffHand`.
    fn is_not_holding_loved_item_in_off_hand(&self) -> bool {
        let off_hand = self.equipment_in_slot(EquipmentSlot::OffHand);
        off_hand.is_empty() || !is_loved_item(&off_hand)
    }
}

/// Vanilla `PiglinAi.isBarterCurrency`.
fn is_barter_currency(item_stack: &ItemStack) -> bool {
    item_stack.is(&vanilla_items::GOLD_INGOT)
}

/// Vanilla `PiglinAi.isFood`.
fn is_food(item_stack: &ItemStack) -> bool {
    item_stack.item().has_tag(&ItemTag::PIGLIN_FOOD)
}

/// Vanilla `PiglinAi.stopHoldingOffHandItem`: an adult barters a gold ingot
/// (when `bartering_enabled`) or keeps anything else; a baby swaps it into
/// its main hand.
pub(super) fn stop_holding_off_hand_item(
    world: &World,
    body: &PiglinEntity,
    bartering_enabled: bool,
) {
    let _ = world;
    let item_stack = body.equipment_in_slot(EquipmentSlot::OffHand);
    body.set_item_slot(EquipmentSlot::OffHand, ItemStack::empty());
    if !body.is_baby() {
        let barter_currency = is_barter_currency(&item_stack);
        if bartering_enabled && barter_currency {
            body.throw_items(body.barter_response_items());
        } else if !barter_currency && body.equip_item_if_possible(item_stack.clone()).is_empty() {
            body.put_in_inventory(item_stack);
        }
        return;
    }
    if !body.equip_item_if_possible(item_stack.clone()).is_empty() {
        return;
    }
    let main_hand = body.equipment_in_slot(EquipmentSlot::MainHand);
    if is_loved_item(&main_hand) {
        body.put_in_inventory(main_hand);
    } else {
        body.throw_items(vec![main_hand]);
    }
    body.hold_in_main_hand(item_stack);
}
