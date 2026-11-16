//! Vanilla `Mount` and `DismountOrSkipMounting`.

use std::sync::Arc;

use crate::entity::ai::brain::behavior::OneShot;
use crate::entity::ai::brain::memory::MemoryRequirement;
use crate::entity::ai::brain::memory_types::{
    LOOK_TARGET, PositionTracker, RIDE_TARGET, WALK_TARGET, WalkTarget,
};
use crate::entity::ai::brain::{BehaviorControl, MemoryStatus};
use crate::entity::{Entity, LivingEntity, SharedEntity};
use crate::world::World;

/// Vanilla `Mount.CLOSE_ENOUGH_TO_START_RIDING_DIST`.
const CLOSE_ENOUGH_TO_START_RIDING_DIST: f64 = 1.0;

/// Vanilla `Entity.closerThan(other, distance)`.
fn closer_than(first: &dyn Entity, second: &(impl Entity + ?Sized), distance: f64) -> bool {
    first.position().distance_squared(second.position()) < distance * distance
}

/// Vanilla `Mount.create`: climbs onto `RIDE_TARGET` once within a block of
/// it, otherwise walks there at `speed_modifier`.
pub fn mount<E: LivingEntity + ?Sized>(speed_modifier: f32) -> impl BehaviorControl<E> {
    OneShot::new(
        vec![
            MemoryRequirement::new(&LOOK_TARGET, MemoryStatus::Registered),
            MemoryRequirement::new(&WALK_TARGET, MemoryStatus::ValueAbsent),
            MemoryRequirement::new(&RIDE_TARGET, MemoryStatus::ValuePresent),
        ],
        move |world: &Arc<World>, body: &E, _time: i64| {
            let Some(memories) = body.brain_memories() else {
                return false;
            };
            let ride_target = {
                let memories = memories.lock();
                if memories.has_value(&WALK_TARGET) {
                    return false;
                }
                let Some(ride_target) = memories.get(&RIDE_TARGET) else {
                    return false;
                };
                ride_target
            };
            if body.is_passenger() {
                return false;
            }
            if closer_than(
                ride_target.as_ref(),
                body,
                CLOSE_ENOUGH_TO_START_RIDING_DIST,
            ) {
                if let Some(entity) = world.get_entity_by_id(body.id()) {
                    entity.start_riding(&ride_target, false);
                }
            } else {
                let mut memories = memories.lock();
                memories.set(
                    &LOOK_TARGET,
                    Some(PositionTracker::entity(Arc::clone(&ride_target), true)),
                );
                memories.set(
                    &WALK_TARGET,
                    Some(WalkTarget::new(
                        PositionTracker::entity(ride_target, false),
                        speed_modifier,
                        1,
                    )),
                );
            }
            true
        },
    )
}

/// Vanilla `DismountOrSkipMounting.isVehicleValid`.
fn is_vehicle_valid(
    body: &(impl LivingEntity + ?Sized),
    vehicle: &SharedEntity,
    max_walk_dist_to_ride_target: i32,
) -> bool {
    vehicle.is_alive()
        && closer_than(
            vehicle.as_ref(),
            body,
            f64::from(max_walk_dist_to_ride_target),
        )
        && match (vehicle.level(), body.level()) {
            (Some(vehicle_level), Some(body_level)) => Arc::ptr_eq(&vehicle_level, &body_level),
            _ => false,
        }
}

/// Vanilla `DismountOrSkipMounting.create`: gets off (or forgets) the
/// current or planned vehicle once it is dead, too far away, in another
/// level, or `dont_ride_if` holds for it.
pub fn dismount_or_skip_mounting<E: LivingEntity + ?Sized>(
    max_walk_dist_to_ride_target: i32,
    dont_ride_if: fn(&E, &SharedEntity) -> bool,
) -> impl BehaviorControl<E> {
    OneShot::new(
        vec![MemoryRequirement::new(
            &RIDE_TARGET,
            MemoryStatus::Registered,
        )],
        move |_world: &Arc<World>, body: &E, _time: i64| {
            let Some(memories) = body.brain_memories() else {
                return false;
            };
            let target_vehicle = memories.lock().get(&RIDE_TARGET);
            let Some(vehicle) = body.vehicle().or(target_vehicle) else {
                return false;
            };
            if is_vehicle_valid(body, &vehicle, max_walk_dist_to_ride_target)
                && !dont_ride_if(body, &vehicle)
            {
                return false;
            }
            body.stop_riding();
            memories.lock().erase(&RIDE_TARGET);
            true
        },
    )
}
