//! Vanilla `StartCelebratingIfTargetDead` and `StopBeingAngryIfTargetDead`.

use std::sync::Arc;

use steel_registry::vanilla_game_rules::FORGIVE_DEAD_PLAYERS;

use crate::entity::ai::brain::behavior::OneShot;
use crate::entity::ai::brain::memory::MemoryRequirement;
use crate::entity::ai::brain::memory_types::{
    ANGRY_AT, ATTACK_TARGET, CELEBRATE_LOCATION, DANCING,
};
use crate::entity::ai::brain::{BehaviorControl, MemoryStatus};
use crate::entity::{LivingEntity, SharedEntity};
use crate::world::World;

/// Whether a dead target lets go of the grudge: anything but a player, or a
/// player while `forgiveDeadPlayers` is on.
fn forgives(world: &World, target: &SharedEntity) -> bool {
    target.as_player().is_none() || world.get_game_rule(&FORGIVE_DEAD_PLAYERS)
}

/// Vanilla `StartCelebratingIfTargetDead.create`: once the attack target
/// dies, remember where to celebrate for `celebrate_duration` ticks (and
/// dance when `dance_predicate` holds), then forget the target unless it was
/// a player that is not forgiven.
pub fn start_celebrating_if_target_dead<E: LivingEntity + ?Sized>(
    celebrate_duration: i64,
    dance_predicate: fn(&Arc<World>, &E, &SharedEntity) -> bool,
) -> impl BehaviorControl<E> {
    OneShot::new(
        vec![
            MemoryRequirement::new(&ATTACK_TARGET, MemoryStatus::ValuePresent),
            MemoryRequirement::new(&ANGRY_AT, MemoryStatus::Registered),
            MemoryRequirement::new(&CELEBRATE_LOCATION, MemoryStatus::ValueAbsent),
            MemoryRequirement::new(&DANCING, MemoryStatus::Registered),
        ],
        move |world: &Arc<World>, body: &E, _time: i64| {
            let Some(memories) = body.brain_memories() else {
                return false;
            };
            let target = {
                let memories = memories.lock();
                if memories.has_value(&CELEBRATE_LOCATION) {
                    return false;
                }
                let Some(target) = memories.get(&ATTACK_TARGET) else {
                    return false;
                };
                target
            };
            if !target
                .as_living_entity()
                .is_some_and(LivingEntity::is_dead_or_dying)
            {
                return false;
            }
            let dance = dance_predicate(world, body, &target);
            let mut memories = memories.lock();
            if dance {
                memories.set_with_expiry(&DANCING, true, celebrate_duration);
            }
            memories.set_with_expiry(
                &CELEBRATE_LOCATION,
                target.block_position(),
                celebrate_duration,
            );
            if forgives(world, &target) {
                memories.erase(&ATTACK_TARGET);
                memories.erase(&ANGRY_AT);
            }
            true
        },
    )
}

/// Vanilla `StopBeingAngryIfTargetDead.create`: drops `ANGRY_AT` once the
/// entity it names is dead, unless it is a player that is not forgiven.
pub fn stop_being_angry_if_target_dead<E: LivingEntity + ?Sized>() -> impl BehaviorControl<E> {
    OneShot::new(
        vec![MemoryRequirement::new(
            &ANGRY_AT,
            MemoryStatus::ValuePresent,
        )],
        |world: &Arc<World>, body: &E, _time: i64| {
            let Some(memories) = body.brain_memories() else {
                return false;
            };
            let Some(angry_at) = memories.lock().get(&ANGRY_AT) else {
                return false;
            };
            let forgiven = world.get_entity_by_uuid(&angry_at).is_some_and(|target| {
                target
                    .as_living_entity()
                    .is_some_and(LivingEntity::is_dead_or_dying)
                    && forgives(world, &target)
            });
            if forgiven {
                memories.lock().erase(&ANGRY_AT);
            }
            true
        },
    )
}
