//! Movement triggers that `ServerPlayer` drives from positions it remembers
//! (vanilla `CriteriaTriggers` `LEVITATION`, `FALL_FROM_HEIGHT`,
//! `RIDE_ENTITY_IN_LAVA_TRIGGER`, `NETHER_TRAVEL` and `FALL_AFTER_EXPLOSION`).

use glam::DVec3;
use steel_registry::vanilla_mob_effects;
use steel_utils::Identifier;

use super::predicates::{ConditionContext, DistancePredicate};
use super::trigger;
use super::{Conditions, DistanceField};
use crate::entity::{Entity, LivingEntity as _, SharedEntity};
use crate::player::Player;
use crate::world::World;

/// Vanilla `CriteriaTriggers.LEVITATION`.
pub(crate) const LEVITATION: Identifier = Identifier::vanilla_static("levitation");
/// Vanilla `CriteriaTriggers.FALL_FROM_HEIGHT`.
pub(crate) const FALL_FROM_HEIGHT: Identifier = Identifier::vanilla_static("fall_from_height");
/// Vanilla `CriteriaTriggers.RIDE_ENTITY_IN_LAVA_TRIGGER`.
pub(crate) const RIDE_ENTITY_IN_LAVA: Identifier =
    Identifier::vanilla_static("ride_entity_in_lava");
/// Vanilla `CriteriaTriggers.NETHER_TRAVEL`.
pub(crate) const NETHER_TRAVEL: Identifier = Identifier::vanilla_static("nether_travel");
/// Vanilla `CriteriaTriggers.FALL_AFTER_EXPLOSION`.
pub(crate) const FALL_AFTER_EXPLOSION: Identifier =
    Identifier::vanilla_static("fall_after_explosion");

/// Vanilla `Level.OVERWORLD`.
const OVERWORLD: Identifier = Identifier::vanilla_static("overworld");
/// Vanilla `Level.NETHER`.
const NETHER: Identifier = Identifier::vanilla_static("the_nether");

/// The movement trigger positions vanilla `ServerPlayer` saves.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SavedMovementPositions {
    /// Vanilla `entered_nether_pos`: where the player last left the
    /// overworld for the nether.
    pub entered_nether_pos: Option<[f64; 3]>,
    /// Vanilla `last_explosion_impact_pos`: where the last explosion hit.
    pub last_explosion_impact_pos: Option<[f64; 3]>,
}

/// The positions `ServerPlayer` keeps for its movement triggers.
#[derive(Default, Clone)]
pub struct MovementCriteria {
    /// Vanilla `levitationStartPos` with `levitationStartTime`.
    levitation_start: Option<(DVec3, i32)>,
    /// Vanilla `startingToFallPosition`.
    starting_to_fall: Option<DVec3>,
    /// Vanilla `enteredNetherPosition`, saved as `entered_nether_pos`.
    entered_nether: Option<DVec3>,
    /// Vanilla `enteredLavaOnVehiclePosition`.
    entered_lava_on_vehicle: Option<DVec3>,
    /// Vanilla `currentExplosionImpactPos`, saved as
    /// `last_explosion_impact_pos`.
    explosion_impact: Option<DVec3>,
    /// Vanilla `currentExplosionCause`, held strongly like vanilla: a wind
    /// charge is gone from the world by the time the fall starts.
    explosion_cause: Option<SharedEntity>,
}

impl MovementCriteria {
    /// Vanilla `enteredNetherPosition`.
    #[must_use]
    pub const fn entered_nether(&self) -> Option<DVec3> {
        self.entered_nether
    }

    /// Vanilla `currentExplosionImpactPos`.
    #[must_use]
    pub const fn explosion_impact(&self) -> Option<DVec3> {
        self.explosion_impact
    }

    /// The positions vanilla saves with the player.
    #[must_use]
    pub fn saved(&self) -> SavedMovementPositions {
        SavedMovementPositions {
            entered_nether_pos: self.entered_nether.map(Into::into),
            last_explosion_impact_pos: self.explosion_impact.map(Into::into),
        }
    }

    /// Restores the positions vanilla saves with the player.
    pub fn restore_saved(&mut self, saved: SavedMovementPositions) {
        self.entered_nether = saved.entered_nether_pos.map(DVec3::from_array);
        self.explosion_impact = saved.last_explosion_impact_pos.map(DVec3::from_array);
    }

    /// Vanilla `ServerPlayer.restoreFrom`: the nether entry and the explosion
    /// carry over to the respawned player.
    pub fn restore_from(&mut self, old: &Self) {
        self.entered_nether = old.entered_nether;
        self.explosion_impact = old.explosion_impact;
        self.explosion_cause.clone_from(&old.explosion_cause);
    }
}

/// Vanilla `DistanceTrigger.TriggerInstance.matches`.
fn distance_trigger_matches(
    conditions: &Conditions,
    world: &World,
    entered: DVec3,
    player_position: DVec3,
) -> bool {
    conditions
        .start_position
        .as_ref()
        .is_none_or(|start| start.matches(world, entered.x, entered.y, entered.z))
        && distance_matches(conditions.distance.as_ref(), entered, player_position)
}

/// A trigger's `distance` field; plain bounds only belong to `used_ender_eye`,
/// so they never match here.
fn distance_matches(distance: Option<&DistanceField>, from: DVec3, to: DVec3) -> bool {
    match distance {
        None => true,
        Some(DistanceField::Distance(predicate)) => DistancePredicate::matches(predicate, from, to),
        Some(DistanceField::Bounds(_)) => false,
    }
}

impl Player {
    /// Vanilla `ServerPlayer.onEffectAdded` for levitation: remembers where
    /// and when it started.
    pub(crate) fn note_levitation_started(&self) {
        self.movement_criteria.lock().levitation_start = Some((self.position(), self.tick_count()));
    }

