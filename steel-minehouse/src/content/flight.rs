//! A fast cart that runs out of rails flies on, as in Mojang's `minecart_improvements`
//! experiment. A port of the Fabric `MinecartFlight` (`MineHouse` Metro 1.1.0); on when
//! `[minecarts] flight = true` in `tweaks.toml`.
//!
//! The server limits a cart off the rails to 0.4 blocks a tick at once, so a cart at 80
//! blocks a second just drops at the end of its track. A flying cart keeps its speed
//! instead and moves by the server's own rules off the rails: gravity, air drag 0.95 a tick
//! and, on the ground, half its speed a tick, until it is as slow as a server cart can be
//! and the server's limit takes over again. Off the top of a ramp it is thrown up as well:
//! in the experiment a cart leaves an ascending rail along the slope, which at 80 blocks a
//! second throws it some 50 blocks up, so the climb is capped at [`MAX_LAUNCH_UP`] (about
//! 5 blocks above the ramp). Only carts faster than any server cart ([`LAUNCH_SPEED`]) fly,
//! so ordinary carts, farms and sorters do not change.

use std::sync::Arc;

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_core::{entity::Entity, world::World};
use steel_registry::blocks::properties::RailShape;
use steel_utils::locks::SyncMutex;
use uuid::Uuid;

use super::end_lock;

/// Blocks a tick: slower than this, a cart off the rails is the server's (limit 0.4).
const LAUNCH_SPEED: f64 = 0.5;
/// Blocks a tick up at most, off the top of a ramp.
const MAX_LAUNCH_UP: f64 = 0.8;
/// Minecart gravity, a tick (already applied this tick when a flight starts).
const GRAVITY: f64 = 0.04;
/// Carts not seen on or off the track for this many ticks are forgotten.
const FORGET_AFTER: i64 = 1200;

#[derive(Clone, Copy)]
struct Cart {
    /// The last rail the cart moved along.
    shape: RailShape,
    /// The game time it did.
    tick: i64,
    flying: bool,
}

static CARTS: SyncMutex<Option<FxHashMap<Uuid, Cart>>> = SyncMutex::new(None);

/// The cart moves along a rail of `shape` (each substep of a fast cart).
pub(super) fn on_track(cart: &dyn Entity, world: &World, shape: RailShape) {
    if !end_lock::minecart_flight() {
        return;
    }
    let now = world.game_time();
    let mut carts = CARTS.lock();
    let carts = carts.get_or_insert_default();
    if now % FORGET_AFTER == 0 {
        carts.retain(|_, state| (now - state.tick).abs() < FORGET_AFTER);
    }
    carts.insert(
        cart.uuid(),
        Cart {
            shape,
            tick: now,
            flying: false,
        },
    );
}

/// The cart is off the track: the speed limit for a flying cart (and a launch for one
/// that has just left the rails fast enough), or `None` for the server's own.
pub(super) fn off_track(cart: &dyn Entity, world: &Arc<World>) -> Option<f64> {
    let mut carts = CARTS.lock();
    let state = carts.as_mut()?.get_mut(&cart.uuid())?;
    if !end_lock::minecart_flight() || cart.is_in_water() || cart.is_in_lava() {
        state.flying = false;
        return None;
    }
    let now = world.game_time();
    let velocity = cart.velocity();
    if state.flying {
        if horizontal_length(velocity) < LAUNCH_SPEED * 0.8 {
            // As slow as a server cart: the server's from here on.
            state.flying = false;
            return None;
        }
        state.tick = now;
    } else {
        // Only right off the rails (the last rail this tick or the one before).
        if now - state.tick > 1 {
            return None;
        }
        let scale = if cart.is_vehicle() { 0.75 } else { 1.0 };
        let launched = launch(state.shape, velocity, scale)?;
        cart.set_velocity(launched);
        state.flying = true;
        state.tick = now;
    }
    Some(end_lock::minecart_top_speed() / 20.0)
}

/// The velocity a cart leaving `shape` with `velocity` flies off with, or `None` when it
/// is too slow to fly. On rails the server moves a ridden cart `scale` (0.75) of its speed;
/// off them, all of it.
fn launch(shape: RailShape, velocity: DVec3, scale: f64) -> Option<DVec3> {
    let raw = horizontal_length(velocity);
    let speed = raw * scale;
    if speed < LAUNCH_SPEED {
        return None;
    }
    let up = if climbing(shape, velocity) {
        speed.min(MAX_LAUNCH_UP)
    } else {
        0.0
    };
    // Gravity for this tick is in already; whatever else the rails left in y is no jump.
    Some(DVec3::new(
        velocity.x / raw * speed,
        up - GRAVITY,
        velocity.z / raw * speed,
    ))
}

/// Was the cart going up this rail (towards its raised end)?
const fn climbing(shape: RailShape, velocity: DVec3) -> bool {
    match shape {
        RailShape::AscendingEast => velocity.x > 0.0,
        RailShape::AscendingWest => velocity.x < 0.0,
        RailShape::AscendingNorth => velocity.z < 0.0,
        RailShape::AscendingSouth => velocity.z > 0.0,
        _ => false,
    }
}

fn horizontal_length(vector: DVec3) -> f64 {
    vector.x.hypot(vector.z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_carts_do_not_fly() {
        assert!(launch(RailShape::EastWest, DVec3::new(0.4, 0.0, 0.0), 1.0).is_none());
        // A ridden cart moves 0.75 of its speed: 0.6 is 0.45 a tick.
        assert!(launch(RailShape::EastWest, DVec3::new(0.6, 0.0, 0.0), 0.75).is_none());
    }

    #[test]
    fn flat_end_flies_level() {
        let v = launch(RailShape::EastWest, DVec3::new(4.0, -0.3, 0.0), 1.0).expect("flies");
        assert!((v.x - 4.0).abs() < 1e-9);
        assert!((v.y + GRAVITY).abs() < 1e-9);
    }

    #[test]
    fn ramp_throws_up_capped() {
        let v = launch(RailShape::AscendingEast, DVec3::new(4.0, 0.0, 0.0), 1.0).expect("flies");
        assert!((v.y - (MAX_LAUNCH_UP - GRAVITY)).abs() < 1e-9);
        let v = launch(RailShape::AscendingNorth, DVec3::new(0.0, 0.0, -0.6), 1.0).expect("flies");
        assert!((v.y - (0.6 - GRAVITY)).abs() < 1e-9);
        // Going down a ramp is no jump.
        let v = launch(RailShape::AscendingEast, DVec3::new(-4.0, 0.0, 0.0), 1.0).expect("flies");
        assert!(v.y < 0.0);
    }

    #[test]
    fn ridden_cart_flies_with_what_it_moved() {
        let v = launch(RailShape::NorthSouth, DVec3::new(0.0, 0.0, 4.0), 0.75).expect("flies");
        assert!((v.z - 3.0).abs() < 1e-9);
    }
}
