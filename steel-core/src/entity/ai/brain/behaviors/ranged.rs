//! Vanilla `BackUpIfTooClose` and `CrossbowAttack`.

use std::sync::Arc;

use steel_registry::vanilla_items;

use super::fight::is_within_attack_range;
use super::utils::{entity_is_visible, look_at_entity};
use crate::behavior::items::crossbow::get_charge_duration;
use crate::entity::ai::brain::behavior::{Behavior, OneShot};
use crate::entity::ai::brain::memory::MemoryRequirement;
use crate::entity::ai::brain::memory_types::{
    ATTACK_TARGET, LOOK_TARGET, NEAREST_VISIBLE_LIVING_ENTITIES, WALK_TARGET,
};
use crate::entity::ai::brain::{BehaviorControl, MemoryStatus};
use crate::entity::ai::control::rotate_if_necessary;
use crate::entity::ai::goal::weapon_holding_hand;
use crate::entity::{LivingEntity, Mob, SharedEntity};
use crate::world::World;

fn attack_target_of(body: &(impl LivingEntity + ?Sized)) -> Option<SharedEntity> {
    body.brain_memories()?.lock().get(&ATTACK_TARGET)
}

/// Vanilla `BackUpIfTooClose.create`: strafes backwards at `strafe_speed`
/// while the visible attack target is closer than `too_close_distance`.
pub fn back_up_if_too_close<E: Mob + ?Sized>(
    too_close_distance: i32,
    strafe_speed: f32,
) -> impl BehaviorControl<E> {
    OneShot::new(
        vec![
            MemoryRequirement::new(&WALK_TARGET, MemoryStatus::ValueAbsent),
            MemoryRequirement::new(&LOOK_TARGET, MemoryStatus::Registered),
            MemoryRequirement::new(&ATTACK_TARGET, MemoryStatus::ValuePresent),
            MemoryRequirement::new(&NEAREST_VISIBLE_LIVING_ENTITIES, MemoryStatus::ValuePresent),
        ],
        move |_world: &Arc<World>, body: &E, _time: i64| {
            let Some(memories) = body.brain_memories() else {
                return false;
            };
            if memories.lock().has_value(&WALK_TARGET) {
                return false;
            }
            let Some(target) = attack_target_of(body) else {
                return false;
            };
            let distance = f64::from(too_close_distance);
            if target.position().distance_squared(body.position()) >= distance * distance
                || !entity_is_visible(body, &target)
            {
                return false;
            }
            look_at_entity(body, &target);
            body.mob_base()
                .controls()
                .lock()
                .move_control
                .strafe(-strafe_speed, 0.0);
            let (y_rot, x_rot) = body.rotation();
            body.set_rotation((rotate_if_necessary(y_rot, body.y_head_rot(), 0.0), x_rot));
            true
        },
    )
}

/// Vanilla `CrossbowAttack.TIMEOUT`.
const CROSSBOW_ATTACK_TIMEOUT: i32 = 1200;

/// Vanilla `CrossbowAttack.CrossbowState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CrossbowState {
    Uncharged,
    Charging,
    Charged,
    ReadyToAttack,
}

/// Vanilla `CrossbowAttack`: charges the held crossbow, waits 20 to 39
/// ticks, then shoots the visible attack target in range.
pub struct CrossbowAttack {
    attack_delay: i32,
    crossbow_state: CrossbowState,
    entry_condition: [MemoryRequirement; 2],
}

impl CrossbowAttack {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            attack_delay: 0,
            crossbow_state: CrossbowState::Uncharged,
            entry_condition: [
                MemoryRequirement::new(&LOOK_TARGET, MemoryStatus::Registered),
                MemoryRequirement::new(&ATTACK_TARGET, MemoryStatus::ValuePresent),
            ],
        }
    }

    fn is_holding_crossbow(body: &(impl Mob + ?Sized)) -> bool {
        body.is_holding(&mut |stack| stack.is(&vanilla_items::CROSSBOW))
    }

    fn crossbow_attack<E: Mob>(&mut self, world: &World, body: &E, target: &SharedEntity) {
        let Some(crossbow_mob) = body.as_crossbow_attack_mob() else {
            return;
        };
        match self.crossbow_state {
            CrossbowState::Uncharged => {
                crossbow_mob.start_using_held_item(weapon_holding_hand(
                    crossbow_mob,
                    &vanilla_items::CROSSBOW,
                ));
                self.crossbow_state = CrossbowState::Charging;
                crossbow_mob.set_charging_crossbow(true);
            }
            CrossbowState::Charging => {
                if !crossbow_mob.living_base().is_using_item() {
                    self.crossbow_state = CrossbowState::Uncharged;
                }
                let pull_time = crossbow_mob.ticks_using_item();
                if pull_time >= get_charge_duration(&crossbow_mob.use_held_item()) {
                    if let Some(entity) = world.get_entity_by_id(body.id()) {
                        crossbow_mob.release_using_held_item(&entity);
                    }
                    self.crossbow_state = CrossbowState::Charged;
                    self.attack_delay = 20 + rand::random_range(0..20);
                    crossbow_mob.set_charging_crossbow(false);
                }
            }
            CrossbowState::Charged => {
                self.attack_delay -= 1;
                if self.attack_delay == 0 {
                    self.crossbow_state = CrossbowState::ReadyToAttack;
                }
            }
            CrossbowState::ReadyToAttack => {
                if let Some(entity) = world.get_entity_by_id(body.id()) {
                    crossbow_mob.perform_ranged_attack(&entity, target, 1.0);
                }
                self.crossbow_state = CrossbowState::Uncharged;
            }
        }
    }
}

impl Default for CrossbowAttack {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: Mob> Behavior<E> for CrossbowAttack {
    fn entry_condition(&self) -> &[MemoryRequirement] {
        &self.entry_condition
    }

    fn duration(&self) -> (i32, i32) {
        (CROSSBOW_ATTACK_TIMEOUT, CROSSBOW_ATTACK_TIMEOUT)
    }

    fn check_extra_start_conditions(&mut self, _world: &Arc<World>, body: &E) -> bool {
        let Some(target) = attack_target_of(body) else {
            return false;
        };
        let Some(target_living) = target.as_living_entity() else {
            return false;
        };
        Self::is_holding_crossbow(body)
            && entity_is_visible(body, &target)
            && is_within_attack_range(body, target_living, 0)
    }

    fn can_still_use(&mut self, world: &Arc<World>, body: &E, _time: i64) -> bool {
        self.check_extra_start_conditions(world, body)
    }

    fn tick(&mut self, world: &Arc<World>, body: &E, _time: i64) {
        let Some(target) = attack_target_of(body) else {
            return;
        };
        look_at_entity(body, &target);
        self.crossbow_attack(world, body, &target);
    }

    fn stop(&mut self, _world: &Arc<World>, body: &E, _time: i64) {
        if body.living_base().is_using_item() {
            body.stop_using_held_item();
        }
        if Self::is_holding_crossbow(body)
            && let Some(crossbow_mob) = body.as_crossbow_attack_mob()
        {
            // VANILLA: `getUseItem().set(CHARGED_PROJECTILES, EMPTY)` runs on
            // the use item, which `stopUsingItem` (or the release when the
            // charge finished) already reset to `ItemStack.EMPTY`, so the
            // crossbow keeps whatever it had loaded.
            crossbow_mob.set_charging_crossbow(false);
        }
    }
}
