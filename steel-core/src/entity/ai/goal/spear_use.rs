//! Vanilla `SpearUseGoal`: a monster with a spear charges its target, holds
//! the spear down for the weapon's damage window, then backs off to charge
//! again.

use glam::DVec3;
use steel_registry::data_components::components::KineticWeaponCondition;
use steel_registry::data_components::vanilla_components::KINETIC_WEAPON;
use steel_utils::types::InteractionHand;

use super::random_pos::land_random_pos_away_between;
use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};
use crate::entity::ai::navigation::PathNavigation;
use crate::entity::{Mob, PathfinderMob, SharedEntity};
use crate::inventory::equipment::EquipmentSlot;

const MIN_REPOSITION_DISTANCE: f64 = 6.0;
const MAX_REPOSITION_DISTANCE: f64 = 7.0;
const MIN_COOLDOWN_DISTANCE: f64 = 9.0;
const MAX_COOLDOWN_DISTANCE: f64 = 11.0;
const AWAY_VERTICAL_DISTANCE: i32 = 7;
const LOOK_MAX_ROTATION: f32 = 30.0;

/// Vanilla `SpearUseGoal.MAX_FLEEING_TIME`.
const fn max_fleeing_time() -> i32 {
    reduced_tick_delay(100)
}

/// Vanilla `SpearUseGoal.SpearUseState`.
#[derive(Debug, Clone, Copy)]
struct SpearUseState {
    engage_time: i32,
    fleeing_time: i32,
    away_pos: Option<DVec3>,
    done: bool,
}

impl SpearUseState {
    const fn new() -> Self {
        Self {
            engage_time: -1,
            fleeing_time: -1,
            away_pos: None,
            done: false,
        }
    }

    const fn not_engaged_yet(&self) -> bool {
        self.engage_time < 0
    }

    /// Vanilla `tickAndCheckEngagement`: whether the spear's damage window
    /// just ran out.
    const fn tick_and_check_engagement(&mut self) -> bool {
        if self.engage_time > 0 {
            self.engage_time -= 1;
            if self.engage_time == 0 {
                return true;
            }
        }
        false
    }

    /// Vanilla `tickAndCheckFleeing`: whether the retreat ran too long.
    const fn tick_and_check_fleeing(&mut self) -> bool {
        if self.fleeing_time > 0 {
            self.fleeing_time += 1;
            if self.fleeing_time > max_fleeing_time() {
                self.done = true;
                return true;
            }
        }
        false
    }
}

/// Vanilla `SpearUseGoal`.
pub(crate) struct SpearUseGoal {
    state: Option<SpearUseState>,
    speed_modifier_when_charging: f64,
    speed_modifier_when_repositioning: f64,
    approach_distance_sq: f64,
    target_in_range_radius_sq: f64,
}

impl SpearUseGoal {
    #[must_use]
    pub(crate) fn new(
        speed_modifier_when_charging: f64,
        speed_modifier_when_repositioning: f64,
        approach_distance: f32,
        target_in_range_radius: f32,
    ) -> Self {
        Self {
            state: None,
            speed_modifier_when_charging,
            speed_modifier_when_repositioning,
            approach_distance_sq: f64::from(approach_distance * approach_distance),
            target_in_range_radius_sq: f64::from(target_in_range_radius * target_in_range_radius),
        }
    }

    fn able_to_attack(mob: &dyn PathfinderMob) -> bool {
        mob.target().is_some() && holds_kinetic_weapon(mob)
    }

    /// Vanilla `getKineticWeaponUseDuration`.
    fn kinetic_weapon_use_duration(mob: &dyn PathfinderMob) -> i32 {
        reduced_tick_delay(kinetic_weapon_damage_use_duration(mob))
    }
}

/// Whether the main hand holds a kinetic weapon such as a spear.
pub(crate) fn holds_kinetic_weapon(mob: &dyn PathfinderMob) -> bool {
    let mut has_spear = false;
    mob.with_equipment_slot(EquipmentSlot::MainHand, &mut |stack| {
        has_spear = stack.has(KINETIC_WEAPON);
    });
    has_spear
}

/// Vanilla `KineticWeapon.computeDamageUseDuration` of the main hand item, or
/// 0 without a kinetic weapon.
pub(crate) fn kinetic_weapon_damage_use_duration(mob: &dyn PathfinderMob) -> i32 {
    let mut duration = 0;
    mob.with_equipment_slot(EquipmentSlot::MainHand, &mut |stack| {
        duration = stack.get(KINETIC_WEAPON).map_or(0, |weapon| {
            weapon.delay_ticks()
                + weapon
                    .damage_conditions()
                    .map_or(0, KineticWeaponCondition::max_duration_ticks)
        });
    });
    duration
}

