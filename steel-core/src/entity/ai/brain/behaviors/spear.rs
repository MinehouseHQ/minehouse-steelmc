//! Vanilla `SpearApproach`, `SpearAttack` and `SpearRetreat`: the brain-driven
//! spear charge, split into walking up to the target, charging it with the
//! spear down and backing off to charge again.

use std::sync::Arc;

use glam::DVec3;
use steel_utils::types::InteractionHand;

use super::utils::look_at_entity;
use crate::entity::ai::brain::MemoryStatus;
use crate::entity::ai::brain::behavior::Behavior;
use crate::entity::ai::brain::memory::{MemoryModuleType, MemoryRequirement, MemoryValue};
use crate::entity::ai::brain::memory_types::{
    ATTACK_TARGET, SPEAR_CHARGE_POSITION, SPEAR_ENGAGE_TIME, SPEAR_FLEEING_POSITION,
    SPEAR_FLEEING_TIME, SPEAR_STATUS, SpearStatus,
};
use crate::entity::ai::goal::random_pos::land_random_pos_away_between;
use crate::entity::ai::goal::{
    holds_kinetic_weapon, kinetic_weapon_damage_use_duration, mount_charge_speed_modifier,
    with_navigation,
};
use crate::entity::ai::navigation::PathNavigation;
use crate::entity::{PathfinderMob, SharedEntity};
use crate::world::World;

/// Vanilla `SpearAttack.MIN_REPOSITION_DISTANCE`.
const MIN_REPOSITION_DISTANCE: f64 = 6.0;
/// Vanilla `SpearAttack.MAX_REPOSITION_DISTANCE`.
const MAX_REPOSITION_DISTANCE: f64 = 7.0;
/// Vanilla `SpearRetreat.MIN_COOLDOWN_DISTANCE`.
const MIN_COOLDOWN_DISTANCE: f64 = 9.0;
/// Vanilla `SpearRetreat.MAX_COOLDOWN_DISTANCE`.
const MAX_COOLDOWN_DISTANCE: f64 = 11.0;
/// Vanilla `SpearRetreat.MAX_FLEEING_TIME`.
const MAX_FLEEING_TIME: i32 = 100;
/// The vertical range of `LandRandomPos.getPosAway` in all three behaviors.
const AWAY_VERTICAL_DISTANCE: i32 = 7;

fn attack_target_of(body: &dyn PathfinderMob) -> Option<SharedEntity> {
    body.brain_memories()?.lock().get(&ATTACK_TARGET)
}

/// Vanilla `ableToAttack`: an attack target and a spear in the main hand.
fn able_to_attack(body: &dyn PathfinderMob) -> bool {
    attack_target_of(body).is_some() && holds_kinetic_weapon(body)
}

fn spear_status(body: &dyn PathfinderMob) -> SpearStatus {
    body.brain_memories()
        .and_then(|memories| memories.lock().get(&SPEAR_STATUS))
        .unwrap_or(SpearStatus::Approach)
}

fn set_memory<T: MemoryValue>(
    body: &dyn PathfinderMob,
    memory: &MemoryModuleType<T>,
    value: Option<T>,
) {
    if let Some(memories) = body.brain_memories() {
        memories.lock().set(memory, value);
    }
}

fn memory<T: MemoryValue>(body: &dyn PathfinderMob, memory: &MemoryModuleType<T>) -> Option<T> {
    body.brain_memories()?.lock().get(memory)
}

/// Vanilla `LandRandomPos.getPosAway` from `target` between `min` and `max`
/// blocks beyond the current distance, plus 2 when riding.
fn pos_away_from(
    body: &dyn PathfinderMob,
    target: &SharedEntity,
    min: f64,
    max: f64,
    clamp: bool,
) -> Option<DVec3> {
    let mount_distance = if body.is_passenger() { 2.0 } else { 0.0 };
    let distance = body.position().distance(target.position());
    let (mut min, mut max) = (
        min + mount_distance - distance,
        max + mount_distance - distance,
    );
    if clamp {
        min = min.max(0.0);
        max = max.max(1.0);
    }
    land_random_pos_away_between(body, min, max, AWAY_VERTICAL_DISTANCE, target.position())
}

/// Vanilla `SpearApproach`: walks up to the attack target until it is within
/// the approach distance, then hands over to [`SpearAttack`].
pub struct SpearApproach {
    speed_modifier_when_repositioning: f64,
    approach_distance_sq: f64,
    entry_condition: [MemoryRequirement; 1],
}

