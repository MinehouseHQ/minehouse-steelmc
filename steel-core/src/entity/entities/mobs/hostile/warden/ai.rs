//! Vanilla `WardenAi` and the warden behaviors of
//! `net.minecraft.world.entity.ai.behavior.warden`.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::entity_type::EntityAttachment;
use steel_registry::particle_type::ParticleData;
use steel_registry::vanilla_particle_types::SONIC_BOOM as SONIC_BOOM_PARTICLE;
use steel_registry::{sound_events, vanilla_attributes, vanilla_damage_types};
use steel_utils::BlockPos;
use steel_utils::entity_events::EntityStatus;

use super::WardenEntity;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::{Behavior, OneShot, TimedBehavior};
use crate::entity::ai::brain::behaviors::utils::look_at_entity;
use crate::entity::ai::brain::behaviors::{
    DoNothing, LookAtTargetSink, MoveToTargetSink, Swim, UniformInt, go_to_target_location,
    melee_attack, random_stroll, run_one,
    set_walk_target_from_attack_target_if_target_out_of_reach,
    stop_attacking_if_target_invalid_full,
};
use crate::entity::ai::brain::memory_types::{
    ATTACK_COOLING_DOWN, ATTACK_TARGET, CANT_REACH_WALK_TARGET_SINCE, DIG_COOLDOWN,
    DISTURBANCE_LOCATION, IS_EMERGING, IS_SNIFFING, LOOK_TARGET, NEAREST_ATTACKABLE,
    NEAREST_VISIBLE_LIVING_ENTITIES, PositionTracker, ROAR_SOUND_COOLDOWN, ROAR_SOUND_DELAY,
    ROAR_TARGET, SNIFF_COOLDOWN, SONIC_BOOM_COOLDOWN, SONIC_BOOM_SOUND_COOLDOWN,
    SONIC_BOOM_SOUND_DELAY, WALK_TARGET,
};
use crate::entity::ai::brain::{ActivityData, BehaviorControl, MemoryRequirement, MemoryStatus};
use crate::entity::callback::RemovalReason;
use crate::entity::damage::DamageSource;
use crate::entity::{Entity, EntityPose, LivingEntity, Mob, SharedEntity};
use crate::world::World;

/// Vanilla `SPEED_MULTIPLIER_WHEN_IDLING`.
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 0.5;
/// Vanilla `SPEED_MULTIPLIER_WHEN_INVESTIGATING`.
const SPEED_MULTIPLIER_WHEN_INVESTIGATING: f32 = 0.7;
/// Vanilla `SPEED_MULTIPLIER_WHEN_FIGHTING`.
const SPEED_MULTIPLIER_WHEN_FIGHTING: f32 = 1.2;
/// Vanilla `MELEE_ATTACK_COOLDOWN`.
const MELEE_ATTACK_COOLDOWN: i32 = 18;
/// Vanilla `DIGGING_DURATION`: `ceil(100)`.
const DIGGING_DURATION: i32 = 100;
/// Vanilla `EMERGE_DURATION`: `ceil(133.59999)`.
pub(super) const EMERGE_DURATION: i32 = 134;
/// Vanilla `ROAR_DURATION`: `ceil(84)`.
const ROAR_DURATION: i32 = 84;
/// Vanilla `SNIFFING_DURATION`: `ceil(83.2)`.
const SNIFFING_DURATION: i32 = 84;
/// Vanilla `DIGGING_COOLDOWN`.
pub(super) const DIGGING_COOLDOWN: i64 = 1200;
/// Vanilla `DISTURBANCE_LOCATION_EXPIRY_TIME`.
const DISTURBANCE_LOCATION_EXPIRY_TIME: i64 = 100;
/// Vanilla `TryToSniff.SNIFF_COOLDOWN`.
const TRY_TO_SNIFF_COOLDOWN: UniformInt = UniformInt::of(100, 200);
/// Vanilla `Roar.TICKS_BEFORE_PLAYING_ROAR_SOUND`.
const TICKS_BEFORE_PLAYING_ROAR_SOUND: i64 = 25;
/// Vanilla `Roar.ROAR_ANGER_INCREASE`.
const ROAR_ANGER_INCREASE: i32 = 20;
/// Vanilla `SonicBoom.DISTANCE_XZ` and `DISTANCE_Y`.
const SONIC_BOOM_DISTANCE_XZ: f64 = 15.0;
const SONIC_BOOM_DISTANCE_Y: f64 = 20.0;
/// Vanilla `SonicBoom.KNOCKBACK_VERTICAL` and `KNOCKBACK_HORIZONTAL`.
const SONIC_BOOM_KNOCKBACK_VERTICAL: f64 = 0.5;
const SONIC_BOOM_KNOCKBACK_HORIZONTAL: f64 = 2.5;
/// Vanilla `SonicBoom.COOLDOWN`.
pub(super) const SONIC_BOOM_COOLDOWN_TICKS: i64 = 40;
/// Vanilla `SonicBoom.TICKS_BEFORE_PLAYING_SOUND`.
const SONIC_BOOM_TICKS_BEFORE_PLAYING_SOUND: i64 = 34;
/// Vanilla `SonicBoom.DURATION`.
const SONIC_BOOM_DURATION: i32 = 60;
/// Vanilla sonic boom damage.
const SONIC_BOOM_DAMAGE: f32 = 10.0;
/// Vanilla `Sniffing.ANGER_FROM_SNIFFING_MAX_DISTANCE_XZ` and `_Y`.
const ANGER_FROM_SNIFFING_MAX_DISTANCE_XZ: f64 = 6.0;
const ANGER_FROM_SNIFFING_MAX_DISTANCE_Y: f64 = 20.0;
/// Vanilla `Warden.createAttributes` follow range, which `initFightActivity`
/// reads when the brain is made.
const FOLLOW_RANGE: f32 = 24.0;

