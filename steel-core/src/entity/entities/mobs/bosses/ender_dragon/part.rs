//! Vanilla `EnderDragonPart`: one hitbox of the Ender Dragon's body.
//!
//! Parts never join the world's entity storage and are never sent to clients:
//! the client builds its own parts with the ids that follow the dragon's id.
//! The world finds them through its Ender Dragon index so attacks, projectiles
//! and explosions can hit them.

use std::sync::Weak;

use glam::DVec3;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_utils::{Downcast as _, DowncastType, DowncastTypeKey};

use super::EnderDragonEntity;
use crate::entity::damage::DamageSource;
use crate::entity::{Entity, EntityBase, EntityPose};
use crate::world::World;

/// One hitbox of an Ender Dragon.
pub struct EnderDragonPartEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    /// The network id of the dragon this part belongs to.
    parent_id: i32,
    /// Vanilla `name`: `head`, `neck`, `body`, `tail` or `wing`.
    name: &'static str,
    dimensions: EntityDimensions,
}

// SAFETY: This key is owned by Steel and uniquely identifies `EnderDragonPartEntity`.
unsafe impl DowncastType for EnderDragonPartEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/ender_dragon_part");
}

impl EnderDragonPartEntity {
    /// Creates a part of the dragon with id `parent_id`.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors vanilla's part constructor plus Steel's entity base inputs"
    )]
    pub(super) fn new(
        entity_type: EntityTypeRef,
        id: i32,
        parent_id: i32,
        name: &'static str,
        width: f32,
        height: f32,
        position: DVec3,
        world: Weak<World>,
    ) -> Self {
        let dimensions = EntityDimensions::with_default_eye_height(width, height);
        Self {
            base: EntityBase::new(id, position, dimensions, world),
            entity_type,
            parent_id,
            name,
            dimensions,
        }
    }

    /// Vanilla `name`.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// The network id of the dragon this part belongs to.
    #[must_use]
    pub const fn parent_id(&self) -> i32 {
        self.parent_id
    }
}

impl Entity for EnderDragonPartEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    /// Vanilla parts report their dragon's type.
    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        self.dimensions
    }

    fn is_pickable(&self) -> bool {
        true
    }

    /// Vanilla `EnderDragonPart.getPickResult`: the dragon's pick result.
    fn pick_result(&self) -> Option<ItemStack> {
        self.level()?
            .get_entity_by_id(self.parent_id)?
            .pick_result()
    }

    /// Vanilla `EnderDragonPart.hurtServer`: the dragon takes the hit for this
    /// part.
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        if self.is_invulnerable_to_base(source) {
            return false;
        }
        let Some(parent) = world.get_entity_by_id(self.parent_id) else {
            return false;
        };
        parent
            .downcast_ref::<EnderDragonEntity>()
            .is_some_and(|dragon| dragon.hurt_part(world, self, source, amount))
    }
}
