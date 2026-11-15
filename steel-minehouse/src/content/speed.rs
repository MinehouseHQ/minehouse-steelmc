//! Fast minecarts, up to [`MAX_SPEED`] blocks a second (faster than a boat on blue ice). A
//! port of the Fabric `MinecartSpeed`.
//!
//! The server moves a minecart at most 0.4 blocks a tick (8 blocks a second). The movement
//! each tick is a straight line followed by snapping back onto the rail, so a cart that
//! moves too far in one tick cuts corners and flies off curves and slopes. So a fast cart
//! covers its tick in substeps: as many of the server's passes along the track as it
//! takes, each as long as the server allows where the cart is (up to [`MAX_STEP`] on a
//! straight flat rail followed by more of it, and the server's own 0.4 on curves, slopes
//! and everything else). Curves and slopes are therefore taken at full speed and as
//! safely as before, and no rail is ever skipped. Speed changes from each substep
//! (friction, slopes, powered rails, braking) are added up, so they act per tick and per
//! block as before, except that a fast cart has no rolling friction on straight flat
//! track, and every powered rail passed adds [`BOOST_PER_RAIL`]. The substeps stop early
//! at a metro station (so a train never jumps past it), off the rails, and in front of
//! chunks that do not tick entities.
//!
//! Only a cart or train with a player in it is fast.

use std::{cell::Cell, sync::Arc};

use glam::DVec3;
use steel_core::{entity::Entity, world::World};
use steel_registry::{
    blocks::{
        block_state_ext::BlockStateExt as _,
        properties::{BlockStateProperties, RailShape},
    },
    vanilla_block_tags::BlockTag,
    vanilla_blocks,
};
use steel_utils::{BlockPos, BlockStateId, ChunkPos};
use uuid::Uuid;

use super::{ride, trains};

/// The top speed of a fast cart, blocks a second.
const MAX_SPEED: f64 = 80.0;
/// What every powered rail passed adds to a fast cart, blocks a second.
const BOOST_PER_RAIL: f64 = 6.0;
/// Blocks a tick: keeps every pass within the current or the next block.
const MAX_STEP: f64 = 0.99;
/// A coupling may pull a cart this much over the top speed.
const CATCH_UP: f64 = 1.25;
/// Substeps a tick at most: the highest speed there could be through curves (0.4 a
/// substep).
const MAX_SUBSTEPS: usize = 14;

thread_local! {
    /// The cart being moved in substeps right now.
    static MOVING: Cell<Option<Uuid>> = const { Cell::new(None) };
}

fn horizontal_length(vector: DVec3) -> f64 {
    vector.x.hypot(vector.z)
}

/// `NorthSouth` or `EastWest` for a straight flat rail, otherwise `None`.
fn flat_shape(state: BlockStateId) -> Option<RailShape> {
    if !state.get_block().has_tag(&BlockTag::RAILS) {
        return None;
    }
    let shape = state.get_value(&BlockStateProperties::RAIL_SHAPE);
    matches!(shape, RailShape::NorthSouth | RailShape::EastWest).then_some(shape)
}

fn rail_under(cart: &dyn Entity) -> Option<BlockPos> {
    let minecart = cart.as_abstract_minecart()?;
    Some(minecart.current_block_pos_or_rail_below())
}

/// May this cart go faster than the server lets it right now? A train is fast if anyone
/// rides any of its carts, or the empty wagons would fall behind.
fn fast(cart: &dyn Entity) -> bool {
    cart.is_on_rails() && !cart.is_in_water() && trains::carries_player(cart)
}

/// The limit for one pass along the track, or `vanilla` when the cart should not go
/// faster here.
pub(super) fn max_speed(cart: &dyn Entity, vanilla: f64) -> f64 {
    // While a cart moves in substeps it is known to be fast; checking its train again for
    // a rider on every substep is what would cost the most.
    if MOVING.get() != Some(cart.uuid()) && !fast(cart) {
        return vanilla;
    }
    let boosted = (MAX_SPEED / 20.0).min(MAX_STEP);
    if boosted <= vanilla {
        return vanilla;
    }
    let (Some(world), Some(pos)) = (cart.level(), rail_under(cart)) else {
        return vanilla;
    };
    let Some(shape) = flat_shape(world.get_block_state(pos)) else {
        return vanilla;
    };
    let movement = cart.velocity();
    let east_west = shape == RailShape::EastWest;
    let along = if east_west { movement.x } else { movement.z };
    if along == 0.0 {
        return vanilla;
    }
    let step = if along > 0.0 { 1 } else { -1 };
    let next = if east_west {
        pos.offset(step, 0, 0)
    } else {
        pos.offset(0, 0, step)
    };
    if flat_shape(world.get_block_state(next)) == Some(shape) {
        boosted
    } else {
        vanilla
    }
}