/// Vanilla `WardenAi.getActivities`.
pub(super) fn activities() -> Vec<ActivityData<WardenEntity>> {
    vec![
        core_activity(),
        emerge_activity(),
        digging_activity(),
        idle_activity(),
        roar_activity(),
        fight_activity(),
        investigate_activity(),
        sniffing_activity(),
    ]
}

/// Vanilla `WardenAi.updateActivity`.
pub(super) fn update_activity(warden: &WardenEntity) {
    warden.brain.set_active_activity_to_first_valid(&[
        Activity::EMERGE,
        Activity::DIG,
        Activity::ROAR,
        Activity::FIGHT,
        Activity::INVESTIGATE,
        Activity::SNIFF,
        Activity::IDLE,
    ]);
}

fn core_activity() -> ActivityData<WardenEntity> {
    ActivityData::with_first_priority(
        Activity::CORE,
        0,
        vec![
            Box::new(TimedBehavior::new(Swim::new(0.8))),
            Box::new(set_warden_look_target()),
            Box::new(TimedBehavior::new(LookAtTargetSink::new(45, 90))),
            Box::new(TimedBehavior::new(MoveToTargetSink::new())),
        ],
    )
}

fn emerge_activity() -> ActivityData<WardenEntity> {
    ActivityData::with_first_priority(
        Activity::EMERGE,
        5,
        vec![Box::new(TimedBehavior::new(Emerging))],
    )
    .while_memory_present(&IS_EMERGING)
}

fn digging_activity() -> ActivityData<WardenEntity> {
    ActivityData::new(
        Activity::DIG,
        vec![
            (0, Box::new(TimedBehavior::new(ForceUnmount))),
            (1, Box::new(TimedBehavior::new(Digging))),
        ],
    )
    .with_conditions(vec![
        MemoryRequirement::new(&ROAR_TARGET, MemoryStatus::ValueAbsent),
        MemoryRequirement::new(&DIG_COOLDOWN, MemoryStatus::ValueAbsent),
    ])
}

fn idle_activity() -> ActivityData<WardenEntity> {
    let wander: Vec<(Box<dyn BehaviorControl<WardenEntity>>, i32)> = vec![
        (
            Box::new(random_stroll(SPEED_MULTIPLIER_WHEN_IDLING, true)),
            2,
        ),
        (Box::new(DoNothing::new(30, 60)), 1),
    ];
    ActivityData::with_first_priority(
        Activity::IDLE,
        10,
        vec![
            Box::new(set_roar_target()),
            Box::new(try_to_sniff()),
            Box::new(run_one(
                vec![MemoryRequirement::new(
                    &IS_SNIFFING,
                    MemoryStatus::ValueAbsent,
                )],
                wander,
            )),
        ],
    )
}