impl SpearApproach {
    /// Vanilla `new SpearApproach(speedModifierWhenRepositioning, approachDistance)`.
    #[must_use]
    pub fn new(speed_modifier_when_repositioning: f64, approach_distance: f32) -> Self {
        Self {
            speed_modifier_when_repositioning,
            approach_distance_sq: f64::from(approach_distance * approach_distance),
            entry_condition: [MemoryRequirement::new(
                &SPEAR_STATUS,
                MemoryStatus::ValueAbsent,
            )],
        }
    }
}

impl<E: PathfinderMob> Behavior<E> for SpearApproach {
    fn entry_condition(&self) -> &[MemoryRequirement] {
        &self.entry_condition
    }

    fn check_extra_start_conditions(&mut self, _world: &Arc<World>, body: &E) -> bool {
        able_to_attack(body) && !body.is_using_item()
    }

    fn start(&mut self, _world: &Arc<World>, body: &E, _time: i64) {
        body.set_aggressive(true);
        set_memory(body, &SPEAR_STATUS, Some(SpearStatus::Approach));
    }

    fn can_still_use(&mut self, _world: &Arc<World>, body: &E, _time: i64) -> bool {
        able_to_attack(body)
            && attack_target_of(body).is_some_and(|target| {
                body.position().distance_squared(target.position()) > self.approach_distance_sq
            })
    }

    fn tick(&mut self, _world: &Arc<World>, body: &E, _time: i64) {
        let Some(target) = attack_target_of(body) else {
            return;
        };
        look_at_entity(body, &target);
        let _ = body.move_to_entity(
            target.as_ref(),
            mount_charge_speed_modifier(body) * self.speed_modifier_when_repositioning,
        );
    }

    fn stop(&mut self, _world: &Arc<World>, body: &E, _time: i64) {
        with_navigation(body, PathNavigation::stop);
        set_memory(body, &SPEAR_STATUS, Some(SpearStatus::Charging));
    }

    fn timed_out(&self, _time: i64, _end_time: i64) -> bool {
        false
    }
}

/// Vanilla `SpearAttack`: charges the attack target with the spear down for
/// the weapon's damage window, stepping back to charge again whenever it
/// reaches the target.
pub struct SpearAttack {
    speed_modifier_when_charging: f64,
    speed_modifier_when_repositioning: f64,
    target_in_range_radius_sq: f64,
    entry_condition: [MemoryRequirement; 1],
}

impl SpearAttack {
    /// Vanilla `new SpearAttack(speedModifierWhenCharging,
    /// speedModifierWhenRepositioning, targetInRangeRadius)`.
    #[must_use]
    pub fn new(
        speed_modifier_when_charging: f64,
        speed_modifier_when_repositioning: f64,
        target_in_range_radius: f32,
    ) -> Self {
        Self {
            speed_modifier_when_charging,
            speed_modifier_when_repositioning,
            target_in_range_radius_sq: f64::from(target_in_range_radius * target_in_range_radius),
            entry_condition: [MemoryRequirement::new(
                &SPEAR_STATUS,
                MemoryStatus::ValuePresent,
            )],
        }
    }
}

impl<E: PathfinderMob> Behavior<E> for SpearAttack {
    fn entry_condition(&self) -> &[MemoryRequirement] {
        &self.entry_condition
    }

    fn check_extra_start_conditions(&mut self, _world: &Arc<World>, body: &E) -> bool {
        spear_status(body) == SpearStatus::Charging && able_to_attack(body) && !body.is_using_item()
    }

    fn start(&mut self, _world: &Arc<World>, body: &E, _time: i64) {
        body.set_aggressive(true);
        set_memory(
            body,
            &SPEAR_ENGAGE_TIME,
            Some(kinetic_weapon_damage_use_duration(body)),
        );
        set_memory(body, &SPEAR_CHARGE_POSITION, None);
        body.start_using_held_item(InteractionHand::MainHand);
    }

    fn can_still_use(&mut self, _world: &Arc<World>, body: &E, _time: i64) -> bool {
        memory(body, &SPEAR_ENGAGE_TIME).unwrap_or(0) > 0 && able_to_attack(body)
    }