/// Takes the place of the server's one pass along the track for a cart on rails. `pass`
/// is that pass. `false` leaves it to the server.
pub(super) fn drive(cart: &dyn Entity, world: &Arc<World>, pass: &mut dyn FnMut()) -> bool {
    if trains::follows(cart) {
        // The front cart of its train moves it.
        return true;
    }
    if !fast(cart) {
        return false;
    }
    MOVING.set(Some(cart.uuid()));
    substeps(cart, world, pass);
    MOVING.set(None);
    true
}

fn set_horizontal(cart: &dyn Entity, speed: DVec3) {
    cart.set_velocity(DVec3::new(speed.x, cart.velocity().y, speed.z));
}

/// Speeds here are the cart's own (its velocity); the server moves a ridden cart 0.75 of
/// it.
fn substeps(cart: &dyn Entity, world: &Arc<World>, pass: &mut dyn FnMut()) {
    let Some(minecart) = cart.as_abstract_minecart() else {
        pass();
        return;
    };
    let scale = if cart.is_vehicle() { 0.75 } else { 1.0 };
    let top = MAX_SPEED / 20.0 / scale;
    let start = cart.velocity();
    let start_speed = horizontal_length(start);
    let mut speed = start_speed.min(top * CATCH_UP);
    // Its own physics (powered rails) never take a cart over the top speed; a coupling may.
    let cap = top.max(speed);
    if speed < 1.0e-6 {
        pass();
        return;
    }
    let mut direction = DVec3::new(start.x / start_speed, 0.0, start.z / start_speed);
    let boost = BOOST_PER_RAIL / 20.0 / scale;
    // Distance to cover this tick; each substep is as long as the server allows where the
    // cart is.
    let mut left = speed * scale;

    for substep in 0..MAX_SUBSTEPS + 3 {
        if left <= 1.0e-4 {
            break;
        }
        let rail_pos = minecart.current_block_pos_or_rail_below();
        let rail = world.get_block_state(rail_pos);
        if substep > 0 && !keep_going(cart, world, rail_pos, rail, direction) {
            break;
        }

        set_horizontal(cart, direction);
        let vanilla = if cart.is_in_water() { 0.2 } else { 0.4 };
        let step = left.min(max_speed(cart, vanilla));
        left -= step;
        let share = step / scale;
        let from = cart.position();
        set_horizontal(cart, direction * share);
        pass();
        // The first rail's activator is run by the server right after this; the others
        // are ours.
        if substep > 0 && rail.get_block() == &vanilla_blocks::ACTIVATOR_RAIL {
            minecart.activate_minecart(
                world,
                rail_pos,
                rail.get_value(&BlockStateProperties::POWERED),
            );
        }

        let after = cart.velocity();
        let after_speed = horizontal_length(after);
        if after_speed < 1.0e-6 {
            // Stopped (an unpowered powered rail, a wall).
            speed = 0.0;
            break;
        }
        // Friction, slopes and powered rails changed the share; the whole speed changes
        // as much.
        speed = (speed + after_speed - share).max(0.0);
        let powered_rail = rail.get_block() == &vanilla_blocks::POWERED_RAIL;
        if flat_shape(rail).is_some() && !powered_rail {
            // A fast cart keeps its speed on straight flat track (no rolling friction
            // there), so one booster carries it a long way; curves and slopes still slow
            // it down.
            speed += share * (1.0 - minecart.slowdown_factor());
        }
        direction = DVec3::new(after.x / after_speed, 0.0, after.z / after_speed);
        if powered_rail && rail.get_value(&BlockStateProperties::POWERED) {
            let moved = cart.position() - from;
            speed += boost * horizontal_length(moved).min(1.0);
        }
        trains::step(cart);
        ride::passed(cart, world);
        if cart.is_removed() || !cart.is_on_rails() || cart.is_in_water() {
            break;
        }
    }
    set_horizontal(cart, direction * speed.min(cap));
}

/// Before another substep: still on rails, not at a station to stop at, the way ahead
/// ticking.
fn keep_going(
    cart: &dyn Entity,
    world: &Arc<World>,
    rail_pos: BlockPos,
    rail: BlockStateId,
    direction: DVec3,
) -> bool {
    if !rail.get_block().has_tag(&BlockTag::RAILS) || ride::stops_at(cart, world, rail_pos) {
        return false;
    }
    let ahead = cart.position() + direction * 2.0;
    let ahead = BlockPos::containing(ahead.x, ahead.y, ahead.z);
    world
        .chunk_map
        .is_entity_ticking_full_chunk_loaded(ChunkPos::new(ahead.x() >> 4, ahead.z() >> 4))
}