fn investigate_activity() -> ActivityData<WardenEntity> {
    ActivityData::with_first_priority(
        Activity::INVESTIGATE,
        5,
        vec![
            Box::new(set_roar_target()),
            Box::new(go_to_target_location(
                &DISTURBANCE_LOCATION,
                2,
                SPEED_MULTIPLIER_WHEN_INVESTIGATING,
            )),
        ],
    )
    .while_memory_present(&DISTURBANCE_LOCATION)
}

fn sniffing_activity() -> ActivityData<WardenEntity> {
    ActivityData::with_first_priority(
        Activity::SNIFF,
        5,
        vec![
            Box::new(set_roar_target()),
            Box::new(TimedBehavior::new(Sniffing)),
        ],
    )
    .while_memory_present(&IS_SNIFFING)
}

fn roar_activity() -> ActivityData<WardenEntity> {
    ActivityData::with_first_priority(Activity::ROAR, 10, vec![Box::new(TimedBehavior::new(Roar))])
        .while_memory_present(&ROAR_TARGET)
}

fn fight_activity() -> ActivityData<WardenEntity> {
    ActivityData::with_first_priority(
        Activity::FIGHT,
        10,
        vec![
            Box::new(dig_cooldown_setter()),
            Box::new(stop_attacking_if_target_invalid_full(
                |_world: &Arc<World>, body: &WardenEntity, target: &dyn LivingEntity| {
                    !body.anger_level().is_angry() || !body.can_target_entity(target)
                },
                |_world: &Arc<World>, body: &WardenEntity, target: &SharedEntity| {
                    on_target_invalid(body, target);
                },
                false,
            )),
            Box::new(set_attack_target_look_target()),
            Box::new(set_walk_target_from_attack_target_if_target_out_of_reach(
                |_| SPEED_MULTIPLIER_WHEN_FIGHTING,
            )),
            Box::new(TimedBehavior::new(SonicBoom)),
            Box::new(melee_attack(MELEE_ATTACK_COOLDOWN)),
        ],
    )
    .while_memory_present(&ATTACK_TARGET)
}

/// Vanilla `DIG_COOLDOWN_SETTER`: fighting keeps the dig cooldown fresh.
fn dig_cooldown_setter() -> impl BehaviorControl<WardenEntity> {
    OneShot::new(
        vec![MemoryRequirement::new(
            &DIG_COOLDOWN,
            MemoryStatus::Registered,
        )],
        |_world: &Arc<World>, body: &WardenEntity, _time: i64| {
            let mut memories = body.brain.memories().lock();
            if memories.has_value(&DIG_COOLDOWN) {
                memories.set_with_expiry(&DIG_COOLDOWN, (), DIGGING_COOLDOWN);
            }
            true
        },
    )
}

/// Vanilla `SetEntityLookTarget.create(entity -> isTarget(body, entity),
/// followRange)`: looks at the attack target while it is the closest visible
/// match within follow range.
fn set_attack_target_look_target() -> impl BehaviorControl<WardenEntity> {
    let max_dist_sqr = f64::from(FOLLOW_RANGE * FOLLOW_RANGE);
    OneShot::new(
        vec![
            MemoryRequirement::new(&LOOK_TARGET, MemoryStatus::ValueAbsent),
            MemoryRequirement::new(&NEAREST_VISIBLE_LIVING_ENTITIES, MemoryStatus::ValuePresent),
        ],
        move |_world: &Arc<World>, body: &WardenEntity, _time: i64| {
            let (visible, attack_target) = {
                let memories = body.brain.memories().lock();
                (
                    memories.get(&NEAREST_VISIBLE_LIVING_ENTITIES),
                    memories.get(&ATTACK_TARGET),
                )
            };
            let (Some(visible), Some(attack_target)) = (visible, attack_target) else {
                return false;
            };
            let position = body.position();
            let Some(target) = visible.find_closest(|entity| {
                entity.id() == attack_target.id()
                    && entity.position().distance_squared(position) <= max_dist_sqr
                    && !body.has_passenger(entity.as_ref())
            }) else {
                return false;
            };
            body.brain
                .memories()
                .lock()
                .set(&LOOK_TARGET, Some(PositionTracker::entity(target, true)));
            true
        },
    )
}