    /// Vanilla `ServerPlayer.tick`: the levitation trigger, then
    /// `trackStartFallingPosition` and `trackEnteredOrExitedLavaOnVehicle`.
    pub(crate) fn tick_movement_criteria(&self) {
        let levitation_start = {
            let mut criteria = self.movement_criteria.lock();
            // Vanilla `onEffectsRemoved` forgets the start once levitation ends.
            if !self
                .living_base()
                .has_mob_effect(vanilla_mob_effects::LEVITATION)
            {
                criteria.levitation_start = None;
            }
            criteria.levitation_start
        };
        if let Some((start, start_tick)) = levitation_start {
            self.trigger_levitation(start, self.tick_count() - start_tick);
        }
        self.track_start_falling_position();
        self.track_entered_or_exited_lava_on_vehicle();
    }

    /// Vanilla `LevitationTrigger.trigger(player, start, duration)`.
    fn trigger_levitation(&self, start: DVec3, duration: i32) {
        let position = self.position();
        trigger(self, &LEVITATION, |conditions| {
            distance_matches(conditions.distance.as_ref(), start, position)
                && conditions.duration.matches(duration)
        });
    }

    /// Vanilla `ServerPlayer.trackStartFallingPosition`.
    fn track_start_falling_position(&self) {
        let explosion = {
            let mut criteria = self.movement_criteria.lock();
            if self.fall_distance() <= 0.0 || criteria.starting_to_fall.is_some() {
                return;
            }
            let start = self.position();
            criteria.starting_to_fall = Some(start);
            let impact = criteria.explosion_impact.take();
            let cause = criteria.explosion_cause.take();
            impact
                .filter(|impact| impact.y <= start.y)
                .map(|impact| (impact, cause))
        };
        if let Some((impact, cause)) = explosion {
            self.trigger_fall_after_explosion(impact, cause.as_deref());
        }
    }

    /// Vanilla `FallAfterExplosionTrigger.trigger(player, start, cause)`.
    fn trigger_fall_after_explosion(&self, start: DVec3, cause: Option<&dyn Entity>) {
        let world = self.get_world();
        let position = self.position();
        let cause = cause.map(|cause| ConditionContext::entity(&world, position, cause));
        trigger(self, &FALL_AFTER_EXPLOSION, |conditions| {
            distance_trigger_matches(conditions, &world, start, position)
                && conditions.cause.as_ref().is_none_or(|predicate| {
                    cause.as_ref().is_some_and(|cause| predicate.test(cause))
                })
        });
    }

    /// Vanilla `ServerPlayer.trackEnteredOrExitedLavaOnVehicle`.
    fn track_entered_or_exited_lava_on_vehicle(&self) {
        let vehicle_in_lava = self.vehicle().is_some_and(|vehicle| vehicle.is_in_lava());
        let entered = {
            let mut criteria = self.movement_criteria.lock();
            if !vehicle_in_lava {
                criteria.entered_lava_on_vehicle = None;
                return;
            }
            let Some(entered) = criteria.entered_lava_on_vehicle else {
                criteria.entered_lava_on_vehicle = Some(self.position());
                return;
            };
            entered
        };
        self.trigger_distance(&RIDE_ENTITY_IN_LAVA, entered);
    }

    /// Vanilla `DistanceTrigger.trigger(player, startPosition)`.
    fn trigger_distance(&self, trigger_id: &Identifier, entered: DVec3) {
        let world = self.get_world();
        let position = self.position();
        trigger(self, trigger_id, |conditions| {
            distance_trigger_matches(conditions, &world, entered, position)
        });
    }

    /// Vanilla `ServerPlayer.resetFallDistance`: a living player that was
    /// falling fires `FALL_FROM_HEIGHT` from where the fall started.
    pub(crate) fn note_fall_distance_reset(&self) {
        let start = self.movement_criteria.lock().starting_to_fall.take();
        if let Some(start) = start
            && self.get_health() > 0.0
        {
            self.trigger_distance(&FALL_FROM_HEIGHT, start);
        }
    }

    /// Vanilla `ServerPlayer.onExplosionHit`: remembers the impact for
    /// `FALL_AFTER_EXPLOSION`.
    pub(crate) fn note_explosion_hit(&self, cause: Option<&dyn Entity>) {
        let mut criteria = self.movement_criteria.lock();
        criteria.explosion_impact = Some(self.position());
        // Vanilla keeps the entity itself; the world still holds the cause
        // while its explosion lands.
        criteria.explosion_cause = cause.and_then(|cause| {
            cause
                .level()
                .and_then(|world| world.get_entity_by_id(cause.id()))
        });
    }

    /// Vanilla `ServerPlayer.teleport`: leaving the overworld for the nether
    /// remembers where.
    pub(crate) fn note_dimension_leaving(&self, from: &Identifier, to: &Identifier) {
        if *from == OVERWORLD && *to == NETHER {
            self.movement_criteria.lock().entered_nether = Some(self.position());
        }
    }

    /// Vanilla `ServerPlayer.triggerDimensionChangeTriggers` after
    /// `CHANGED_DIMENSION`: back from the nether fires `NETHER_TRAVEL`, and
    /// anywhere but the nether forgets the entry.
    pub(crate) fn trigger_nether_travel(&self, from: &Identifier, to: &Identifier) {
        let entered = self.movement_criteria.lock().entered_nether;
        if *from == NETHER
            && *to == OVERWORLD
            && let Some(entered) = entered
        {
            self.trigger_distance(&NETHER_TRAVEL, entered);
        }
        if *to != NETHER {
            self.movement_criteria.lock().entered_nether = None;
        }
    }
}
