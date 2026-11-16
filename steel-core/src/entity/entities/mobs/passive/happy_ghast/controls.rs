//! The grown happy ghast's own float goal and look control.

use steel_math::{RAD_TO_DEG, wrap_degrees_90};
use steel_utils::Downcast as _;

use super::HappyGhastEntity;
use crate::entity::ai::goal::{FloatGoal, Goal, GoalControls};
use crate::entity::entities::mobs::hostile::ghast::flight::face_movement_direction;
use crate::entity::{Entity, LivingEntity, Mob, MobBase, PathfinderMob, SharedEntity};

/// Vanilla `HappyGhastFloatGoal`: floats up out of fluids unless it holds
/// still for a rider.
pub(super) struct HappyGhastFloatGoal {
    float: FloatGoal,
}

impl HappyGhastFloatGoal {
    pub(super) fn new(mob_base: &MobBase) -> Self {
        Self {
            float: FloatGoal::new(mob_base),
        }
    }
}

impl Goal for HappyGhastFloatGoal {
    fn controls(&self) -> GoalControls {
        self.float.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let still = mob
            .downcast_ref::<HappyGhastEntity>()
            .is_some_and(HappyGhastEntity::is_on_still_timeout);
        !still && self.float.can_use(mob)
    }

    fn requires_update_every_tick(&self) -> bool {
        self.float.requires_update_every_tick()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.float.tick(mob, entity);
    }
}

/// Vanilla `HappyGhastLookControl.tick`: snaps to a right angle while it
/// holds still, faces what it was told to look at, or else faces where it
/// flies.
pub(super) fn tick_happy_ghast_look_control(ghast: &HappyGhastEntity) {
    if ghast.is_on_still_timeout() {
        let (y_rot, x_rot) = ghast.rotation();
        let y_rot = y_rot - wrap_degrees_90(y_rot);
        ghast.set_rotation((y_rot, x_rot));
        ghast.set_y_head_rot(y_rot);
        return;
    }
    let looking = {
        let mut controls = ghast.mob_base().controls().lock();
        controls
            .look_control
            .tick_cooldown()
            .then(|| controls.look_control.wanted_position())
    };
    if let Some(wanted) = looking {
        let position = ghast.position();
        let y_rot = -((wanted.x - position.x).atan2(wanted.z - position.z) as f32) * RAD_TO_DEG;
        ghast.set_rotation((y_rot, ghast.rotation().1));
        ghast.set_y_body_rot(y_rot);
        ghast.set_y_head_rot(y_rot);
    } else {
        face_movement_direction(ghast);
    }
}