/// Vanilla `Mob.chargeSpeedModifier` of the root vehicle when it is a mob,
/// else 1.
pub(crate) fn mount_charge_speed_modifier(mob: &dyn PathfinderMob) -> f64 {
    let speed_modifier = mob
        .root_vehicle()
        .and_then(|mount| mount.as_mob().map(Mob::charge_speed_modifier))
        .unwrap_or(1.0);
    f64::from(speed_modifier)
}

/// Vanilla `Mob.getNavigation`: a rider steers with its controlled vehicle's
/// navigation.
pub(crate) fn with_navigation<R>(
    mob: &dyn PathfinderMob,
    f: impl FnOnce(&mut PathNavigation) -> R,
) -> R {
    if let Some(vehicle) = mob.controlled_pathfinder_vehicle()
        && let Some(vehicle) = vehicle.as_mob()
    {
        return f(&mut vehicle.mob_base().navigation().lock());
    }
    f(&mut mob.mob_base().navigation().lock())
}

impl Goal for SpearUseGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        Self::able_to_attack(mob) && !mob.is_using_item()
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.state.is_some_and(|state| !state.done) && Self::able_to_attack(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        mob.set_aggressive(true);
        self.state = Some(SpearUseState::new());
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        with_navigation(mob, PathNavigation::stop);
        mob.set_aggressive(false);
        self.state = None;
        mob.stop_using_held_item();
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let (Some(mut state), Some(target)) = (self.state, mob.target()) else {
            return;
        };
        self.state = Some(self.tick_state(mob, &target, &mut state));
    }
}

impl SpearUseGoal {
    /// Vanilla `SpearUseGoal.tick` with the state copied out.
    fn tick_state(
        &self,
        mob: &dyn PathfinderMob,
        target: &SharedEntity,
        state: &mut SpearUseState,
    ) -> SpearUseState {
        let target_position = target.position();
        let target_dist_sqr = mob.position().distance_squared(target_position);
        let speed_modifier = mount_charge_speed_modifier(mob);
        let mount_distance = if mob.is_passenger() { 2.0 } else { 0.0 };
        mob.mob_look_at(target.as_ref(), LOOK_MAX_ROTATION, LOOK_MAX_ROTATION);
        mob.mob_base().controls().lock().look_control.set_look_at(
            DVec3::new(target_position.x, target.get_eye_y(), target_position.z),
            LOOK_MAX_ROTATION,
            LOOK_MAX_ROTATION,
        );
        if state.not_engaged_yet() {
            if target_dist_sqr > self.approach_distance_sq {
                let _ = mob.move_to_entity(
                    target.as_ref(),
                    speed_modifier * self.speed_modifier_when_repositioning,
                );
                return *state;
            }
            state.engage_time = Self::kinetic_weapon_use_duration(mob);
            mob.start_using_held_item(InteractionHand::MainHand);
        }

        if state.tick_and_check_engagement() {
            mob.stop_using_held_item();
            let distance = target_dist_sqr.sqrt();
            state.away_pos = land_random_pos_away_between(
                mob,
                (MIN_COOLDOWN_DISTANCE + mount_distance - distance).max(0.0),
                (MAX_COOLDOWN_DISTANCE + mount_distance - distance).max(1.0),
                AWAY_VERTICAL_DISTANCE,
                target_position,
            );
            state.fleeing_time = 1;
        }

        if state.tick_and_check_fleeing() {
            return *state;
        }
        if let Some(away_pos) = state.away_pos {
            let _ = mob.move_to_pos(
                away_pos,
                speed_modifier * self.speed_modifier_when_repositioning,
            );
            if with_navigation(mob, |navigation| navigation.is_done()) {
                if state.fleeing_time > 0 {
                    state.done = true;
                    return *state;
                }
                state.away_pos = None;
            }
        } else {
            let _ = mob.move_to_entity(
                target.as_ref(),
                speed_modifier * self.speed_modifier_when_charging,
            );
            if target_dist_sqr < self.target_in_range_radius_sq
                || with_navigation(mob, |navigation| navigation.is_done())
            {
                let distance = target_dist_sqr.sqrt();
                state.away_pos = land_random_pos_away_between(
                    mob,
                    MIN_REPOSITION_DISTANCE + mount_distance - distance,
                    MAX_REPOSITION_DISTANCE + mount_distance - distance,
                    AWAY_VERTICAL_DISTANCE,
                    target_position,
                );
            }
        }
        *state
    }
}
