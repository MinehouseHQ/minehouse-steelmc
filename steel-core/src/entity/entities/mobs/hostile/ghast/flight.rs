//! Flight shared by the ghast and the happy ghast: vanilla `GhastMoveControl`,
//! `Ghast.faceMovementDirection`, `GhastLookGoal` and `RandomFloatAroundGoal`.

use crate::chunk::heightmap::HeightmapType;
use std::f32::consts::PI;
use std::sync::Arc;

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::fluid::FluidStateExt as _;
use steel_registry::vanilla_attributes;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_utils::{BlockPos, Direction};

use crate::behavior::{BLOCK_BEHAVIORS, BlockCollisionContext};
use crate::entity::ai::control::MoveControlOperation;
use crate::entity::ai::goal::{Goal, GoalControls};
use crate::entity::block_effects::{
    collided_with_shape_moving_from, for_each_block_intersected_between,
};
use crate::entity::projectile::normalize_or_zero;
use crate::entity::{Mob, PathfinderMob, SharedEntity, collided_with_fluid};
use crate::fluid::get_fluid_state;
use crate::world::{LevelReader as _, World};

/// Squared distance within which a ghast turns to face its target.
const FACE_TARGET_DISTANCE_SQR: f64 = 4096.0;
const MAX_FLY_TO_ATTEMPTS: i32 = 64;
const FLY_TO_RANGE: f32 = 16.0;

/// Vanilla `GhastMoveControl` state.
#[derive(Debug, Default)]
pub(crate) struct GhastMoveState {
    float_duration: i32,
}

/// Vanilla `GhastMoveControl.tick` for a non-careful ghast: every few ticks
/// push toward the wanted position, or give up when the way is blocked.
pub(crate) fn tick_ghast_move_control(mob: &dyn Mob, state: &mut GhastMoveState) {
    tick_ghast_move_control_with(mob, state, false, false);
}

/// Vanilla `GhastMoveControl.tick`. A `careful` ghast (the happy ghast) keeps
/// a block away from what it avoids and from fluids it is not already in;
/// `should_be_stopped` holds it in place.
pub(crate) fn tick_ghast_move_control_with(
    mob: &dyn Mob,
    state: &mut GhastMoveState,
    careful: bool,
    should_be_stopped: bool,
) {
    if should_be_stopped {
        mob.mob_base().controls().lock().move_control.set_wait();
        mob.stop_in_place();
    }
    let (operation, wanted) = {
        let controls = mob.mob_base().controls().lock();
        (
            controls.move_control.operation(),
            controls.move_control.wanted_position(),
        )
    };
    if operation != MoveControlOperation::MoveTo {
        return;
    }
    let float_duration = state.float_duration;
    state.float_duration -= 1;
    if float_duration > 0 {
        return;
    }
    state.float_duration += rand::random_range(0..5) + 2;
    let travel = wanted - mob.position();
    if can_reach(mob, travel, careful) {
        let flying_speed = mob
            .attributes()
            .lock()
            .required_value(vanilla_attributes::FLYING_SPEED);
        mob.set_velocity(mob.velocity() + normalize_or_zero(travel) * (flying_speed * 5.0 / 3.0));
    } else {
        mob.mob_base().controls().lock().move_control.set_wait();
    }
}

/// Vanilla `GhastMoveControl.canReach`.
fn can_reach(mob: &dyn Mob, travel: DVec3, careful: bool) -> bool {
    let Some(world) = mob.level() else {
        return false;
    };
    let aabb = mob.bounding_box();
    let aabb_at_destination = aabb.translate(travel);
    if careful {
        let around = aabb_at_destination.inflate(1.0);
        for x in floor(around.min_x())..=floor(around.max_x()) {
            for y in floor(around.min_y())..=floor(around.max_y()) {
                for z in floor(around.min_z())..=floor(around.max_z()) {
                    let pos = BlockPos::new(x, y, z);
                    let traversal = Traversal::coarse(careful);
                    if !block_traversal_possible(&world, mob, pos, traversal) {
                        return false;
                    }
                }
            }
        }
    }
    let start = mob.position();
    let end = start + travel;
    let traversal = Traversal {
        path: Some((start, end)),
        through_water: mob.is_in_water(),
        through_lava: mob.is_in_lava(),
        careful,
    };
    for_each_block_intersected_between(start, end, aabb_at_destination, |pos, _| {
        aabb.intersects_block(pos) || block_traversal_possible(&world, mob, pos, traversal)
    })
    .is_some()
}

/// Vanilla `Mth.floor` of a coordinate.
const fn floor(value: f64) -> i32 {
    value.floor() as i32
}

/// The arguments of vanilla `GhastMoveControl.blockTraversalPossible`.
#[derive(Clone, Copy)]
struct Traversal {
    /// The move, for precise collisions; without it a block must have no
    /// collision at all.
    path: Option<(DVec3, DVec3)>,
    through_water: bool,
    through_lava: bool,
    careful: bool,
}

impl Traversal {
    /// The coarse check vanilla runs around the destination.
    const fn coarse(careful: bool) -> Self {
        Self {
            path: None,
            through_water: false,
            through_lava: false,
            careful,
        }
    }
}

