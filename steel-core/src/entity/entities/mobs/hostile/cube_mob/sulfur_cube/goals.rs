//! Vanilla `SulfurCubeTemptGoal` and `SulfurCubeSearchForItemsGoal`.

use steel_registry::item_stack::ItemStack;

use super::SulfurCubeEntity;
use crate::entity::ai::goal::{Goal, GoalControls, TemptGoal};
use crate::entity::entities::ItemEntity;
use steel_utils::Downcast as _;

use super::super::AbstractCubeMob as _;
use crate::entity::{Entity, LivingEntity, Mob as _, PathfinderMob, SharedEntity};

/// Vanilla search box: the cube's bounding box grown 8 blocks each way.
const ITEM_SEARCH_RANGE: f64 = 8.0;
/// Vanilla `lookAt(target, 10, 10)`.
const LOOK_MAX_ROT: f32 = 10.0;

/// Vanilla `SulfurCubeTemptGoal(this, 1.0, ..., false, 1.0)`: babies follow
/// slime balls, adults follow blocks they can swallow.
pub(super) fn tempt_goal() -> TemptGoal {
    TemptGoal::for_cube_mobs(1.0, is_tempting, false, 1.0)
}

fn is_tempting(mob: &dyn PathfinderMob, stack: &ItemStack) -> bool {
    if LivingEntity::is_baby(mob) {
        SulfurCubeEntity::is_food(stack)
    } else {
        SulfurCubeEntity::is_swallowable_item(stack)
    }
}

/// Vanilla `SulfurCube.ALLOWED_ITEMS`.
fn is_allowed_item(item: &ItemEntity) -> bool {
    !item.has_pickup_delay()
        && Entity::is_alive(item)
        && SulfurCubeEntity::is_swallowable_item(&item.get_item())
}

/// Vanilla `SulfurCubeSearchForItemsGoal`: an adult cube turns and hops toward
/// the nearest block lying around.
pub(super) struct SearchForItemsGoal {
    target_item: Option<SharedEntity>,
}

impl SearchForItemsGoal {
    pub(super) const fn new() -> Self {
        Self { target_item: None }
    }
}

impl Goal for SearchForItemsGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(cube) = mob.downcast_ref::<SulfurCubeEntity>() else {
            return false;
        };
        if LivingEntity::is_baby(cube) || cube.state.lock().pickup_timer > 0 {
            return false;
        }
        let Some(world) = cube.level() else {
            return false;
        };
        let area = cube.bounding_box().inflate(ITEM_SEARCH_RANGE);
        self.target_item =
            world.nearest_entity_in_aabb_matching(&area, cube.position(), |entity| {
                entity
                    .downcast_ref::<ItemEntity>()
                    .is_some_and(is_allowed_item)
            });
        self.target_item.is_some()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(cube) = mob.downcast_ref::<SulfurCubeEntity>() else {
            return;
        };
        if let Some(target) = &self.target_item {
            cube.mob_look_at(target.as_ref(), LOOK_MAX_ROT, LOOK_MAX_ROT);
        }
        cube.set_move_direction(cube.rotation().0, true);
    }
}
