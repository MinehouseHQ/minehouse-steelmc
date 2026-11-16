//! Vanilla `HappyGhastAi`: the ghastling's brain. A grown happy ghast runs
//! on goals instead.

use super::HappyGhastEntity;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::TimedBehavior;
use crate::entity::ai::brain::behaviors::{
    AnimalPanic, CountDownCooldownTicks, FollowTemptation, LookAtTargetSink, MoveToTargetSink,
    Swim, UniformInt, baby_follow_adult, random_fly, run_one, set_walk_target_from_look_target,
};
use crate::entity::ai::brain::memory::{AnyMemoryModuleType, MemoryRequirement};
use crate::entity::ai::brain::memory_types::{
    IS_PANICKING, NEAREST_VISIBLE_ADULT, NEAREST_VISIBLE_PLAYER, TEMPTATION_COOLDOWN_TICKS,
};
use crate::entity::ai::brain::sensors::{
    AdultSensorAnyType, HurtBySensor, NearestLivingEntitySensor, PlayerSensor, TemptingSensor,
};
use crate::entity::ai::brain::{ActivityData, BehaviorControl, MemoryStatus, Sensor};

const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
const SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 1.25;
const SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT: f32 = 1.1;
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 2.0;
/// Vanilla `BABY_GHAST_CLOSE_ENOUGH_DIST`.
const BABY_GHAST_CLOSE_ENOUGH_DIST: f64 = 3.0;
/// Vanilla `ADULT_FOLLOW_RANGE`.
const ADULT_FOLLOW_RANGE: UniformInt = UniformInt::of(3, 16);
const SWIM_CHANCE: f32 = 0.8;
const LOOK_TARGET_CLOSE_ENOUGH: i32 = 3;

/// Vanilla's happy ghast `Brain.provider` lists only sensors.
pub(super) fn memories() -> [&'static dyn AnyMemoryModuleType; 0] {
    []
}

/// The sensors vanilla's happy ghast `Brain.provider` lists.
pub(super) fn sensors() -> Vec<Box<dyn Sensor<HappyGhastEntity>>> {
    vec![
        Box::new(NearestLivingEntitySensor),
        Box::new(HurtBySensor),
        Box::new(TemptingSensor::for_animal()),
        Box::new(AdultSensorAnyType),
        Box::new(PlayerSensor),
    ]
}

/// Vanilla `HappyGhastAi.getActivities`.
pub(super) fn activities() -> Vec<ActivityData<HappyGhastEntity>> {
    vec![core_activity(), idle_activity(), panic_activity()]
}

fn core_activity() -> ActivityData<HappyGhastEntity> {
    ActivityData::with_first_priority(
        Activity::CORE,
        0,
        vec![
            Box::new(TimedBehavior::new(Swim::new(SWIM_CHANCE))),
            Box::new(TimedBehavior::new(AnimalPanic::new(
                SPEED_MULTIPLIER_WHEN_PANICKING,
            ))),
            Box::new(TimedBehavior::new(LookAtTargetSink::new(45, 90))),
            Box::new(TimedBehavior::new(MoveToTargetSink::new())),
            Box::new(TimedBehavior::new(CountDownCooldownTicks::new(
                &TEMPTATION_COOLDOWN_TICKS,
            ))),
        ],
    )
}

fn idle_activity() -> ActivityData<HappyGhastEntity> {
    let wander: Vec<(Box<dyn BehaviorControl<HappyGhastEntity>>, i32)> = vec![
        (Box::new(random_fly(SPEED_MULTIPLIER_WHEN_IDLING)), 1),
        (
            Box::new(set_walk_target_from_look_target(
                |_| true,
                |_| SPEED_MULTIPLIER_WHEN_IDLING,
                LOOK_TARGET_CLOSE_ENOUGH,
            )),
            1,
        ),
    ];
    ActivityData::new(
        Activity::IDLE,
        vec![
            (
                1,
                Box::new(TimedBehavior::new(FollowTemptation::with_distance(
                    |_| SPEED_MULTIPLIER_WHEN_TEMPTED,
                    |_| BABY_GHAST_CLOSE_ENOUGH_DIST,
                    true,
                ))),
            ),
            (
                2,
                Box::new(baby_follow_adult(
                    ADULT_FOLLOW_RANGE,
                    |_| SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT,
                    &NEAREST_VISIBLE_PLAYER,
                    true,
                )),
            ),
            (
                3,
                Box::new(baby_follow_adult(
                    ADULT_FOLLOW_RANGE,
                    |_| SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT,
                    &NEAREST_VISIBLE_ADULT,
                    true,
                )),
            ),
            (4, Box::new(run_one(Vec::new(), wander))),
        ],
    )
}

fn panic_activity() -> ActivityData<HappyGhastEntity> {
    ActivityData::new(Activity::PANIC, Vec::new()).with_conditions(vec![MemoryRequirement::new(
        &IS_PANICKING,
        MemoryStatus::ValuePresent,
    )])
}

/// Vanilla `HappyGhastAi.updateActivity`.
pub(super) fn update_activity(body: &HappyGhastEntity) {
    body.brain
        .set_active_activity_to_first_valid(&[Activity::PANIC, Activity::IDLE]);
}
