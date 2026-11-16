//! Piglin-only brain behaviors: admiring gold and hunting hoglins (vanilla
//! `StartAdmiringItemIfSeen`, `StopAdmiringIfItemTooFarAway`,
//! `StopAdmiringIfTiredOfTryingToReachItem`,
//! `StopHoldingItemIfNoLongerAdmiring`, `StartHuntingHoglin` and
//! `RememberIfHoglinWasKilled`).

use std::sync::Arc;

use steel_registry::data_components::vanilla_components::BLOCKS_ATTACKS;
use steel_registry::vanilla_entities;

use super::PiglinEntity;
use super::ai::stop_holding_off_hand_item;
use super::anger::{
    TIME_BETWEEN_HUNTS, as_abstract_piglin, broadcast_anger_target,
    dont_kill_any_more_hoglins_for_a_while, is_loved_item, set_anger_target,
};
use crate::entity::ai::brain::behavior::OneShot;
use crate::entity::ai::brain::memory::MemoryRequirement;
use crate::entity::ai::brain::memory_types::{
    ADMIRING_DISABLED, ADMIRING_ITEM, ANGRY_AT, ATTACK_TARGET, DISABLE_WALK_TO_ADMIRE_ITEM,
    HUNTED_RECENTLY, NEAREST_VISIBLE_ADULT_PIGLINS, NEAREST_VISIBLE_HUNTABLE_HOGLIN,
    NEAREST_VISIBLE_WANTED_ITEM, TIME_TRYING_TO_REACH_ADMIRE_ITEM,
};
use crate::entity::ai::brain::{BehaviorControl, MemoryStatus};
use crate::entity::entities::ItemEntity;
use crate::entity::{Entity, LivingEntity};
use crate::inventory::equipment::EquipmentSlot;
use crate::world::World;
use steel_utils::Downcast as _;

fn off_hand_is_empty(body: &PiglinEntity) -> bool {
    body.equipment_in_slot(EquipmentSlot::OffHand).is_empty()
}

/// Vanilla `StartAdmiringItemIfSeen.create`: starts admiring for
/// `admire_duration` ticks once a loved item is in sight.
pub(super) fn start_admiring_item_if_seen(
    admire_duration: i64,
) -> impl BehaviorControl<PiglinEntity> {
    OneShot::new(
        vec![
            MemoryRequirement::new(&NEAREST_VISIBLE_WANTED_ITEM, MemoryStatus::ValuePresent),
            MemoryRequirement::new(&ADMIRING_ITEM, MemoryStatus::ValueAbsent),
            MemoryRequirement::new(&ADMIRING_DISABLED, MemoryStatus::ValueAbsent),
            MemoryRequirement::new(&DISABLE_WALK_TO_ADMIRE_ITEM, MemoryStatus::ValueAbsent),
        ],
        move |_world: &Arc<World>, body: &PiglinEntity, _time: i64| {
            let mut memories = body.memories().lock();
            if memories.has_value(&ADMIRING_ITEM)
                || memories.has_value(&ADMIRING_DISABLED)
                || memories.has_value(&DISABLE_WALK_TO_ADMIRE_ITEM)
            {
                return false;
            }
            let loved = memories
                .get_ref(&NEAREST_VISIBLE_WANTED_ITEM)
                .and_then(|item| item.downcast_ref::<ItemEntity>().map(ItemEntity::get_item))
                .is_some_and(|item| is_loved_item(&item));
            if !loved {
                return false;
            }
            memories.set_with_expiry(&ADMIRING_ITEM, true, admire_duration);
            true
        },
    )
}

/// Vanilla `StopAdmiringIfItemTooFarAway.create`: with empty hands, stops
/// admiring once no wanted item is within `max_distance_to_item`.
pub(super) fn stop_admiring_if_item_too_far_away(
    max_distance_to_item: i32,
) -> impl BehaviorControl<PiglinEntity> {
    let max_distance_sqr = f64::from(max_distance_to_item * max_distance_to_item);
    OneShot::new(
        vec![
            MemoryRequirement::new(&ADMIRING_ITEM, MemoryStatus::ValuePresent),
            MemoryRequirement::new(&NEAREST_VISIBLE_WANTED_ITEM, MemoryStatus::Registered),
        ],
        move |_world: &Arc<World>, body: &PiglinEntity, _time: i64| {
            if !off_hand_is_empty(body) {
                return false;
            }
            let mut memories = body.memories().lock();
            if !memories.has_value(&ADMIRING_ITEM) {
                return false;
            }
            let item_close = memories
                .get_ref(&NEAREST_VISIBLE_WANTED_ITEM)
                .is_some_and(|item| {
                    item.position().distance_squared(body.position()) < max_distance_sqr
                });
            if item_close {
                return false;
            }
            memories.erase(&ADMIRING_ITEM);
            true
        },
    )
}

