//! The spawner minecart (vanilla `MinecartSpawner`).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_entity_data::{
    AbstractMinecartEntityData, SpawnerMinecartEntityData, VehicleEntityData,
};
use steel_registry::vanilla_items;
use steel_utils::axis::Axis;
use steel_utils::block_util::FoundRectangle;
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::{Direction, DowncastType, DowncastTypeKey};

use super::abstract_minecart::{AbstractMinecart, MinecartCore, tick_minecart};
use super::vehicle_entity::VehicleEntity;
use crate::block_entity::entities::BaseSpawner;
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityMovementEmission, EntitySyncedData, MoveResult,
    reset_forward_direction_of_relative_portal_position,
};
use crate::physics::MoverType;
use crate::portal::portal_shape::PortalShape;
use crate::world::{Explosion, World};

/// A minecart carrying a working mob spawner.
#[entity_behavior(class = "MinecartSpawner")]
pub struct SpawnerMinecartEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<SpawnerMinecartEntityData>,
    minecart: MinecartCore,
    spawner: BaseSpawner,
}

// SAFETY: This key is owned by Steel and uniquely identifies `SpawnerMinecartEntity`.
unsafe impl DowncastType for SpawnerMinecartEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/spawner_minecart");
}

impl SpawnerMinecartEntity {
    /// Creates a new spawner minecart.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Creates a spawner minecart from saved data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        Self {
            base,
            entity_type,
            entity_data: SyncMutex::new(SpawnerMinecartEntityData::new()),
            minecart: MinecartCore::new(),
            spawner: BaseSpawner::new(),
        }
    }

    /// Vanilla `MinecartSpawner.getSpawner`.
    #[must_use]
    pub const fn spawner(&self) -> &BaseSpawner {
        &self.spawner
    }
}

impl VehicleEntity for SpawnerMinecartEntity {
    fn with_vehicle_data(&self, f: &mut dyn FnMut(&mut VehicleEntityData)) {
        f(&mut self
            .entity_data
            .lock()
            .abstract_minecart_mut()
            .vehicle_entity);
    }

    /// Vanilla `MinecartSpawner.getDropItem`: only the minecart comes back.
    fn drop_item(&self) -> ItemRef {
        &vanilla_items::MINECART
    }
}

impl AbstractMinecart for SpawnerMinecartEntity {
    fn minecart_core(&self) -> &MinecartCore {
        &self.minecart
    }

    fn with_minecart_data(&self, f: &mut dyn FnMut(&mut AbstractMinecartEntityData)) {
        f(self.entity_data.lock().abstract_minecart_mut());
    }
}

impl Entity for SpawnerMinecartEntity {
    /// Vanilla `MinecartSpawner.getPickResult`: a plain minecart.
    fn pick_result(&self) -> Option<ItemStack> {
        Some(ItemStack::new(&vanilla_items::MINECART))
    }

    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    /// Vanilla `MinecartSpawner.tick`: roll like any cart, then run the spawner
    /// at the cart's block.
    fn tick(self: Arc<Self>) {
        tick_minecart(&self);
        if let Some(world) = self.level() {
            let cart = Arc::clone(&self);
            self.spawner
                .server_tick_broadcasting(&world, self.block_position(), &|_, _, _| {
                    // Vanilla `broadcastEntityEvent(this, id)`; the spawner's only
                    // event id, 1, shares its byte with `Jump`.
                    cart.broadcast_entity_event(EntityStatus::Jump);
                });
        }
    }

    fn move_entity(self: Arc<Self>, mover_type: MoverType, delta: DVec3) -> Option<MoveResult> {
        let result = Arc::clone(&self).default_move_entity(mover_type, delta);
        self.apply_effects_from_blocks();
        result
    }

