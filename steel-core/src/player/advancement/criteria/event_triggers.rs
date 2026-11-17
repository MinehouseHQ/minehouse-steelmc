//! Triggers fired by things happening to or around the player (vanilla
//! `CriteriaTriggers` `PLAYER_SHEARED_EQUIPMENT`, `THROWN_ITEM_PICKED_UP_BY_ENTITY`,
//! `THROWN_ITEM_PICKED_UP_BY_PLAYER`, `KILL_MOB_NEAR_SCULK_CATALYST`,
//! `AVOID_VIBRATION`, `CURED_ZOMBIE_VILLAGER`, `HONEY_BLOCK_SLIDE`,
//! `LIGHTNING_STRIKE`, `CHANNELED_LIGHTNING` and `USED_ENDER_EYE`).

use steel_registry::item_stack::ItemStack;
use steel_utils::{BlockPos, BlockStateId, Identifier};

use super::predicates::{ConditionContext, block_matches};
use super::triggers::test_context;
use super::{DistanceField, trigger};
use crate::entity::entities::ItemEntity;
use crate::entity::{Entity, SharedEntity};
use crate::player::Player;

/// Vanilla `CriteriaTriggers.PLAYER_SHEARED_EQUIPMENT`.
pub(crate) const PLAYER_SHEARED_EQUIPMENT: Identifier =
    Identifier::vanilla_static("player_sheared_equipment");
/// Vanilla `CriteriaTriggers.THROWN_ITEM_PICKED_UP_BY_ENTITY`.
pub(crate) const THROWN_ITEM_PICKED_UP_BY_ENTITY: Identifier =
    Identifier::vanilla_static("thrown_item_picked_up_by_entity");
/// Vanilla `CriteriaTriggers.THROWN_ITEM_PICKED_UP_BY_PLAYER`.
pub(crate) const THROWN_ITEM_PICKED_UP_BY_PLAYER: Identifier =
    Identifier::vanilla_static("thrown_item_picked_up_by_player");
/// Vanilla `CriteriaTriggers.KILL_MOB_NEAR_SCULK_CATALYST`.
pub(crate) const KILL_MOB_NEAR_SCULK_CATALYST: Identifier =
    Identifier::vanilla_static("kill_mob_near_sculk_catalyst");
/// Vanilla `CriteriaTriggers.AVOID_VIBRATION`.
pub(crate) const AVOID_VIBRATION: Identifier = Identifier::vanilla_static("avoid_vibration");
/// Vanilla `CriteriaTriggers.CURED_ZOMBIE_VILLAGER`.
pub(crate) const CURED_ZOMBIE_VILLAGER: Identifier =
    Identifier::vanilla_static("cured_zombie_villager");
/// Vanilla `CriteriaTriggers.HONEY_BLOCK_SLIDE`.
pub(crate) const HONEY_BLOCK_SLIDE: Identifier = Identifier::vanilla_static("slide_down_block");

/// Vanilla `CriteriaTriggers.LIGHTNING_STRIKE`.
pub(crate) const LIGHTNING_STRIKE: Identifier = Identifier::vanilla_static("lightning_strike");
/// Vanilla `CriteriaTriggers.CHANNELED_LIGHTNING`.
pub(crate) const CHANNELED_LIGHTNING: Identifier =
    Identifier::vanilla_static("channeled_lightning");

/// Vanilla `CriteriaTriggers.USED_ENDER_EYE`.
pub(crate) const USED_ENDER_EYE: Identifier = Identifier::vanilla_static("used_ender_eye");

/// Vanilla `LivingEntity.onItemPickup` (and `ServerPlayer.onItemPickup`):
/// `picker` takes `stack` from `item_entity`. A player thrower hears
/// `THROWN_ITEM_PICKED_UP_BY_ENTITY`; a player picker hears
/// `THROWN_ITEM_PICKED_UP_BY_PLAYER` about whoever threw it.
pub(crate) fn on_item_pickup(picker: &dyn Entity, item_entity: &ItemEntity, stack: &ItemStack) {
    let Some(world) = picker.level() else {
        return;
    };
    let thrower = item_entity
        .get_thrower()
        .and_then(|uuid| world.get_entity_by_uuid(&uuid));
    if let Some(thrower) = &thrower
        && let Some(player) = thrower.as_player()
    {
        player.trigger_item_and_entity(&THROWN_ITEM_PICKED_UP_BY_ENTITY, stack, picker);
    }
    if let Some(player) = picker.as_player()
        && let Some(thrower) = &thrower
    {
        player.trigger_item_and_entity(&THROWN_ITEM_PICKED_UP_BY_PLAYER, stack, thrower.as_ref());
    }
}

