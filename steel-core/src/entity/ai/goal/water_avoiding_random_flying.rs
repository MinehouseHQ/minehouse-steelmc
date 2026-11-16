//! Vanilla `WaterAvoidingRandomFlyingGoal.getPosition`, the wander target of
//! flying mobs. Their wander goals pair `RandomStrollGoal` with this position.

use std::f32::consts::FRAC_PI_2;

use glam::DVec3;

use super::RandomStrollGoal;
use super::random_pos::{air_and_water_random_pos, hover_random_pos};
use super::selector::{Goal, GoalControls};
use crate::entity::PathfinderMob;

/// Vanilla `xzDist`.
const HORIZONTAL_DIST: i32 = 8;

/// Vanilla `WaterAvoidingRandomFlyingGoal.getPosition`: a spot ahead of the
/// mob hovering one to three blocks above the ground, else any open spot.
pub(crate) fn random_flying_pos(mob: &(impl PathfinderMob + ?Sized)) -> Option<DVec3> {
    let wander_direction = mob.look_angle();
    hover_random_pos(
        mob,
        HORIZONTAL_DIST,
        7,
        wander_direction.x,
        wander_direction.z,
        FRAC_PI_2,
        3,
        1,
    )
    .or_else(|| {
        air_and_water_random_pos(
            mob,
            HORIZONTAL_DIST,
            4,
            -2,
            wander_direction.x,
            wander_direction.z,
            f64::from(FRAC_PI_2),
        )
    })
}

/// Vanilla `WaterAvoidingRandomFlyingGoal`: a random stroll through the air
/// to [`random_flying_pos`].
pub(crate) struct WaterAvoidingRandomFlyingGoal {
    stroll: RandomStrollGoal,
}

impl WaterAvoidingRandomFlyingGoal {
    #[must_use]
    pub(crate) const fn new(speed_modifier: f64) -> Self {
        Self {
            stroll: RandomStrollGoal::new(speed_modifier),
        }
    }
}

fn flying_pos(mob: &dyn PathfinderMob) -> Option<DVec3> {
    random_flying_pos(mob)
}

impl Goal for WaterAvoidingRandomFlyingGoal {
    fn controls(&self) -> GoalControls {
        self.stroll.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.stroll.can_use_with_position(mob, flying_pos)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.stroll.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.stroll.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.stroll.stop(mob);
    }
}