/// Vanilla `WardenAi.onTargetInvalid`.
fn on_target_invalid(body: &WardenEntity, target: &SharedEntity) {
    if !body.can_target_entity(target.as_ref()) {
        body.clear_anger(target.as_ref());
    }
    set_dig_cooldown(body);
}

/// Vanilla `WardenAi.setDigCooldown`: renews a running dig cooldown.
pub(super) fn set_dig_cooldown(body: &WardenEntity) {
    let mut memories = body.brain.memories().lock();
    if memories.has_value(&DIG_COOLDOWN) {
        memories.set_with_expiry(&DIG_COOLDOWN, (), DIGGING_COOLDOWN);
    }
}

/// Vanilla `WardenAi.setDisturbanceLocation`: a calm warden goes to look.
pub(super) fn set_disturbance_location(body: &WardenEntity, position: BlockPos) {
    let Some(world) = body.level() else {
        return;
    };
    if !world
        .world_border_snapshot()
        .is_block_within_bounds(position)
        || body.entity_angry_at().is_some()
        || body.brain.memories().lock().has_value(&ATTACK_TARGET)
    {
        return;
    }
    set_dig_cooldown(body);
    let mut memories = body.brain.memories().lock();
    memories.set_with_expiry(&SNIFF_COOLDOWN, (), DISTURBANCE_LOCATION_EXPIRY_TIME);
    memories.set_with_expiry(
        &LOOK_TARGET,
        PositionTracker::block(position),
        DISTURBANCE_LOCATION_EXPIRY_TIME,
    );
    memories.set_with_expiry(
        &DISTURBANCE_LOCATION,
        position,
        DISTURBANCE_LOCATION_EXPIRY_TIME,
    );
    memories.erase(&WALK_TARGET);
}

/// Vanilla `SonicBoom.setCooldown`.
pub(super) fn set_sonic_boom_cooldown(body: &WardenEntity, cooldown: i64) {
    body.brain
        .memories()
        .lock()
        .set_with_expiry(&SONIC_BOOM_COOLDOWN, (), cooldown);
}

/// Vanilla `SetRoarTarget.create(Warden::getEntityAngryAt)`.
fn set_roar_target() -> impl BehaviorControl<WardenEntity> {
    OneShot::new(
        vec![
            MemoryRequirement::new(&ROAR_TARGET, MemoryStatus::ValueAbsent),
            MemoryRequirement::new(&ATTACK_TARGET, MemoryStatus::ValueAbsent),
            MemoryRequirement::new(&CANT_REACH_WALK_TARGET_SINCE, MemoryStatus::Registered),
        ],
        |_world: &Arc<World>, body: &WardenEntity, _time: i64| {
            let Some(target) = body
                .entity_angry_at()
                .filter(|target| body.can_target_entity(target.as_ref()))
            else {
                return false;
            };
            let mut memories = body.brain.memories().lock();
            memories.set(&ROAR_TARGET, Some(target));
            memories.erase(&CANT_REACH_WALK_TARGET_SINCE);
            true
        },
    )
}

/// Vanilla `SetWardenLookTarget`: looks at the roar target, else the
/// disturbance.
fn set_warden_look_target() -> impl BehaviorControl<WardenEntity> {
    OneShot::new(
        vec![
            MemoryRequirement::new(&LOOK_TARGET, MemoryStatus::Registered),
            MemoryRequirement::new(&DISTURBANCE_LOCATION, MemoryStatus::Registered),
            MemoryRequirement::new(&ROAR_TARGET, MemoryStatus::Registered),
            MemoryRequirement::new(&ATTACK_TARGET, MemoryStatus::ValueAbsent),
        ],
        |_world: &Arc<World>, body: &WardenEntity, _time: i64| {
            let mut memories = body.brain.memories().lock();
            let target = memories
                .get(&ROAR_TARGET)
                .map(|roar_target| roar_target.block_position())
                .or_else(|| memories.get(&DISTURBANCE_LOCATION));
            let Some(target) = target else {
                return false;
            };
            memories.set(&LOOK_TARGET, Some(PositionTracker::block(target)));
            true
        },
    )
}