impl Player {
    /// Vanilla `CriteriaTriggers.PLAYER_SHEARED_EQUIPMENT.trigger(player,
    /// sheared, entity)`, a `PlayerInteractTrigger`.
    pub(crate) fn trigger_sheared_equipment(&self, sheared: &ItemStack, entity: &dyn Entity) {
        self.trigger_item_and_entity(&PLAYER_SHEARED_EQUIPMENT, sheared, entity);
    }

    /// Vanilla `CuredZombieVillagerTrigger.trigger(player, zombie, villager)`.
    pub(crate) fn trigger_cured_zombie_villager(&self, zombie: &dyn Entity, villager: &dyn Entity) {
        let world = self.get_world();
        let position = self.position();
        let zombie = ConditionContext::entity(&world, position, zombie);
        let villager = ConditionContext::entity(&world, position, villager);
        trigger(self, &CURED_ZOMBIE_VILLAGER, |conditions| {
            test_context(conditions.zombie.as_ref(), &zombie)
                && test_context(conditions.villager.as_ref(), &villager)
        });
    }

    /// Vanilla `SlideDownBlockTrigger.trigger(player, state)`.
    pub(crate) fn trigger_slide_down_block(&self, state: BlockStateId) {
        trigger(self, &HONEY_BLOCK_SLIDE, |conditions| {
            block_matches(conditions.blocks.as_ref(), conditions.state.as_ref(), state)
        });
    }

    /// Vanilla `LightningStrikeTrigger.trigger(player, lightning, entitiesAround)`.
    pub(crate) fn trigger_lightning_strike(
        &self,
        lightning: &dyn Entity,
        bystanders: &[SharedEntity],
    ) {
        let world = self.get_world();
        let position = self.position();
        let lightning = ConditionContext::entity(&world, position, lightning);
        let bystanders: Vec<_> = bystanders
            .iter()
            .map(|bystander| ConditionContext::entity(&world, position, bystander.as_ref()))
            .collect();
        trigger(self, &LIGHTNING_STRIKE, |conditions| {
            test_context(conditions.lightning.as_ref(), &lightning)
                && conditions.bystander.as_ref().is_none_or(|predicate| {
                    bystanders.iter().any(|bystander| predicate.test(bystander))
                })
        });
    }

    /// Vanilla `ChanneledLightningTrigger.trigger(player, victims)`: every
    /// `victims` predicate matches one of the struck entities.
    pub(crate) fn trigger_channeled_lightning(&self, victims: &[SharedEntity]) {
        let world = self.get_world();
        let position = self.position();
        let victims: Vec<_> = victims
            .iter()
            .map(|victim| ConditionContext::entity(&world, position, victim.as_ref()))
            .collect();
        trigger(self, &CHANNELED_LIGHTNING, |conditions| {
            conditions
                .victims
                .iter()
                .all(|predicate| victims.iter().any(|victim| predicate.test(victim)))
        });
    }

    /// Vanilla `UsedEnderEyeTrigger.trigger(player, feature)`: the squared
    /// horizontal distance to the structure the eye flies to.
    pub(crate) fn trigger_used_ender_eye(&self, feature: BlockPos) {
        let position = self.position();
        let dx = position.x - f64::from(feature.x());
        let dz = position.z - f64::from(feature.z());
        let distance_sqr = dx * dx + dz * dz;
        trigger(self, &USED_ENDER_EYE, |conditions| {
            ender_eye_distance_matches(conditions.distance.as_ref(), distance_sqr)
        });
    }
}

/// Vanilla `UsedEnderEyeTrigger.TriggerInstance.matches`.
fn ender_eye_distance_matches(distance: Option<&DistanceField>, distance_sqr: f64) -> bool {
    match distance {
        Some(DistanceField::Bounds(bounds)) => bounds.matches_sqr(distance_sqr),
        // Vanilla bounds ignore unknown fields, so a distance predicate reads
        // as no bounds at all.
        Some(DistanceField::Distance(_)) | None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::super::Conditions;
    use super::ender_eye_distance_matches;

    #[test]
    fn ender_eye_bounds_compare_the_squared_horizontal_distance() {
        let conditions: Conditions =
            serde_json::from_str(r#"{"distance": {"min": 10.0, "max": 20.0}}"#)
                .expect("ender eye bounds decode");
        let distance = conditions.distance.as_ref();
        assert!(!ender_eye_distance_matches(distance, 99.0));
        assert!(ender_eye_distance_matches(distance, 100.0));
        assert!(ender_eye_distance_matches(distance, 400.0));
        assert!(!ender_eye_distance_matches(distance, 401.0));
        assert!(ender_eye_distance_matches(None, 1.0e9));
    }
}