/// Vanilla `GhastMoveControl.blockTraversalPossible`.
fn block_traversal_possible(
    world: &Arc<World>,
    mob: &dyn Mob,
    pos: BlockPos,
    traversal: Traversal,
) -> bool {
    let state = world.get_block_state(pos);
    if state.is_air() {
        return true;
    }
    let shape = BLOCK_BEHAVIORS
        .get_behavior(state.get_block())
        .get_collision_shape(state, world.as_ref(), pos, BlockCollisionContext::empty());
    let path_no_collisions = match traversal.path {
        Some((start, end)) => !collided_with_shape_moving_from(
            mob.make_bounding_box_at(start),
            start,
            end,
            pos,
            shape,
        ),
        None => shape.is_empty(),
    };
    if !traversal.careful {
        return path_no_collisions;
    }
    if state.get_block().has_tag(&BlockTag::HAPPY_GHAST_AVOIDS) {
        return false;
    }
    let fluid_state = get_fluid_state(world, pos);
    let touches_fluid = !fluid_state.is_empty()
        && traversal.path.is_none_or(|(start, end)| {
            collided_with_fluid(world, fluid_state, pos, start, end, mob)
        });
    if touches_fluid {
        if fluid_state.is_water() {
            return traversal.through_water;
        }
        if fluid_state.is_lava() {
            return traversal.through_lava;
        }
    }
    path_no_collisions
}

/// Vanilla `Ghast.faceMovementDirection`: face the flight direction, or the
/// target when it is within 64 blocks.
pub(crate) fn face_movement_direction(mob: &dyn Mob) {
    let y_rot = if let Some(target) = mob.target() {
        if target.distance_to_sqr(mob.position()) >= FACE_TARGET_DISTANCE_SQR {
            return;
        }
        let target_position = target.position();
        let position = mob.position();
        heading(
            target_position.x - position.x,
            target_position.z - position.z,
        )
    } else {
        let movement = mob.velocity();
        heading(movement.x, movement.z)
    };
    mob.set_rotation((y_rot, mob.rotation().1));
    mob.set_y_body_rot(y_rot);
}

/// Vanilla `-((float)Mth.atan2(x, z)) * (180.0F / (float)Math.PI)`.
fn heading(x: f64, z: f64) -> f32 {
    -(x.atan2(z) as f32) * (180.0_f32 / PI)
}

/// Vanilla `Ghast.GhastLookGoal`.
pub(crate) struct GhastLookGoal;

impl Goal for GhastLookGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::LOOK
    }

    fn can_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        true
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        face_movement_direction(mob);
    }
}

/// Vanilla `Ghast.RandomFloatAroundGoal`: pick a new spot when idle, close
/// to it, or more than 60 blocks away.
pub(crate) struct RandomFloatAroundGoal {
    distance_to_blocks: i32,
}

impl RandomFloatAroundGoal {
    /// A goal that accepts any spot (the hostile ghast).
    pub(crate) const fn new() -> Self {
        Self::with_distance_to_blocks(0)
    }

    /// A goal that prefers air spots near blocks (the happy ghast).
    pub(crate) const fn with_distance_to_blocks(distance_to_blocks: i32) -> Self {
        Self { distance_to_blocks }
    }
}

impl Goal for RandomFloatAroundGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let (operation, wanted) = {
            let controls = mob.mob_base().controls().lock();
            (
                controls.move_control.operation(),
                controls.move_control.wanted_position(),
            )
        };
        if operation != MoveControlOperation::MoveTo {
            return true;
        }
        let distance = (wanted - mob.position()).length_squared();
        !(1.0..=3600.0).contains(&distance)
    }

    fn can_continue_to_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let result = suitable_fly_to_position(mob, self.distance_to_blocks);
        mob.mob_base()
            .controls()
            .lock()
            .move_control
            .set_wanted_position(result, 1.0);
    }
}

/// Vanilla `RandomFloatAroundGoal.getSuitableFlyToPosition`.
pub(crate) fn suitable_fly_to_position(mob: &dyn Mob, distance_to_blocks: i32) -> DVec3 {
    let center = mob.position();
    let Some(world) = mob.level() else {
        return center;
    };
    let mut result = None;
    for _ in 0..MAX_FLY_TO_ATTEMPTS {
        result = random_position_with_restriction(mob, center);
        if let Some(candidate) = result
            && is_good_target(&world, candidate, distance_to_blocks)
        {
            return candidate;
        }
    }
    let mut result = result.unwrap_or_else(|| random_position(center));
    let pos = BlockPos::from(result);
    let height_y = world.height_at(HeightmapType::MotionBlocking, pos.x(), pos.z());
    if height_y < pos.y() && height_y > world.get_min_y() {
        result = DVec3::new(
            result.x,
            mob.position().y - (mob.position().y - result.y).abs(),
            result.z,
        );
    }
    result
}

fn is_good_target(world: &World, target: DVec3, distance_to_blocks: i32) -> bool {
    if distance_to_blocks <= 0 {
        return true;
    }
    let pos = BlockPos::from(target);
    if !world.get_block_state(pos).is_air() {
        return false;
    }
    Direction::ALL.iter().any(|&direction| {
        (1..distance_to_blocks).any(|distance| {
            !world
                .get_block_state(pos.relative_n(direction, distance))
                .is_air()
        })
    })
}

fn random_position(center: DVec3) -> DVec3 {
    let offset = || f64::from((rand::random::<f32>() * 2.0 - 1.0) * FLY_TO_RANGE);
    let x = center.x + offset();
    let y = center.y + offset();
    let z = center.z + offset();
    DVec3::new(x, y, z)
}

fn random_position_with_restriction(mob: &dyn Mob, center: DVec3) -> Option<DVec3> {
    let target = random_position(center);
    (!mob.has_home() || mob.is_within_home_vec(target)).then_some(target)
}