/// Vanilla `TryToSniff`: with someone near and nothing heard, sniff around.
fn try_to_sniff() -> impl BehaviorControl<WardenEntity> {
    OneShot::new(
        vec![
            MemoryRequirement::new(&IS_SNIFFING, MemoryStatus::Registered),
            MemoryRequirement::new(&WALK_TARGET, MemoryStatus::Registered),
            MemoryRequirement::new(&SNIFF_COOLDOWN, MemoryStatus::ValueAbsent),
            MemoryRequirement::new(&NEAREST_ATTACKABLE, MemoryStatus::ValuePresent),
            MemoryRequirement::new(&DISTURBANCE_LOCATION, MemoryStatus::ValueAbsent),
        ],
        |_world: &Arc<World>, body: &WardenEntity, _time: i64| {
            {
                let mut memories = body.brain.memories().lock();
                memories.set(&IS_SNIFFING, Some(()));
                memories.set_with_expiry(
                    &SNIFF_COOLDOWN,
                    (),
                    i64::from(TRY_TO_SNIFF_COOLDOWN.sample()),
                );
                memories.erase(&WALK_TARGET);
            }
            body.set_pose(EntityPose::Sniffing);
            true
        },
    )
}

/// Vanilla `closerThan(entity, horizontal, vertical)`.
pub(super) fn closer_than(body: &dyn Entity, target: &dyn Entity, xz: f64, y: f64) -> bool {
    let delta = target.position() - body.position();
    delta.x.mul_add(delta.x, delta.z * delta.z) < xz * xz && delta.y * delta.y < y * y
}

const NO_ENTRY: [MemoryRequirement; 0] = [];

/// Vanilla `ForceUnmount`: a warden about to dig leaves its vehicle.
struct ForceUnmount;

impl Behavior<WardenEntity> for ForceUnmount {
    fn entry_condition(&self) -> &[MemoryRequirement] {
        &NO_ENTRY
    }

    fn check_extra_start_conditions(&mut self, _world: &Arc<World>, body: &WardenEntity) -> bool {
        body.is_passenger()
    }

    fn start(&mut self, _world: &Arc<World>, body: &WardenEntity, _time: i64) {
        body.stop_riding();
    }
}

const DIGGING_ENTRY: [MemoryRequirement; 2] = [
    MemoryRequirement::new(&ATTACK_TARGET, MemoryStatus::ValueAbsent),
    MemoryRequirement::new(&WALK_TARGET, MemoryStatus::ValueAbsent),
];

/// Vanilla `Digging`: a bored warden digs back down and is gone.
struct Digging;

impl Digging {
    fn discard(body: &WardenEntity) {
        if body.removal_reason().is_none() {
            body.set_removed(RemovalReason::Discarded);
        }
    }
}

impl Behavior<WardenEntity> for Digging {
    fn entry_condition(&self) -> &[MemoryRequirement] {
        &DIGGING_ENTRY
    }

    fn duration(&self) -> (i32, i32) {
        (DIGGING_DURATION, DIGGING_DURATION)
    }

    fn check_extra_start_conditions(&mut self, _world: &Arc<World>, body: &WardenEntity) -> bool {
        body.on_ground() || body.is_in_water() || body.is_in_lava()
    }

    fn start(&mut self, _world: &Arc<World>, body: &WardenEntity, _time: i64) {
        if body.on_ground() {
            body.set_pose(EntityPose::Digging);
            body.play_sound(&sound_events::ENTITY_WARDEN_DIG, 5.0, 1.0);
        } else {
            body.play_sound(&sound_events::ENTITY_WARDEN_AGITATED, 5.0, 1.0);
            Self::discard(body);
        }
    }

    fn can_still_use(&mut self, _world: &Arc<World>, body: &WardenEntity, _time: i64) -> bool {
        body.removal_reason().is_none()
    }

    fn stop(&mut self, _world: &Arc<World>, body: &WardenEntity, _time: i64) {
        Self::discard(body);
    }
}

