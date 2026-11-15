//! Illagers (Vanilla `AbstractIllager`): pillagers, vindicators, evokers and illusioners.

use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::{REGISTRY, TaggedRegistryExt, vanilla_entities};

use crate::entity::{Entity, LivingEntity, Raider};

/// Vanilla `AbstractIllager`.
pub trait AbstractIllager: Raider {
    /// Vanilla `AbstractIllager.canAttack`: illagers leave baby villagers alone.
    fn illager_can_attack(&self, target: &dyn LivingEntity) -> bool {
        if is_abstract_villager(target) && target.is_baby() {
            return false;
        }
        self.default_can_attack_mob(target)
    }

    /// Vanilla `AbstractIllager.considersEntityAsAlly`: the team, and
    /// otherwise `illager_friends` when neither is on a team.
    fn illager_is_allied_to(&self, other: &dyn Entity) -> bool {
        if self.is_allied_to_team(other.team().as_ref()) {
            return true;
        }
        REGISTRY
            .entity_types
            .is_in_tag(other.entity_type(), &EntityTypeTag::ILLAGER_FRIENDS)
            && self.team().is_none()
            && other.team().is_none()
    }
}

/// Vanilla `instanceof AbstractVillager`: villagers and wandering traders.
#[must_use]
pub(crate) fn is_abstract_villager(entity: &dyn LivingEntity) -> bool {
    let entity_type = entity.entity_type();
    entity_type == &vanilla_entities::VILLAGER || entity_type == &vanilla_entities::WANDERING_TRADER
}

/// Vanilla `instanceof AbstractIllager`: evokers, illusioners, pillagers and
/// vindicators.
#[must_use]
pub(crate) fn is_abstract_illager(entity: &dyn Entity) -> bool {
    let entity_type = entity.entity_type();
    entity_type == &vanilla_entities::EVOKER
        || entity_type == &vanilla_entities::ILLUSIONER
        || entity_type == &vanilla_entities::PILLAGER
        || entity_type == &vanilla_entities::VINDICATOR
}