    fn tick(&mut self, _world: &Arc<World>, body: &E, _time: i64) {
        let Some(target) = attack_target_of(body) else {
            return;
        };
        let target_dist_sqr = body.position().distance_squared(target.position());
        let speed_modifier = mount_charge_speed_modifier(body);
        look_at_entity(body, &target);
        let engage_time = memory(body, &SPEAR_ENGAGE_TIME).unwrap_or(0) - 1;
        set_memory(body, &SPEAR_ENGAGE_TIME, Some(engage_time));
        if let Some(away_pos) = memory(body, &SPEAR_CHARGE_POSITION) {
            let _ = body.move_to_pos(
                away_pos,
                speed_modifier * self.speed_modifier_when_repositioning,
            );
            if with_navigation(body, |navigation| navigation.is_done()) {
                set_memory(body, &SPEAR_CHARGE_POSITION, None);
            }
        } else {
            let _ = body.move_to_entity(
                target.as_ref(),
                speed_modifier * self.speed_modifier_when_charging,
            );
            if target_dist_sqr < self.target_in_range_radius_sq
                || with_navigation(body, |navigation| navigation.is_done())
            {
                let away_pos = pos_away_from(
                    body,
                    &target,
                    MIN_REPOSITION_DISTANCE,
                    MAX_REPOSITION_DISTANCE,
                    false,
                );
                set_memory(body, &SPEAR_CHARGE_POSITION, away_pos);
            }
        }
    }

    fn stop(&mut self, _world: &Arc<World>, body: &E, _time: i64) {
        with_navigation(body, PathNavigation::stop);
        body.stop_using_held_item();
        set_memory(body, &SPEAR_CHARGE_POSITION, None);
        set_memory(body, &SPEAR_ENGAGE_TIME, None);
        set_memory(body, &SPEAR_STATUS, Some(SpearStatus::Retreat));
    }

    fn timed_out(&self, _time: i64, _end_time: i64) -> bool {
        false
    }
}

/// Vanilla `SpearRetreat`: after a charge, backs away from the attack target
/// for up to 100 ticks so the next charge has room to build speed.
pub struct SpearRetreat {
    speed_modifier_when_repositioning: f64,
    entry_condition: [MemoryRequirement; 1],
}

impl SpearRetreat {
    /// Vanilla `new SpearRetreat(speedModifierWhenRepositioning)`.
    #[must_use]
    pub fn new(speed_modifier_when_repositioning: f64) -> Self {
        Self {
            speed_modifier_when_repositioning,
            entry_condition: [MemoryRequirement::new(
                &SPEAR_STATUS,
                MemoryStatus::ValuePresent,
            )],
        }
    }
}

impl<E: PathfinderMob> Behavior<E> for SpearRetreat {
    fn entry_condition(&self) -> &[MemoryRequirement] {
        &self.entry_condition
    }

    fn duration(&self) -> (i32, i32) {
        (MAX_FLEEING_TIME, MAX_FLEEING_TIME)
    }

    fn check_extra_start_conditions(&mut self, _world: &Arc<World>, body: &E) -> bool {
        if !able_to_attack(body) || body.is_using_item() {
            return false;
        }
        if spear_status(body) != SpearStatus::Retreat {
            return false;
        }
        let Some(target) = attack_target_of(body) else {
            return false;
        };
        let Some(away_pos) = pos_away_from(
            body,
            &target,
            MIN_COOLDOWN_DISTANCE,
            MAX_COOLDOWN_DISTANCE,
            true,
        ) else {
            return false;
        };
        set_memory(body, &SPEAR_FLEEING_POSITION, Some(away_pos));
        true
    }

    fn start(&mut self, _world: &Arc<World>, body: &E, _time: i64) {
        body.set_aggressive(true);
        set_memory(body, &SPEAR_FLEEING_TIME, Some(0));
    }

    fn can_still_use(&mut self, _world: &Arc<World>, body: &E, _time: i64) -> bool {
        memory(body, &SPEAR_FLEEING_TIME).unwrap_or(MAX_FLEEING_TIME) < MAX_FLEEING_TIME
            && memory(body, &SPEAR_FLEEING_POSITION).is_some()
            && !with_navigation(body, |navigation| navigation.is_done())
            && able_to_attack(body)
    }

    fn tick(&mut self, _world: &Arc<World>, body: &E, _time: i64) {
        let Some(target) = attack_target_of(body) else {
            return;
        };
        let speed_modifier = mount_charge_speed_modifier(body);
        look_at_entity(body, &target);
        let fleeing_time = memory(body, &SPEAR_FLEEING_TIME).unwrap_or(0) + 1;
        set_memory(body, &SPEAR_FLEEING_TIME, Some(fleeing_time));
        if let Some(flee_pos) = memory(body, &SPEAR_FLEEING_POSITION) {
            let _ = body.move_to_pos(
                flee_pos,
                speed_modifier * self.speed_modifier_when_repositioning,
            );
        }
    }

    fn stop(&mut self, _world: &Arc<World>, body: &E, _time: i64) {
        with_navigation(body, PathNavigation::stop);
        body.set_aggressive(false);
        body.stop_using_held_item();
        set_memory(body, &SPEAR_FLEEING_TIME, None);
        set_memory(body, &SPEAR_FLEEING_POSITION, None);
        set_memory(body, &SPEAR_STATUS, None);
    }
}