/// Vanilla `StopAdmiringIfTiredOfTryingToReachItem.create`: with empty hands,
/// gives up admiring after `max_time_to_reach_item` ticks of walking and
/// stops walking to admired items for `disable_time` ticks.
pub(super) fn stop_admiring_if_tired_of_trying_to_reach_item(
    max_time_to_reach_item: i32,
    disable_time: i64,
) -> impl BehaviorControl<PiglinEntity> {
    OneShot::new(
        vec![
            MemoryRequirement::new(&ADMIRING_ITEM, MemoryStatus::ValuePresent),
            MemoryRequirement::new(&NEAREST_VISIBLE_WANTED_ITEM, MemoryStatus::ValuePresent),
            MemoryRequirement::new(&TIME_TRYING_TO_REACH_ADMIRE_ITEM, MemoryStatus::Registered),
            MemoryRequirement::new(&DISABLE_WALK_TO_ADMIRE_ITEM, MemoryStatus::Registered),
        ],
        move |_world: &Arc<World>, body: &PiglinEntity, _time: i64| {
            if !off_hand_is_empty(body) {
                return false;
            }
            let mut memories = body.memories().lock();
            if !memories.has_value(&ADMIRING_ITEM)
                || !memories.has_value(&NEAREST_VISIBLE_WANTED_ITEM)
            {
                return false;
            }
            match memories.get(&TIME_TRYING_TO_REACH_ADMIRE_ITEM) {
                None => memories.set(&TIME_TRYING_TO_REACH_ADMIRE_ITEM, Some(0)),
                Some(time) if time > max_time_to_reach_item => {
                    memories.erase(&ADMIRING_ITEM);
                    memories.erase(&TIME_TRYING_TO_REACH_ADMIRE_ITEM);
                    memories.set_with_expiry(&DISABLE_WALK_TO_ADMIRE_ITEM, true, disable_time);
                }
                Some(time) => memories.set(&TIME_TRYING_TO_REACH_ADMIRE_ITEM, Some(time + 1)),
            }
            true
        },
    )
}

/// Vanilla `StopHoldingItemIfNoLongerAdmiring.create`: once admiring ends,
/// barters or keeps whatever is in the off hand, unless it is a shield.
pub(super) fn stop_holding_item_if_no_longer_admiring() -> impl BehaviorControl<PiglinEntity> {
    OneShot::new(
        vec![MemoryRequirement::new(
            &ADMIRING_ITEM,
            MemoryStatus::ValueAbsent,
        )],
        |world: &Arc<World>, body: &PiglinEntity, _time: i64| {
            if body.memories().lock().has_value(&ADMIRING_ITEM) {
                return false;
            }
            let off_hand = body.equipment_in_slot(EquipmentSlot::OffHand);
            if off_hand.is_empty() || off_hand.has(BLOCKS_ATTACKS) {
                return false;
            }
            stop_holding_off_hand_item(world, body, true);
            true
        },
    )
}

/// Vanilla `StartHuntingHoglin.create`: an adult piglin picks a fight with a
/// huntable hoglin unless it or a piglin in sight hunted recently, and the
/// whole group then waits before hunting again.
pub(super) fn start_hunting_hoglin() -> impl BehaviorControl<PiglinEntity> {
    OneShot::new(
        vec![
            MemoryRequirement::new(&NEAREST_VISIBLE_HUNTABLE_HOGLIN, MemoryStatus::ValuePresent),
            MemoryRequirement::new(&ANGRY_AT, MemoryStatus::ValueAbsent),
            MemoryRequirement::new(&HUNTED_RECENTLY, MemoryStatus::ValueAbsent),
            MemoryRequirement::new(&NEAREST_VISIBLE_ADULT_PIGLINS, MemoryStatus::Registered),
        ],
        |world: &Arc<World>, body: &PiglinEntity, _time: i64| {
            let (target, piglins) = {
                let memories = body.memories().lock();
                if memories.has_value(&ANGRY_AT) || memories.has_value(&HUNTED_RECENTLY) {
                    return false;
                }
                let Some(target) = memories.get(&NEAREST_VISIBLE_HUNTABLE_HOGLIN) else {
                    return false;
                };
                (
                    target,
                    memories
                        .get(&NEAREST_VISIBLE_ADULT_PIGLINS)
                        .unwrap_or_default(),
                )
            };
            let someone_hunted_recently = piglins.iter().any(|piglin| {
                piglin
                    .as_living_entity()
                    .and_then(LivingEntity::brain_memories)
                    .is_some_and(|memories| memories.lock().has_value(&HUNTED_RECENTLY))
            });
            if body.is_baby() || someone_hunted_recently {
                return false;
            }
            set_anger_target(world, body, &target);
            dont_kill_any_more_hoglins_for_a_while(body);
            broadcast_anger_target(world, body, &target);
            for piglin in &piglins {
                if let Some(piglin) = as_abstract_piglin(piglin) {
                    dont_kill_any_more_hoglins_for_a_while(piglin);
                }
            }
            true
        },
    )
}

/// Vanilla `RememberIfHoglinWasKilled.create`: killing a hoglin starts the
/// wait before the next hunt.
pub(super) fn remember_if_hoglin_was_killed() -> impl BehaviorControl<PiglinEntity> {
    OneShot::new(
        vec![
            MemoryRequirement::new(&ATTACK_TARGET, MemoryStatus::ValuePresent),
            MemoryRequirement::new(&HUNTED_RECENTLY, MemoryStatus::Registered),
        ],
        |_world: &Arc<World>, body: &PiglinEntity, _time: i64| {
            let mut memories = body.memories().lock();
            let Some(target) = memories.get(&ATTACK_TARGET) else {
                return false;
            };
            if target.entity_type() == &vanilla_entities::HOGLIN
                && target
                    .as_living_entity()
                    .is_some_and(LivingEntity::is_dead_or_dying)
            {
                memories.set_with_expiry(
                    &HUNTED_RECENTLY,
                    true,
                    i64::from(TIME_BETWEEN_HUNTS.sample()),
                );
            }
            true
        },
    )
}
