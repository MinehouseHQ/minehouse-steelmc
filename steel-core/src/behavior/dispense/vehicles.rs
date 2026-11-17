//! Dispenser behaviors that launch boats and minecarts (Vanilla
//! `BoatDispenseItemBehavior` and `MinecartDispenseItemBehavior`).

use std::sync::Arc;

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction, RailShape};
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_fluid_tags::FluidTag;
use steel_utils::BlockStateId;

use super::{BlockSource, default_dispense, entity_placement::direction_y_rot};
use crate::behavior::blocks::is_rail_state;
use crate::entity::{add_spawned_entity, apply_item_stack_components, create_entity_instance};

/// How far above the dispenser's center a boat fired up or down spawns.
const BOAT_VERTICAL_STEP: f64 = 1.125;
/// How far from the dispenser's face a boat's near edge spawns.
const BOAT_FACE_GAP: f64 = 0.5625;
/// How far from the dispenser's center a minecart spawns.
const MINECART_STEP: f64 = 1.125;

/// Vanilla `BoatDispenseItemBehavior.execute`: launches the boat onto the
/// water in front, or drops it when there is none.
pub(super) fn boat(
    source: &BlockSource<'_>,
    mut stack: ItemStack,
    entity_type: EntityTypeRef,
) -> ItemStack {
    let direction = source.facing();
    let world = source.world;
    let center = source.center();
    let (step_x, step_y, step_z) = direction.offset();
    let just_outside = BOAT_FACE_GAP + f64::from(entity_type.dimensions.width) / 2.0;
    let spawn_x = center.x + f64::from(step_x) * just_outside;
    let spawn_y = center.y + f64::from(step_y) * BOAT_VERTICAL_STEP;
    let spawn_z = center.z + f64::from(step_z) * just_outside;
    let front = source.target();
    let y_offset = if is_water(world.get_block_state(front)) {
        1.0
    } else if !world.get_block_state(front).is_air()
        || !is_water(world.get_block_state(front.below()))
    {
        return default_dispense(source, stack);
    } else {
        0.0
    };

    let position = DVec3::new(spawn_x, spawn_y + y_offset, spawn_z);
    let Ok(boat) = create_entity_instance(world, entity_type, position) else {
        return stack;
    };
    boat.set_old_position_to_current();
    if apply_item_stack_components(&boat, &stack, false).is_err() {
        return stack;
    }
    boat.set_rotation((direction_y_rot(direction), boat.rotation().1));
    if add_spawned_entity(world, Arc::clone(&boat)).is_ok() {
        stack.shrink(1);
    }
    stack
}

/// Vanilla `MinecartDispenseItemBehavior.execute`: puts the cart on the rail
/// in front (or the rail below an empty block in front), or drops it.
pub(super) fn minecart(
    source: &BlockSource<'_>,
    mut stack: ItemStack,
    entity_type: EntityTypeRef,
) -> ItemStack {
    let direction = source.facing();
    let world = source.world;
    let center = source.center();
    let (step_x, step_y, step_z) = direction.offset();
    let spawn_x = center.x + f64::from(step_x) * MINECART_STEP;
    let spawn_y = center.y.floor() + f64::from(step_y);
    let spawn_z = center.z + f64::from(step_z) * MINECART_STEP;
    let front = source.target();
    let front_state = world.get_block_state(front);
    let y_offset = if front_state.get_block().has_tag(&BlockTag::RAILS) {
        if rail_shape(front_state).is_slope() {
            0.6
        } else {
            0.1
        }
    } else {
        if !front_state.is_air() {
            return default_dispense(source, stack);
        }
        let below = world.get_block_state(front.below());
        if !below.get_block().has_tag(&BlockTag::RAILS) {
            return default_dispense(source, stack);
        }
        if direction != Direction::Down && rail_shape(below).is_slope() {
            -0.4
        } else {
            -0.9
        }
    };

    let position = DVec3::new(spawn_x, spawn_y + y_offset, spawn_z);
    let Ok(cart) = create_entity_instance(world, entity_type, position) else {
        return stack;
    };
    cart.set_old_position_to_current();
    if apply_item_stack_components(&cart, &stack, false).is_err() {
        return stack;
    }
    if add_spawned_entity(world, Arc::clone(&cart)).is_ok() {
        stack.shrink(1);
    }
    stack
}

/// Vanilla `level.getFluidState(pos).is(FluidTags.WATER)`.
fn is_water(state: BlockStateId) -> bool {
    state.get_fluid_state().fluid_id.has_tag(&FluidTag::WATER)
}

/// Vanilla `MinecartDispenseItemBehavior.getRailShape`.
fn rail_shape(state: BlockStateId) -> RailShape {
    if is_rail_state(state) {
        state.get_value(&BlockStateProperties::RAIL_SHAPE)
    } else {
        RailShape::NorthSouth
    }
}
