//! The anger and hunting half of vanilla `PiglinAi` that piglins and piglin
//! brutes share.

use std::sync::Arc;

use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_game_rules::UNIVERSAL_ANGER as UNIVERSAL_ANGER_RULE;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_utils::Downcast as _;

use super::AbstractPiglin;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behaviors::utils::entity_is_visible;
use crate::entity::ai::brain::behaviors::{
    UniformInt, get_nearest_target, is_other_target_much_further_away_than_current_attack_target,
};
use crate::entity::ai::brain::memory_types::{
    ANGRY_AT, CANT_REACH_WALK_TARGET_SINCE, HUNTED_RECENTLY, NEARBY_ADULT_PIGLINS,
    NEAREST_LIVING_ENTITIES, NEAREST_VISIBLE_ATTACKABLE_PLAYER, UNIVERSAL_ANGER,
};
use crate::entity::ai::brain::nearest_visible::is_entity_attackable_ignoring_line_of_sight;
use crate::entity::entities::HoglinEntity;
use crate::entity::{Entity, LivingEntity, SharedEntity};
use crate::inventory::equipment::EquipmentSlot;
use crate::player::Player;
use crate::world::World;

/// Vanilla `PiglinAi.ANGER_DURATION`.
const ANGER_DURATION: i64 = 600;
/// Vanilla `PiglinAi.PLAYER_ANGER_RANGE`.
const PLAYER_ANGER_RANGE: f64 = 16.0;
/// Vanilla `PiglinAi.TIME_BETWEEN_HUNTS` (30 to 120 seconds).
pub(super) const TIME_BETWEEN_HUNTS: UniformInt = UniformInt::of(600, 2400);

/// The piglin or piglin brute behind `entity`, if it is one.
pub(super) fn as_abstract_piglin(entity: &SharedEntity) -> Option<&dyn AbstractPiglin> {
    entity.as_abstract_piglin()
}

/// Vanilla `PiglinAi.isLovedItem`.
pub(super) fn is_loved_item(item_stack: &ItemStack) -> bool {
    item_stack.item().has_tag(&ItemTag::PIGLIN_LOVED)
}

/// Vanilla `PiglinAi.isZombified`.
pub(super) fn is_zombified(entity: &dyn Entity) -> bool {
    let entity_type = entity.entity_type();
    entity_type == &vanilla_entities::ZOMBIFIED_PIGLIN || entity_type == &vanilla_entities::ZOGLIN
}

/// Vanilla `PiglinAi.isWearingSafeArmor`: any piece of gold armor.
pub(super) fn is_wearing_safe_armor(living: &dyn LivingEntity) -> bool {
    EquipmentSlot::ALL
        .iter()
        .filter(|slot| slot.is_armor())
        .any(|slot| {
            living
                .equipment_in_slot(*slot)
                .item()
                .has_tag(&ItemTag::PIGLIN_SAFE_ARMOR)
        })
}

/// Vanilla `PiglinAi.isPlayerHoldingLovedItem`.
pub(super) fn is_player_holding_loved_item(entity: &dyn LivingEntity) -> bool {
    entity.entity_type() == &vanilla_entities::PLAYER && entity.is_holding(&mut is_loved_item)
}

/// Vanilla `PiglinAi.findNearbyAdultPiglins`: the adult piglins and brutes
/// among the body's nearest living entities.
pub(super) fn find_nearby_adult_piglins(body: &dyn LivingEntity) -> Vec<SharedEntity> {
    let Some(memories) = body.brain_memories() else {
        return Vec::new();
    };
    let living = memories
        .lock()
        .get(&NEAREST_LIVING_ENTITIES)
        .unwrap_or_default();
    living
        .into_iter()
        .filter(|entity| as_abstract_piglin(entity).is_some_and(AbstractPiglin::is_adult))
        .collect()
}

/// Vanilla `PiglinAi.isIdle`.
pub(super) fn is_idle(body: &dyn AbstractPiglin) -> bool {
    body.is_activity_active(Activity::IDLE)
}

/// Vanilla `BehaviorUtils.getLivingEntityFromUUIDMemory(body, ANGRY_AT)`.
pub(super) fn anger_target(world: &World, body: &dyn LivingEntity) -> Option<SharedEntity> {
    let uuid = body.brain_memories()?.lock().get(&ANGRY_AT)?;
    world
        .get_entity_by_uuid(&uuid)
        .filter(|entity| entity.is_living_entity())
}

/// Vanilla `PiglinAi.getNearestVisibleTargetablePlayer`.
pub(super) fn nearest_visible_targetable_player(body: &dyn LivingEntity) -> Option<SharedEntity> {
    body.brain_memories()?
        .lock()
        .get(&NEAREST_VISIBLE_ATTACKABLE_PLAYER)
}

/// Vanilla `PiglinAi.getAdultPiglins`.
fn adult_piglins(body: &dyn LivingEntity) -> Vec<SharedEntity> {
    body.brain_memories()
        .and_then(|memories| memories.lock().get(&NEARBY_ADULT_PIGLINS))
        .unwrap_or_default()
}

fn attackable_ignoring_line_of_sight(
    world: &World,
    body: &dyn AbstractPiglin,
    target: &SharedEntity,
) -> bool {
    target
        .as_living_entity()
        .is_some_and(|living| is_entity_attackable_ignoring_line_of_sight(world, body, living))
}