const EMERGING_ENTRY: [MemoryRequirement; 3] = [
    MemoryRequirement::new(&IS_EMERGING, MemoryStatus::ValuePresent),
    MemoryRequirement::new(&WALK_TARGET, MemoryStatus::ValueAbsent),
    MemoryRequirement::new(&LOOK_TARGET, MemoryStatus::Registered),
];

/// Vanilla `Emerging`: the summoned warden climbs out of the ground.
struct Emerging;

impl Behavior<WardenEntity> for Emerging {
    fn entry_condition(&self) -> &[MemoryRequirement] {
        &EMERGING_ENTRY
    }

    fn duration(&self) -> (i32, i32) {
        (EMERGE_DURATION, EMERGE_DURATION)
    }

    fn start(&mut self, _world: &Arc<World>, body: &WardenEntity, _time: i64) {
        body.set_pose(EntityPose::Emerging);
        body.play_sound(&sound_events::ENTITY_WARDEN_EMERGE, 5.0, 1.0);
    }

    fn can_still_use(&mut self, _world: &Arc<World>, _body: &WardenEntity, _time: i64) -> bool {
        true
    }

    fn stop(&mut self, _world: &Arc<World>, body: &WardenEntity, _time: i64) {
        if body.pose() == EntityPose::Emerging {
            body.set_pose(EntityPose::Standing);
        }
    }
}

const SNIFFING_ENTRY: [MemoryRequirement; 7] = [
    MemoryRequirement::new(&IS_SNIFFING, MemoryStatus::ValuePresent),
    MemoryRequirement::new(&ATTACK_TARGET, MemoryStatus::ValueAbsent),
    MemoryRequirement::new(&WALK_TARGET, MemoryStatus::ValueAbsent),
    MemoryRequirement::new(&LOOK_TARGET, MemoryStatus::Registered),
    MemoryRequirement::new(&NEAREST_ATTACKABLE, MemoryStatus::Registered),
    MemoryRequirement::new(&DISTURBANCE_LOCATION, MemoryStatus::Registered),
    MemoryRequirement::new(&SNIFF_COOLDOWN, MemoryStatus::Registered),
];

/// Vanilla `Sniffing`: smelling someone close angers the warden, smelling
/// anyone gives it somewhere to look.
struct Sniffing;

impl Behavior<WardenEntity> for Sniffing {
    fn entry_condition(&self) -> &[MemoryRequirement] {
        &SNIFFING_ENTRY
    }

    fn duration(&self) -> (i32, i32) {
        (SNIFFING_DURATION, SNIFFING_DURATION)
    }

    fn start(&mut self, _world: &Arc<World>, body: &WardenEntity, _time: i64) {
        body.play_sound(&sound_events::ENTITY_WARDEN_SNIFF, 5.0, 1.0);
    }

    fn can_still_use(&mut self, _world: &Arc<World>, _body: &WardenEntity, _time: i64) -> bool {
        true
    }

    fn stop(&mut self, _world: &Arc<World>, body: &WardenEntity, _time: i64) {
        if body.pose() == EntityPose::Sniffing {
            body.set_pose(EntityPose::Standing);
        }
        let nearest = {
            let mut memories = body.brain.memories().lock();
            memories.erase(&IS_SNIFFING);
            memories.get(&NEAREST_ATTACKABLE)
        };
        let Some(entity) = nearest.filter(|entity| body.can_target_entity(entity.as_ref())) else {
            return;
        };
        if closer_than(
            body,
            entity.as_ref(),
            ANGER_FROM_SNIFFING_MAX_DISTANCE_XZ,
            ANGER_FROM_SNIFFING_MAX_DISTANCE_Y,
        ) {
            body.increase_anger_at(Some(&entity));
        }
        if !body
            .brain
            .memories()
            .lock()
            .has_value(&DISTURBANCE_LOCATION)
        {
            set_disturbance_location(body, entity.block_position());
        }
    }
}

