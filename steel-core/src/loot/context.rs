//! Vanilla `LootContext`: the parameters a condition or number provider reads.

use std::sync::Arc;

use glam::DVec3;
use serde::Deserialize;
use steel_registry::item_stack::ItemStack;
use steel_utils::BlockStateId;

use super::LootRegistries;
use crate::command::storage::CommandStorage;
use crate::entity::Entity;
use crate::entity::damage::DamageSource;
use crate::world::World;

/// Vanilla `LootContext.EntityTarget`: which entity parameter a condition or
/// provider reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EntityTarget {
    /// `LootContextParams.THIS_ENTITY`.
    This,
    /// `LootContextParams.ATTACKING_ENTITY`.
    Attacker,
    /// `LootContextParams.DIRECT_ATTACKING_ENTITY`.
    DirectAttacker,
    /// `LootContextParams.LAST_DAMAGE_PLAYER`.
    AttackingPlayer,
    /// `LootContextParams.TARGET_ENTITY`.
    TargetEntity,
    /// `LootContextParams.INTERACTING_ENTITY`.
    InteractingEntity,
}

/// Vanilla `LootContext`: the level, the parameters the caller supplied and
/// the registries references resolve in. A parameter left `None` is one the
/// caller's parameter set does not supply.
pub(crate) struct LootContext<'a> {
    /// The level the context is in.
    pub(crate) world: &'a Arc<World>,
    /// `ORIGIN`.
    pub(crate) origin: Option<DVec3>,
    /// `THIS_ENTITY`.
    pub(crate) this: Option<&'a dyn Entity>,
    /// `ATTACKING_ENTITY`.
    pub(crate) attacker: Option<&'a dyn Entity>,
    /// `DIRECT_ATTACKING_ENTITY`.
    pub(crate) direct_attacker: Option<&'a dyn Entity>,
    /// `LAST_DAMAGE_PLAYER`.
    pub(crate) attacking_player: Option<&'a dyn Entity>,
    /// `TARGET_ENTITY`.
    pub(crate) target_entity: Option<&'a dyn Entity>,
    /// `INTERACTING_ENTITY`.
    pub(crate) interacting_entity: Option<&'a dyn Entity>,
    /// `TOOL`.
    pub(crate) tool: Option<&'a ItemStack>,
    /// `BLOCK_STATE`.
    pub(crate) block_state: Option<BlockStateId>,
    /// `DAMAGE_SOURCE`.
    pub(crate) damage_source: Option<&'a DamageSource>,
    /// `EXPLOSION_RADIUS`.
    pub(crate) explosion_radius: Option<f32>,
    /// `ENCHANTMENT_LEVEL`.
    pub(crate) enchantment_level: Option<i32>,
    /// `ENCHANTMENT_ACTIVE`.
    pub(crate) enchantment_active: Option<bool>,
    /// The domain's `predicate`, `context_int_provider` and
    /// `context_float_provider` registries; without them no reference
    /// resolves.
    pub(crate) registries: Option<&'a LootRegistries>,
    /// The domain's command storage, which `storage` providers read.
    pub(crate) storage: Option<&'a CommandStorage>,
}

impl<'a> LootContext<'a> {
    /// A context in `world` with no parameters.
    pub(crate) const fn new(world: &'a Arc<World>) -> Self {
        Self {
            world,
            origin: None,
            this: None,
            attacker: None,
            direct_attacker: None,
            attacking_player: None,
            target_entity: None,
            interacting_entity: None,
            tool: None,
            block_state: None,
            damage_source: None,
            explosion_radius: None,
            enchantment_level: None,
            enchantment_active: None,
            registries: None,
            storage: None,
        }
    }

    /// Vanilla `EntityPredicate.createContext(player, entity)`: `this` is the
    /// entity, the origin is the player's position.
    pub(crate) const fn entity(
        world: &'a Arc<World>,
        origin: DVec3,
        entity: &'a dyn Entity,
    ) -> Self {
        let mut ctx = Self::new(world);
        ctx.origin = Some(origin);
        ctx.this = Some(entity);
        ctx
    }

    /// The entity parameter `target` names.
    pub(crate) const fn entity_for(&self, target: EntityTarget) -> Option<&'a dyn Entity> {
        match target {
            EntityTarget::This => self.this,
            EntityTarget::Attacker => self.attacker,
            EntityTarget::DirectAttacker => self.direct_attacker,
            EntityTarget::AttackingPlayer => self.attacking_player,
            EntityTarget::TargetEntity => self.target_entity,
            EntityTarget::InteractingEntity => self.interacting_entity,
        }
    }
}