/// Vanilla `PiglinAi.dontKillAnyMoreHoglinsForAWhile`.
pub(super) fn dont_kill_any_more_hoglins_for_a_while(body: &dyn LivingEntity) {
    if let Some(memories) = body.brain_memories() {
        memories.lock().set_with_expiry(
            &HUNTED_RECENTLY,
            true,
            i64::from(TIME_BETWEEN_HUNTS.sample()),
        );
    }
}

/// Vanilla `PiglinAi.setAngerTarget`.
pub(super) fn set_anger_target(world: &World, body: &dyn AbstractPiglin, target: &SharedEntity) {
    if !attackable_ignoring_line_of_sight(world, body, target) {
        return;
    }
    if let Some(memories) = body.brain_memories() {
        let mut memories = memories.lock();
        memories.erase(&CANT_REACH_WALK_TARGET_SINCE);
        memories.set_with_expiry(&ANGRY_AT, target.uuid(), ANGER_DURATION);
    }
    if target.entity_type() == &vanilla_entities::HOGLIN && body.can_hunt() {
        dont_kill_any_more_hoglins_for_a_while(body);
    }
    if target.entity_type() == &vanilla_entities::PLAYER
        && world.get_game_rule(&UNIVERSAL_ANGER_RULE)
        && let Some(memories) = body.brain_memories()
    {
        memories
            .lock()
            .set_with_expiry(&UNIVERSAL_ANGER, true, ANGER_DURATION);
    }
}

/// Vanilla `PiglinAi.setAngerTargetToNearestTargetablePlayerIfFound`.
fn set_anger_target_to_nearest_targetable_player_if_found(
    world: &World,
    body: &dyn AbstractPiglin,
    target_if_no_player_found: &SharedEntity,
) {
    let target = nearest_visible_targetable_player(body)
        .unwrap_or_else(|| Arc::clone(target_if_no_player_found));
    set_anger_target(world, body, &target);
}

/// Vanilla `PiglinAi.setAngerTargetIfCloserThanCurrent`.
fn set_anger_target_if_closer_than_current(
    world: &World,
    body: &dyn AbstractPiglin,
    new_target: &SharedEntity,
) {
    let current = anger_target(world, body);
    let nearest = get_nearest_target(body, current.clone(), Arc::clone(new_target));
    if current.is_none_or(|current| current.id() != nearest.id()) {
        set_anger_target(world, body, &nearest);
    }
}

/// Vanilla `PiglinAi.broadcastAngerTarget`: the nearby adult piglins join in,
/// except against a hoglin that one of them may not hunt.
pub(super) fn broadcast_anger_target(
    world: &World,
    body: &dyn AbstractPiglin,
    target: &SharedEntity,
) {
    for piglin in adult_piglins(body) {
        let Some(piglin) = as_abstract_piglin(&piglin) else {
            continue;
        };
        let protected_hoglin = target
            .downcast_ref::<HoglinEntity>()
            .is_some_and(|hoglin| !piglin.can_hunt() || !hoglin.can_be_hunted());
        if !protected_hoglin {
            set_anger_target_if_closer_than_current(world, piglin, target);
        }
    }
}

/// Vanilla `PiglinAi.broadcastUniversalAnger`.
pub(super) fn broadcast_universal_anger(world: &World, body: &dyn AbstractPiglin) {
    for piglin in adult_piglins(body) {
        let Some(piglin) = as_abstract_piglin(&piglin) else {
            continue;
        };
        if let Some(player) = nearest_visible_targetable_player(piglin) {
            set_anger_target(world, piglin, &player);
        }
    }
}

/// Vanilla `PiglinAi.maybeRetaliate`.
pub(super) fn maybe_retaliate(world: &World, body: &dyn AbstractPiglin, attacker: &SharedEntity) {
    if body.is_activity_active(Activity::AVOID)
        || !attackable_ignoring_line_of_sight(world, body, attacker)
        || is_other_target_much_further_away_than_current_attack_target(
            body,
            attacker.as_ref(),
            4.0,
        )
    {
        return;
    }
    if attacker.entity_type() == &vanilla_entities::PLAYER
        && world.get_game_rule(&UNIVERSAL_ANGER_RULE)
    {
        set_anger_target_to_nearest_targetable_player_if_found(world, body, attacker);
        broadcast_universal_anger(world, body);
    } else {
        set_anger_target(world, body, attacker);
        broadcast_anger_target(world, body, attacker);
    }
}

/// Vanilla `PiglinAi.angerNearbyPiglins`: idle piglins within 16 blocks of
/// `player` (only those that see the player when `only_if_they_see_the_player`)
/// get angry at them, or at the nearest player under universal anger.
pub fn anger_nearby_piglins(world: &World, player: &Player, only_if_they_see_the_player: bool) {
    let area = player.bounding_box().inflate(PLAYER_ANGER_RANGE);
    let Some(player_entity) = world.get_entity_by_id(player.id()) else {
        return;
    };
    let universal_anger = world.get_game_rule(&UNIVERSAL_ANGER_RULE);
    for entity in world.get_entities_in_aabb(&area) {
        let Some(piglin) = entity.downcast_ref::<super::PiglinEntity>() else {
            continue;
        };
        if !is_idle(piglin)
            || only_if_they_see_the_player && !entity_is_visible(piglin, &player_entity)
        {
            continue;
        }
        if universal_anger {
            set_anger_target_to_nearest_targetable_player_if_found(world, piglin, &player_entity);
        } else {
            set_anger_target(world, piglin, &player_entity);
        }
    }
}