    fn apply_effects_from_blocks(&self) {
        let position = self.position();
        self.apply_effects_from_blocks_between(position, position);
        self.base().clear_movement_this_tick();
    }

    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        self.hurt_vehicle(world, source, amount)
    }

    fn is_on_rails(&self) -> bool {
        self.minecart_on_rails()
    }

    fn movement_emission(&self) -> EntityMovementEmission {
        EntityMovementEmission::Events
    }

    fn is_pickable(&self) -> bool {
        !self.is_removed()
    }

    fn is_pushable(&self) -> bool {
        true
    }

    fn blocks_building(&self) -> bool {
        true
    }

    fn can_collide_with(&self, other: &dyn Entity) -> bool {
        (other.can_be_collided_with(Some(self)) || other.is_pushable())
            && !self.is_passenger_of_same_vehicle(other)
            && !super::abstract_minecart::passes_through(self, other)
    }

    fn push_entity(&self, entity: &dyn Entity) {
        self.push_minecart_entity(entity);
    }

    fn passenger_attachment_point(&self, passenger: &dyn Entity) -> DVec3 {
        self.minecart_passenger_attachment_point(passenger)
            .unwrap_or_else(|| self.default_passenger_attachment_point(passenger))
    }

    fn dismount_location_for_passenger(&self, passenger: &dyn Entity) -> DVec3 {
        self.minecart_dismount_location(passenger)
    }

    fn motion_direction(&self) -> Direction {
        self.minecart_motion_direction()
    }

    fn known_movement(&self) -> DVec3 {
        let movement = self.velocity();
        if movement.is_nan() {
            return DVec3::ZERO;
        }
        DVec3::new(
            movement.x.clamp(-0.4, 0.4),
            movement.y,
            movement.z.clamp(-0.4, 0.4),
        )
    }

    fn block_speed_factor(&self) -> f32 {
        self.minecart_block_speed_factor()
    }

    fn get_default_gravity(&self) -> f64 {
        self.minecart_gravity()
    }

    fn ignore_explosion(&self, explosion: &dyn Explosion) -> bool {
        self.vehicle_ignores_explosion(explosion)
    }

    fn dimension_changing_delay(&self) -> i32 {
        10
    }

    fn get_relative_portal_position(&self, axis: Axis, portal_area: FoundRectangle) -> DVec3 {
        reset_forward_direction_of_relative_portal_position(PortalShape::get_relative_position(
            portal_area,
            axis,
            self.position(),
            self.dimensions_for_pose(self.pose()),
        ))
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    /// Vanilla `MinecartSpawner.addAdditionalSaveData`.
    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_minecart(nbt);
        self.spawner.save(nbt);
    }

    /// Vanilla `MinecartSpawner.readAdditionalSaveData`.
    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_minecart(nbt);
        self.spawner.load_view(nbt);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use simdnbt::borrow::read_compound;
    use steel_registry::{init_vanilla_registry, vanilla_entities};
    use steel_utils::nbt::parse_snbt_compound;
    use steel_utils::{ChunkPos, Downcast as _, WorldAabb};

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::entity::entities::PigEntity;
    use crate::entity::{SharedEntity, init_entities, next_entity_id};
    use crate::player::ResetReason;
    use crate::test_support::{TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk};

    fn load(cart: &SpawnerMinecartEntity, snbt: &str) {
        let nbt = parse_snbt_compound(snbt).expect("valid snbt");
        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let borrowed = read_compound(&mut Cursor::new(&bytes)).expect("nbt");
        cart.load_additional((&borrowed).into());
    }

    #[test]
    fn a_spawner_minecart_spawns_around_itself_and_saves_its_spawner() {
        init_vanilla_registry();
        init_behaviors();
        init_entities();
        let fixture = fresh_test_world("spawner_minecart");
        let world = &fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let player = TestPlayerBuilder::new(Arc::clone(world), "Watcher", next_entity_id()).build();
        assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));
        player
            .try_set_position(DVec3::new(8.5, 80.0, 4.5))
            .expect("move player");

        let cart = Arc::new(SpawnerMinecartEntity::new(
            &vanilla_entities::SPAWNER_MINECART,
            next_entity_id(),
            DVec3::new(8.5, 80.0, 8.5),
            Arc::downgrade(world),
        ));
        cart.set_no_gravity(true);
        load(
            &cart,
            "{Delay: 0s, MinSpawnDelay: 10s, MaxSpawnDelay: 10s, SpawnCount: 4s, \
             SpawnData: {entity: {id: \"minecraft:pig\"}, \
             custom_spawn_rules: {block_light_limit: [0, 15], sky_light_limit: [0, 15]}}}",
        );
        assert!(
            world
                .try_add_entity(Arc::clone(&cart) as SharedEntity)
                .is_ok()
        );

        Arc::clone(&cart).tick();

        let pigs = world.get_entities_in_aabb_matching(
            &WorldAabb::of_size(DVec3::new(8.5, 80.0, 8.5), 12.0, 12.0, 12.0),
            |entity| entity.downcast_ref::<PigEntity>().is_some(),
        );
        assert!(!pigs.is_empty(), "delay {}", cart.spawner().spawn_delay());
        assert_eq!(cart.spawner().spawn_delay(), 10);

        let mut saved = NbtCompound::new();
        cart.save_additional(&mut saved);
        assert_eq!(saved.short("Delay"), Some(10));
        assert!(saved.compound("SpawnData").is_some());
    }
}
