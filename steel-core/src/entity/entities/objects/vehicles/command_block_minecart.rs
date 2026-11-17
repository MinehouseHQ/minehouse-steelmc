//! The command block minecart (vanilla `MinecartCommandBlock`).

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_entity_data::{
    AbstractMinecartEntityData, CommandBlockMinecartEntityData, VehicleEntityData,
};
use steel_registry::vanilla_items;
use steel_utils::axis::Axis;
use steel_utils::block_util::FoundRectangle;
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, Direction, Downcast as _, DowncastType, DowncastTypeKey};

use super::abstract_minecart::{AbstractMinecart, MinecartCore, tick_minecart};
use super::vehicle_entity::VehicleEntity;
use crate::behavior::InteractionResult;
use crate::command::command_block::{BaseCommandBlock, CommandBlockOrigin, CommandBlockRunner};
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityMovementEmission, EntitySyncedData, MoveResult,
    SharedEntity, reset_forward_direction_of_relative_portal_position,
};
use crate::physics::MoverType;
use crate::player::Player;
use crate::portal::portal_shape::PortalShape;
use crate::world::{CommandBlockRun, Explosion, World};

/// Vanilla `MinecartCommandBlock.ACTIVATION_DELAY`: ticks between two runs.
const ACTIVATION_DELAY: i32 = 4;

/// A minecart carrying a command block, run by powered activator rails.
#[entity_behavior(class = "MinecartCommandBlock")]
pub struct CommandBlockMinecartEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<CommandBlockMinecartEntityData>,
    minecart: MinecartCore,
    command_block: Arc<BaseCommandBlock>,
    last_activated: AtomicI32,
}

// SAFETY: This key is owned by Steel and uniquely identifies `CommandBlockMinecartEntity`.
unsafe impl DowncastType for CommandBlockMinecartEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/command_block_minecart");
}

impl CommandBlockMinecartEntity {
    /// Creates a new command block minecart.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Creates a command block minecart from saved data.
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
            entity_data: SyncMutex::new(CommandBlockMinecartEntityData::new()),
            minecart: MinecartCore::new(),
            command_block: Arc::new(BaseCommandBlock::new()),
            last_activated: AtomicI32::new(0),
        }
    }

    /// Vanilla `getCommandBlock`.
    #[must_use]
    pub const fn command_block(&self) -> &Arc<BaseCommandBlock> {
        &self.command_block
    }

    /// Vanilla `MinecartCommandBase.onUpdated`: shows the command and its
    /// last output to watching clients.
    pub fn sync_command_block(&self) {
        let command = self.command_block.command();
        let last_output = self.command_block.last_output();
        let mut data = self.entity_data.lock();
        data.minecart_command_block.id_command_name.set(command);
        data.minecart_command_block
            .id_last_output
            .set(Box::new(last_output));
    }

    /// Runs the command for a queued activation: vanilla `performCommand` from
    /// the cart's position and rotation, with the cart as the executing entity.
    pub(crate) fn run_command(
        entity: &SharedEntity,
        world: &Arc<World>,
        runner: &mut CommandBlockRunner<'_>,
    ) {
        let Some(cart) = entity.downcast_ref::<Self>() else {
            return;
        };
        let weak = Arc::downgrade(entity);
        let origin = CommandBlockOrigin {
            position: cart.position(),
            rotation: cart.rotation(),
            entity: Some(Arc::clone(entity)),
            on_updated: Arc::new(move |_world: &World| {
                if let Some(entity) = weak.upgrade()
                    && let Some(cart) = entity.downcast_ref::<Self>()
                {
                    cart.sync_command_block();
                }
            }),
        };
        cart.command_block.perform_command(runner, world, origin);
    }
}

impl VehicleEntity for CommandBlockMinecartEntity {
    fn with_vehicle_data(&self, f: &mut dyn FnMut(&mut VehicleEntityData)) {
        f(&mut self
            .entity_data
            .lock()
            .abstract_minecart_mut()
            .vehicle_entity);
    }

    /// Vanilla `MinecartCommandBlock.getDropItem`: only the minecart comes back.
    fn drop_item(&self) -> ItemRef {
        &vanilla_items::MINECART
    }
}

impl AbstractMinecart for CommandBlockMinecartEntity {
    fn minecart_core(&self) -> &MinecartCore {
        &self.minecart
    }

    fn with_minecart_data(&self, f: &mut dyn FnMut(&mut AbstractMinecartEntityData)) {
        f(self.entity_data.lock().abstract_minecart_mut());
    }

    /// Vanilla `activateMinecart`: a powered activator rail runs the command at
    /// most every four ticks. The server runs it once every world has ticked.
    fn activate_minecart(&self, world: &Arc<World>, _pos: BlockPos, powered: bool) {
        let tick_count = self.tick_count();
        if powered && tick_count - self.last_activated.load(Ordering::Relaxed) >= ACTIVATION_DELAY {
            world.queue_command_block_run(CommandBlockRun::Minecart(self.id()));
            self.last_activated.store(tick_count, Ordering::Relaxed);
        }
    }
}

impl Entity for CommandBlockMinecartEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        tick_minecart(&self);
    }

    /// Vanilla `MinecartCommandBlock.interact`: game masters open the
    /// command screen, which the client shows on its own.
    fn interact(
        &self,
        player: &Player,
        _hand: InteractionHand,
        _location: DVec3,
    ) -> InteractionResult {
        if player.can_use_game_master_blocks() {
            InteractionResult::Success
        } else {
            InteractionResult::Pass
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

    /// Vanilla `MinecartCommandBlock.addAdditionalSaveData`.
    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_minecart(nbt);
        self.command_block.save(nbt);
    }

    /// Vanilla `MinecartCommandBlock.readAdditionalSaveData`.
    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_minecart(nbt);
        self.command_block.load(&nbt);
        self.sync_command_block();
    }
}