const ROAR_ENTRY: [MemoryRequirement; 4] = [
    MemoryRequirement::new(&ROAR_TARGET, MemoryStatus::ValuePresent),
    MemoryRequirement::new(&ATTACK_TARGET, MemoryStatus::ValueAbsent),
    MemoryRequirement::new(&ROAR_SOUND_COOLDOWN, MemoryStatus::Registered),
    MemoryRequirement::new(&ROAR_SOUND_DELAY, MemoryStatus::Registered),
];

/// Vanilla `Roar`: the warden roars at its target, then attacks it.
struct Roar;

impl Behavior<WardenEntity> for Roar {
    fn entry_condition(&self) -> &[MemoryRequirement] {
        &ROAR_ENTRY
    }

    fn duration(&self) -> (i32, i32) {
        (ROAR_DURATION, ROAR_DURATION)
    }

    fn start(&mut self, _world: &Arc<World>, body: &WardenEntity, _time: i64) {
        let target = {
            let mut memories = body.brain.memories().lock();
            memories.set_with_expiry(&ROAR_SOUND_DELAY, (), TICKS_BEFORE_PLAYING_ROAR_SOUND);
            memories.erase(&WALK_TARGET);
            memories.get(&ROAR_TARGET)
        };
        let Some(target) = target else {
            return;
        };
        look_at_entity(body, &target);
        body.set_pose(EntityPose::Roaring);
        body.increase_anger_at_by(Some(&target), ROAR_ANGER_INCREASE, false);
    }

    fn can_still_use(&mut self, _world: &Arc<World>, _body: &WardenEntity, _time: i64) -> bool {
        true
    }

    fn tick(&mut self, _world: &Arc<World>, body: &WardenEntity, _time: i64) {
        let play = {
            let mut memories = body.brain.memories().lock();
            let play =
                !memories.has_value(&ROAR_SOUND_DELAY) && !memories.has_value(&ROAR_SOUND_COOLDOWN);
            if play {
                memories.set_with_expiry(
                    &ROAR_SOUND_COOLDOWN,
                    (),
                    i64::from(ROAR_DURATION) - TICKS_BEFORE_PLAYING_ROAR_SOUND,
                );
            }
            play
        };
        if play {
            body.play_sound(&sound_events::ENTITY_WARDEN_ROAR, 3.0, 1.0);
        }
    }

    fn stop(&mut self, _world: &Arc<World>, body: &WardenEntity, _time: i64) {
        if body.pose() == EntityPose::Roaring {
            body.set_pose(EntityPose::Standing);
        }
        let roar_target = body.brain.memories().lock().get(&ROAR_TARGET);
        if let Some(target) = roar_target {
            body.set_attack_target(target);
        }
        body.brain.memories().lock().erase(&ROAR_TARGET);
    }
}

const SONIC_BOOM_ENTRY: [MemoryRequirement; 4] = [
    MemoryRequirement::new(&ATTACK_TARGET, MemoryStatus::ValuePresent),
    MemoryRequirement::new(&SONIC_BOOM_COOLDOWN, MemoryStatus::ValueAbsent),
    MemoryRequirement::new(&SONIC_BOOM_SOUND_COOLDOWN, MemoryStatus::Registered),
    MemoryRequirement::new(&SONIC_BOOM_SOUND_DELAY, MemoryStatus::Registered),
];

/// Vanilla `SonicBoom`: a ranged blast through walls and armor.
struct SonicBoom;

