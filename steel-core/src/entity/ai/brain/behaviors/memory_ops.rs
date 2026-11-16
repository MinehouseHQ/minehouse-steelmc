//! Brain behaviors that only touch memories (vanilla
//! `BecomePassiveIfMemoryPresent`, `CopyMemoryWithExpiry` and `EraseMemoryIf`).

use std::sync::Arc;

use crate::entity::LivingEntity;
use crate::entity::ai::brain::behavior::OneShot;
use crate::entity::ai::brain::behaviors::UniformInt;
use crate::entity::ai::brain::memory::{MemoryModuleType, MemoryRequirement, MemoryValue};
use crate::entity::ai::brain::memory_types::{ATTACK_TARGET, PACIFIED};
use crate::entity::ai::brain::{AnyMemoryModuleType, BehaviorControl, MemoryStatus};
use crate::world::World;

/// Vanilla `BecomePassiveIfMemoryPresent`: while `pacifying_memory` holds a
/// value, drop the attack target and stay pacified for `pacify_duration`
/// ticks.
pub fn become_passive_if_memory_present<E: LivingEntity + ?Sized>(
    pacifying_memory: &'static dyn AnyMemoryModuleType,
    pacify_duration: i64,
) -> impl BehaviorControl<E> {
    OneShot::new(
        vec![
            MemoryRequirement::new(&ATTACK_TARGET, MemoryStatus::Registered),
            MemoryRequirement::new(&PACIFIED, MemoryStatus::Registered),
            MemoryRequirement::new(pacifying_memory, MemoryStatus::Registered),
        ],
        move |_world: &Arc<World>, body: &E, _time: i64| {
            let Some(memories) = body.brain_memories() else {
                return false;
            };
            let mut memories = memories.lock();
            if memories.has_value(&PACIFIED) || !memories.has_value(pacifying_memory) {
                return false;
            }
            memories.set_with_expiry(&PACIFIED, true, pacify_duration);
            memories.erase(&ATTACK_TARGET);
            true
        },
    )
}

/// Vanilla `CopyMemoryWithExpiry`: while `source` holds a value and `target`
/// is empty, copies it into `target` for a duration drawn from
/// `duration_of_copy`, as long as `copy_if_true` holds for the body.
pub fn copy_memory_with_expiry<E: LivingEntity + ?Sized, T: MemoryValue>(
    copy_if_true: fn(&E) -> bool,
    source: &'static MemoryModuleType<T>,
    target: &'static MemoryModuleType<T>,
    duration_of_copy: UniformInt,
) -> impl BehaviorControl<E> {
    OneShot::new(
        vec![
            MemoryRequirement::new(source, MemoryStatus::ValuePresent),
            MemoryRequirement::new(target, MemoryStatus::ValueAbsent),
        ],
        move |_world: &Arc<World>, body: &E, _time: i64| {
            let Some(memories) = body.brain_memories() else {
                return false;
            };
            let value = {
                let memories = memories.lock();
                if memories.has_value(target) {
                    return false;
                }
                let Some(value) = memories.get(source) else {
                    return false;
                };
                value
            };
            if !copy_if_true(body) {
                return false;
            }
            memories
                .lock()
                .set_with_expiry(target, value, i64::from(duration_of_copy.sample()));
            true
        },
    )
}