impl SonicBoom {
    /// The blast itself, `TICKS_BEFORE_PLAYING_SOUND` after the charge.
    fn blast(world: &Arc<World>, body: &WardenEntity, target: &SharedEntity) {
        let dimensions = body.base().dimensions();
        let chest = dimensions.attachments.get_clamped(
            EntityAttachment::WardenChest,
            0,
            body.rotation().0,
            dimensions,
        );
        let source = body.position() + chest;
        let target_eye = DVec3::new(target.position().x, target.get_eye_y(), target.position().z);
        let delta = target_eye - source;
        let normalized = delta.normalize_or_zero();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "vanilla Mth.floor of the blast length"
        )]
        let steps = delta.length().floor() as i32 + 7;
        for step in 1..steps {
            let particle = source + normalized * f64::from(step);
            world.send_particles(
                ParticleData::simple(&SONIC_BOOM_PARTICLE),
                particle,
                1,
                DVec3::ZERO,
                0.0,
            );
        }
        body.play_sound(&sound_events::ENTITY_WARDEN_SONIC_BOOM, 3.0, 1.0);
        let Some(living) = target.as_living_entity() else {
            return;
        };
        let Some(body_entity) = world.get_entity_by_id(body.id()) else {
            return;
        };
        let source = DamageSource::direct(&vanilla_damage_types::SONIC_BOOM, body_entity);
        if living.hurt_server(world, &source, SONIC_BOOM_DAMAGE) {
            let knockback_resistance = living
                .attributes()
                .lock()
                .required_value(vanilla_attributes::KNOCKBACK_RESISTANCE);
            let vertical = SONIC_BOOM_KNOCKBACK_VERTICAL * (1.0 - knockback_resistance);
            let horizontal = SONIC_BOOM_KNOCKBACK_HORIZONTAL * (1.0 - knockback_resistance);
            target.push_impulse(DVec3::new(
                normalized.x * horizontal,
                normalized.y * vertical,
                normalized.z * horizontal,
            ));
        }
    }
}

impl Behavior<WardenEntity> for SonicBoom {
    fn entry_condition(&self) -> &[MemoryRequirement] {
        &SONIC_BOOM_ENTRY
    }

    fn duration(&self) -> (i32, i32) {
        (SONIC_BOOM_DURATION, SONIC_BOOM_DURATION)
    }

    fn check_extra_start_conditions(&mut self, _world: &Arc<World>, body: &WardenEntity) -> bool {
        body.brain
            .memories()
            .lock()
            .get(&ATTACK_TARGET)
            .is_some_and(|target| {
                closer_than(
                    body,
                    target.as_ref(),
                    SONIC_BOOM_DISTANCE_XZ,
                    SONIC_BOOM_DISTANCE_Y,
                )
            })
    }

    fn can_still_use(&mut self, _world: &Arc<World>, _body: &WardenEntity, _time: i64) -> bool {
        true
    }

    fn start(&mut self, _world: &Arc<World>, body: &WardenEntity, _time: i64) {
        {
            let mut memories = body.brain.memories().lock();
            memories.set_with_expiry(&ATTACK_COOLING_DOWN, true, i64::from(SONIC_BOOM_DURATION));
            memories.set_with_expiry(
                &SONIC_BOOM_SOUND_DELAY,
                (),
                SONIC_BOOM_TICKS_BEFORE_PLAYING_SOUND,
            );
        }
        body.broadcast_entity_event(EntityStatus::SonicCharge);
        body.play_sound(&sound_events::ENTITY_WARDEN_SONIC_CHARGE, 3.0, 1.0);
    }

    fn tick(&mut self, world: &Arc<World>, body: &WardenEntity, _time: i64) {
        let attack_target = body.brain.memories().lock().get(&ATTACK_TARGET);
        if let Some(target) = &attack_target {
            let head_rot_speed = body.head_rot_speed();
            body.mob_base().controls().lock().look_control.set_look_at(
                target.position(),
                head_rot_speed,
                body.max_head_x_rot(),
            );
        }
        let blast = {
            let mut memories = body.brain.memories().lock();
            let blast = !memories.has_value(&SONIC_BOOM_SOUND_DELAY)
                && !memories.has_value(&SONIC_BOOM_SOUND_COOLDOWN);
            if blast {
                memories.set_with_expiry(
                    &SONIC_BOOM_SOUND_COOLDOWN,
                    (),
                    i64::from(SONIC_BOOM_DURATION) - SONIC_BOOM_TICKS_BEFORE_PLAYING_SOUND,
                );
            }
            blast
        };
        if !blast {
            return;
        }
        if let Some(target) = attack_target.filter(|target| {
            body.can_target_entity(target.as_ref())
                && closer_than(
                    body,
                    target.as_ref(),
                    SONIC_BOOM_DISTANCE_XZ,
                    SONIC_BOOM_DISTANCE_Y,
                )
        }) {
            Self::blast(world, body, &target);
        }
    }

    fn stop(&mut self, _world: &Arc<World>, body: &WardenEntity, _time: i64) {
        set_sonic_boom_cooldown(body, SONIC_BOOM_COOLDOWN_TICKS);
    }
}
