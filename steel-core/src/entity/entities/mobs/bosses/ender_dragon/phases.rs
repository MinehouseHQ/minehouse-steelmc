//! Vanilla `EnderDragonPhase` and its phase instances: what the dragon is
//! doing and where it flies.
//!
//! Vanilla keeps one instance of every phase and reuses it, so the state that
//! a phase's `begin` does not reset (the holding pattern's direction, the
//! flaming phase's flame count) lives on between visits. [`DragonPhases`]
//! keeps every phase's state the same way.

use std::f32::consts::PI;
use std::sync::{Arc, Weak};

use glam::DVec3;
use steel_math::angle::{DEG_TO_RAD, RAD_TO_DEG, wrap_degrees};
use steel_math::trig;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::particle_type::{ParticleData, PowerParticleOption};
use steel_registry::{
    MobEffectInstance, level_events, vanilla_entities, vanilla_mob_effects, vanilla_particle_types,
};
use steel_utils::BlockPos;

use super::EnderDragonEntity;
use crate::entity::ai::node::Node;
use crate::entity::ai::path::Path;
use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::damage::DamageSource;
use crate::entity::entities::{AreaEffectCloudEntity, DragonFireballEntity};
use crate::entity::{
    AbstractHurtingProjectile as _, Entity, LivingEntity, Mob, RemovalReason, SharedEntity,
    next_entity_id,
};
use crate::world::World;

/// Vanilla `EnderDragonPhase`, in id order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragonPhase {
    /// Circling the island between the towers.
    HoldingPattern,
    /// Flying at a player to shoot a fireball.
    StrafePlayer,
    /// Flying toward the portal to land.
    LandingApproach,
    /// Descending onto the portal.
    Landing,
    /// Leaving the portal.
    Takeoff,
    /// Breathing a cloud of dragon's breath on the portal.
    SittingFlaming,
    /// Looking around for a player while perched.
    SittingScanning,
    /// Roaring at a player while perched.
    SittingAttacking,
    /// Charging at a player.
    ChargingPlayer,
    /// The death flight back to the portal.
    Dying,
    /// Hovering in place; the phase a fresh dragon starts in.
    Hovering,
}

impl DragonPhase {
    const ALL: [Self; 11] = [
        Self::HoldingPattern,
        Self::StrafePlayer,
        Self::LandingApproach,
        Self::Landing,
        Self::Takeoff,
        Self::SittingFlaming,
        Self::SittingScanning,
        Self::SittingAttacking,
        Self::ChargingPlayer,
        Self::Dying,
        Self::Hovering,
    ];

    /// Vanilla `getId`.
    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }

    /// Vanilla `getById`: unknown ids fall back to the holding pattern.
    #[must_use]
    pub fn from_id(id: i32) -> Self {
        usize::try_from(id)
            .ok()
            .and_then(|index| Self::ALL.get(index).copied())
            .unwrap_or(Self::HoldingPattern)
    }

    /// Vanilla `isSitting`: the perched phases and hovering.
    #[must_use]
    pub const fn is_sitting(self) -> bool {
        matches!(
            self,
            Self::SittingFlaming | Self::SittingScanning | Self::SittingAttacking | Self::Hovering
        )
    }

    /// Vanilla `getFlySpeed`.
    #[must_use]
    pub const fn fly_speed(self) -> f32 {
        match self {
            Self::Landing => 1.5,
            Self::ChargingPlayer | Self::Dying => 3.0,
            Self::Hovering => 1.0,
            _ => 0.6,
        }
    }
}

/// A path along the node graph and the point it is flying to now.
#[derive(Default)]
pub(super) struct PathState {
    path: Option<Path>,
    target: Option<DVec3>,
}

impl PathState {
    fn reset(&mut self) {
        self.path = None;
        self.target = None;
    }

    fn path_done(&self) -> bool {
        self.path.as_ref().is_some_and(Path::is_done)
    }

    fn needs_path(&self) -> bool {
        self.path.as_ref().is_none_or(Path::is_done)
    }

    /// Vanilla `navigateToNextPathNode`: fly to the next node, up to 20
    /// blocks above it.
    fn navigate_to_next_node(&mut self) {
        let Some(path) = self.path.as_mut() else {
            return;
        };
        if path.is_done() {
            return;
        }
        let Some(current) = path.next_node_pos() else {
            return;
        };
        path.advance();
        let y = f64::from(current.y()) + f64::from(rand::random::<f32>() * 20.0);
        self.target = Some(DVec3::new(
            f64::from(current.x()),
            y,
            f64::from(current.z()),
        ));
    }

    fn set_path(&mut self, path: Option<Path>) {
        self.path = path;
        if let Some(path) = self.path.as_mut() {
            path.advance();
        }
    }
}

/// Every phase's state, kept between visits like vanilla's phase instances.
#[derive(Default)]
pub(super) struct DragonPhases {
    pub(super) current: Option<DragonPhase>,
    holding: PathState,
    holding_clockwise: bool,
    strafe: PathState,
    strafe_clockwise: bool,
    strafe_fireball_charge: i32,
    strafe_target: Option<Weak<dyn Entity>>,
    approach: PathState,
    landing_target: Option<DVec3>,
    takeoff: PathState,
    takeoff_first_tick: bool,
    flame_ticks: i32,
    flame_count: i32,
    flame: Option<Weak<dyn Entity>>,
    scanning_time: i32,
    attacking_ticks: i32,
    charge_target: Option<DVec3>,
    time_since_charge: i32,
    death_target: Option<DVec3>,
    death_time: i32,
    hover_target: Option<DVec3>,
}

impl DragonPhases {
    /// Vanilla `begin` of `phase`.
    fn begin(&mut self, phase: DragonPhase) {
        match phase {
            DragonPhase::HoldingPattern => self.holding.reset(),
            DragonPhase::StrafePlayer => {
                self.strafe_fireball_charge = 0;
                self.strafe.reset();
                self.strafe_target = None;
            }
            DragonPhase::LandingApproach => self.approach.reset(),
            DragonPhase::Landing => self.landing_target = None,
            DragonPhase::Takeoff => {
                self.takeoff_first_tick = true;
                self.takeoff.reset();
            }
            DragonPhase::SittingFlaming => {
                self.flame_ticks = 0;
                self.flame_count += 1;
            }
            DragonPhase::SittingScanning => self.scanning_time = 0,
            DragonPhase::SittingAttacking => self.attacking_ticks = 0,
            DragonPhase::ChargingPlayer => {
                self.charge_target = None;
                self.time_since_charge = 0;
            }
            DragonPhase::Dying => {
                self.death_target = None;
                self.death_time = 0;
            }
            DragonPhase::Hovering => self.hover_target = None,
        }
    }

    /// Vanilla `getFlyTargetLocation` of the current phase.
    pub(super) fn fly_target(&self) -> Option<DVec3> {
        match self.current? {
            DragonPhase::HoldingPattern => self.holding.target,
            DragonPhase::StrafePlayer => self.strafe.target,
            DragonPhase::LandingApproach => self.approach.target,
            DragonPhase::Landing => self.landing_target,
            DragonPhase::Takeoff => self.takeoff.target,
            DragonPhase::ChargingPlayer => self.charge_target,
            DragonPhase::Dying => self.death_target,
            DragonPhase::Hovering => self.hover_target,
            DragonPhase::SittingFlaming
            | DragonPhase::SittingScanning
            | DragonPhase::SittingAttacking => None,
        }
    }
}

/// Vanilla `TargetingConditions.forCombat().ignoreLineOfSight()`, used to pick
/// the player nearest the portal.
const fn near_egg_targeting() -> TargetingConditions {
    TargetingConditions::for_combat().ignore_line_of_sight()
}

/// Vanilla `getNearestPlayer(conditions, dragon, x, y, z)`.
fn nearest_player(
    world: &World,
    dragon: &EnderDragonEntity,
    conditions: &TargetingConditions,
    pos: DVec3,
) -> Option<SharedEntity> {
    world
        .nearest_player(pos, -1.0, |player| {
            conditions.test(world, Some(dragon), player)
        })
        .map(|player| player as SharedEntity)
}

/// The node to circle toward, wrapped to the outer ring while the fight has
/// crystals left and to the inner rings otherwise.
fn wrap_ring(index: i32, outer_ring: bool) -> usize {
    let wrapped = if outer_ring {
        index.rem_euclid(12)
    } else {
        ((index - 12) & 7) + 12
    };
    usize::try_from(wrapped).unwrap_or(0)
}

/// The angle in degrees between the dragon's heading and the direction to
/// `target`, plus vanilla's half-degree bias.
fn heading_angle_to(dragon: &EnderDragonEntity, target: DVec3) -> f32 {
    let position = dragon.position();
    let aim = DVec3::new(target.x - position.x, 0.0, target.z - position.z).normalize_or_zero();
    let y_rot = dragon.rotation().0 * DEG_TO_RAD;
    let dir = DVec3::new(
        f64::from(trig::sin(y_rot)),
        0.0,
        f64::from(-trig::cos(y_rot)),
    )
    .normalize_or_zero();
    let dot = dir.dot(aim) as f32;
    (f64::from(dot).acos() * f64::from(180.0_f32) / f64::from(PI)) as f32 + 0.5
}

impl EnderDragonEntity {
    /// Vanilla `EnderDragonPhaseManager.setPhase`.
    pub fn set_phase(&self, phase: DragonPhase) {
        let ended_flame = {
            let mut state = self.state.lock();
            let phases = &mut state.phases;
            if phases.current == Some(phase) {
                return;
            }
            // Vanilla `DragonSittingFlamingPhase.end` discards the flame.
            let ended_flame = if phases.current == Some(DragonPhase::SittingFlaming) {
                phases.flame.take()
            } else {
                None
            };
            phases.current = Some(phase);
            phases.begin(phase);
            ended_flame
        };
        if let Some(flame) = ended_flame.and_then(|flame| flame.upgrade()) {
            flame.set_removed(RemovalReason::Discarded);
        }
        self.entity_data.lock().phase.set(phase.id());
        log::debug!("Dragon is now in phase {phase:?} on the server");
    }

    /// The current phase.
    #[must_use]
    pub fn phase(&self) -> DragonPhase {
        self.state
            .lock()
            .phases
            .current
            .unwrap_or(DragonPhase::Hovering)
    }

    /// Vanilla `getTurnSpeed` of the current phase.
    pub(super) fn turn_speed(&self) -> f32 {
        let rot_speed = self.velocity().x.hypot(self.velocity().z) as f32 + 1.0;
        let dist = rot_speed.min(40.0);
        if self.phase() == DragonPhase::Landing {
            dist / rot_speed
        } else {
            0.7 / dist / rot_speed
        }
    }

    /// Vanilla `onHurt` of the current phase: perched, the dragon shrugs off
    /// arrows and wind charges and sets them on fire.
    pub(super) fn phase_on_hurt(&self, source: &DamageSource, damage: f32) -> f32 {
        let sitting = self.phase().is_sitting() && self.phase() != DragonPhase::Hovering;
        if sitting
            && let Some(direct) = source.direct_entity()
            && (direct.is_abstract_arrow()
                || direct.entity_type() == &vanilla_entities::WIND_CHARGE)
        {
            // Vanilla `igniteForSeconds(1.0F)`.
            direct.ignite_for_ticks(20);
            return 0.0;
        }
        damage
    }

    /// Runs the current phase's `doServerTick`.
    pub(super) fn tick_phase(&self, world: &Arc<World>, this: &SharedEntity) {
        match self.phase() {
            DragonPhase::HoldingPattern => self.tick_holding_pattern(world),
            DragonPhase::StrafePlayer => self.tick_strafe_player(world, this),
            DragonPhase::LandingApproach => self.tick_landing_approach(world),
            DragonPhase::Landing => self.tick_landing(world),
            DragonPhase::Takeoff => self.tick_takeoff(world),
            DragonPhase::SittingFlaming => self.tick_sitting_flaming(world, this),
            DragonPhase::SittingScanning => self.tick_sitting_scanning(world),
            DragonPhase::SittingAttacking => self.tick_sitting_attacking(),
            DragonPhase::ChargingPlayer => self.tick_charging_player(),
            DragonPhase::Dying => self.tick_dying(world),
            DragonPhase::Hovering => {
                let position = self.position();
                self.state
                    .lock()
                    .phases
                    .hover_target
                    .get_or_insert(position);
            }
        }
    }

    /// Whether the fly target is reached, too far, or blocked: vanilla's
    /// `distToTarget < 100 || distToTarget > 22500 || collision` check.
    fn should_retarget(&self, target: Option<DVec3>) -> bool {
        let dist = target.map_or(0.0, |target| target.distance_squared(self.position()));
        dist < 100.0 || dist > 22_500.0 || self.horizontal_collision() || self.vertical_collision()
    }

    /// Vanilla `DragonHoldingPatternPhase.doServerTick`.
    fn tick_holding_pattern(&self, world: &Arc<World>) {
        let target = self.state.lock().phases.holding.target;
        if self.should_retarget(target) {
            self.holding_find_new_target(world);
        }
    }

    /// Vanilla `DragonHoldingPatternPhase.findNewTarget`.
    fn holding_find_new_target(&self, world: &Arc<World>) {
        if self.state.lock().phases.holding.path_done() {
            let egg = self.podium_surface(world);
            let crystals = self.alive_crystals().unwrap_or(0);
            if rand::random_range(0..crystals + 3) == 0 {
                self.set_phase(DragonPhase::LandingApproach);
                return;
            }
            let player = nearest_player(world, self, &near_egg_targeting(), super::corner_of(egg));
            let dist_sqr = player.as_ref().map_or(64.0, |player| {
                super::center_of(egg).distance_squared(player.position()) / 512.0
            });
            if let Some(player) = player
                && (rand::random_range(0..(dist_sqr + 2.0) as i32) == 0
                    || rand::random_range(0..crystals + 2) == 0)
            {
                self.strafe_player(&player);
                return;
            }
        }

        if self.state.lock().phases.holding.needs_path() {
            let current = self.find_closest_node();
            let path = {
                let mut state = self.state.lock();
                let phases = &mut state.phases;
                let mut target = current as i32;
                if rand::random_range(0..8) == 0 {
                    phases.holding_clockwise = !phases.holding_clockwise;
                    target = current as i32 + 6;
                }
                if phases.holding_clockwise {
                    target += 1;
                } else {
                    target -= 1;
                }
                // Vanilla checks `aliveCrystals() >= 0` here, so any fight
                // keeps the dragon on the outer ring.
                target
            };
            let target = wrap_ring(path, self.alive_crystals().is_some());
            let path = self.find_path(current, target, None);
            self.state.lock().phases.holding.set_path(path);
        }
        self.state.lock().phases.holding.navigate_to_next_node();
    }

    /// Vanilla `DragonHoldingPatternPhase.strafePlayer`.
    fn strafe_player(&self, player: &SharedEntity) {
        self.set_phase(DragonPhase::StrafePlayer);
        self.strafe_set_target(player);
    }

    /// Vanilla `DragonHoldingPatternPhase.onCrystalDestroyed`.
    pub(super) fn phase_on_crystal_destroyed(&self, player: Option<&SharedEntity>) {
        if self.phase() != DragonPhase::HoldingPattern {
            return;
        }
        if let Some(player) = player
            && player
                .as_living_entity()
                .is_some_and(|target| Mob::can_attack(self, target))
        {
            self.strafe_player(player);
        }
    }

    /// Vanilla `DragonStrafePlayerPhase.setTarget`.
    fn strafe_set_target(&self, target: &SharedEntity) {
        self.state.lock().phases.strafe_target = Some(Arc::downgrade(target));
        let current = self.find_closest_node();
        let target_pos = target.position();
        let target_node = self.find_closest_node_at(target_pos);
        let block = target.block_position();
        let xd = f64::from(block.x()) - self.position().x;
        let zd = f64::from(block.z()) - self.position().z;
        let sd = (xd * xd + zd * zd).sqrt();
        let height_offset = (f64::from(0.4_f32) + sd / 80.0 - 1.0).min(10.0);
        let final_node = Node::new(
            block.x(),
            (target_pos.y + height_offset).floor() as i32,
            block.z(),
        );
        let path = self.find_path(current, target_node, Some(final_node));
        let mut state = self.state.lock();
        let strafe = &mut state.phases.strafe;
        strafe.path = path;
        if let Some(path) = strafe.path.as_mut() {
            path.advance();
            strafe.navigate_to_next_node();
        }
    }

    /// Vanilla `DragonStrafePlayerPhase.doServerTick`.
    fn tick_strafe_player(&self, world: &Arc<World>, this: &SharedEntity) {
        let target = self
            .state
            .lock()
            .phases
            .strafe_target
            .as_ref()
            .and_then(Weak::upgrade);
        let Some(target) = target else {
            log::warn!("Skipping player strafe phase because no player was found");
            self.set_phase(DragonPhase::HoldingPattern);
            return;
        };
        let position = self.position();
        let target_pos = target.position();
        {
            let mut state = self.state.lock();
            let strafe = &mut state.phases.strafe;
            if strafe.path_done() {
                let dist = (target_pos.x - position.x).hypot(target_pos.z - position.z);
                let height_offset = (f64::from(0.4_f32) + dist / 80.0 - 1.0).min(10.0);
                strafe.target = Some(DVec3::new(
                    target_pos.x,
                    target_pos.y + height_offset,
                    target_pos.z,
                ));
            }
        }
        let fly_target = self.state.lock().phases.strafe.target;
        let dist = fly_target.map_or(0.0, |fly| fly.distance_squared(position));
        if dist < 100.0 || dist > 22_500.0 {
            self.strafe_find_new_target();
        }

        let can_see = target_pos.distance_squared(position) < 4096.0
            && target
                .as_living_entity()
                .is_some_and(|_| self.has_line_of_sight(target.as_ref()));
        if !can_see {
            let mut state = self.state.lock();
            let charge = &mut state.phases.strafe_fireball_charge;
            if *charge > 0 {
                *charge -= 1;
            }
            return;
        }
        let charge = {
            let mut state = self.state.lock();
            state.phases.strafe_fireball_charge += 1;
            state.phases.strafe_fireball_charge
        };
        let angle = heading_angle_to(self, target_pos);
        if charge >= 5 && (0.0..10.0).contains(&angle) {
            self.shoot_fireball(world, this, &target);
            {
                let mut state = self.state.lock();
                state.phases.strafe_fireball_charge = 0;
                if let Some(path) = state.phases.strafe.path.as_mut() {
                    while !path.is_done() {
                        path.advance();
                    }
                }
            }
            self.set_phase(DragonPhase::HoldingPattern);
        }
    }

    /// The fireball half of vanilla `DragonStrafePlayerPhase.doServerTick`.
    fn shoot_fireball(&self, world: &Arc<World>, this: &SharedEntity, target: &SharedEntity) {
        let view = self.view_vector();
        let head = self.part(super::HEAD);
        let start = DVec3::new(
            head.position().x - view.x,
            head.position().y + head.bounding_box().height() * 0.5 + 0.5,
            head.position().z - view.z,
        );
        let aim =
            target.position() + DVec3::new(0.0, target.bounding_box().height() * 0.5, 0.0) - start;
        if !self.is_silent() {
            world.level_event(
                level_events::SOUND_DRAGON_FIREBALL,
                self.block_position(),
                0,
                None,
            );
        }
        let fireball = Arc::new(DragonFireballEntity::new(
            &vanilla_entities::DRAGON_FIREBALL,
            next_entity_id(),
            start,
            Arc::downgrade(world),
        ));
        fireball.shoot_from_mob(this, aim.normalize_or_zero());
        fireball.snap_to(start, 0.0, 0.0);
        if let Err(error) = world.try_add_entity(fireball) {
            log::debug!("world rejected a dragon fireball: {error}");
        }
    }

    /// Vanilla `DragonStrafePlayerPhase.findNewTarget`.
    fn strafe_find_new_target(&self) {
        if self.state.lock().phases.strafe.needs_path() {
            let current = self.find_closest_node();
            let target = {
                let mut state = self.state.lock();
                let phases = &mut state.phases;
                let mut target = current as i32;
                if rand::random_range(0..8) == 0 {
                    phases.strafe_clockwise = !phases.strafe_clockwise;
                    target = current as i32 + 6;
                }
                if phases.strafe_clockwise {
                    target + 1
                } else {
                    target - 1
                }
            };
            let target = wrap_ring(target, self.alive_crystals().is_some_and(|alive| alive > 0));
            let path = self.find_path(current, target, None);
            self.state.lock().phases.strafe.set_path(path);
        }
        self.state.lock().phases.strafe.navigate_to_next_node();
    }

    /// Vanilla `DragonLandingApproachPhase.doServerTick`.
    fn tick_landing_approach(&self, world: &Arc<World>) {
        let target = self.state.lock().phases.approach.target;
        if !self.should_retarget(target) {
            return;
        }
        if self.state.lock().phases.approach.needs_path() {
            let current = self.find_closest_node();
            let egg = self.podium_surface(world);
            let player = nearest_player(world, self, &near_egg_targeting(), super::corner_of(egg));
            let target = if let Some(player) = player {
                let aim =
                    DVec3::new(player.position().x, 0.0, player.position().z).normalize_or_zero();
                self.find_closest_node_at(DVec3::new(-aim.x * 40.0, 105.0, -aim.z * 40.0))
            } else {
                self.find_closest_node_at(DVec3::new(40.0, f64::from(egg.y()), 0.0))
            };
            let final_node = Node::new(egg.x(), egg.y(), egg.z());
            let path = self.find_path(current, target, Some(final_node));
            self.state.lock().phases.approach.set_path(path);
        }
        let done = {
            let mut state = self.state.lock();
            state.phases.approach.navigate_to_next_node();
            state.phases.approach.path_done()
        };
        if done {
            self.set_phase(DragonPhase::Landing);
        }
    }

    /// Vanilla `DragonLandingPhase.doServerTick`.
    fn tick_landing(&self, world: &Arc<World>) {
        let target = {
            let egg = self.podium_surface(world);
            *self
                .state
                .lock()
                .phases
                .landing_target
                .get_or_insert_with(|| super::bottom_center_of(egg))
        };
        if target.distance_squared(self.position()) < 1.0 {
            self.state.lock().phases.flame_count = 0;
            self.set_phase(DragonPhase::SittingScanning);
        }
    }

    /// Vanilla `DragonTakeoffPhase.doServerTick`.
    fn tick_takeoff(&self, world: &Arc<World>) {
        let (first_tick, has_path) = {
            let state = self.state.lock();
            (
                state.phases.takeoff_first_tick,
                state.phases.takeoff.path.is_some(),
            )
        };
        if !first_tick && has_path {
            let egg = self.podium_surface(world);
            if super::center_of(egg).distance_squared(self.position()) >= 100.0 {
                self.set_phase(DragonPhase::HoldingPattern);
            }
            return;
        }
        self.state.lock().phases.takeoff_first_tick = false;
        let current = self.find_closest_node();
        let look = self.head_look_vector();
        let target = self.find_closest_node_at(DVec3::new(-look.x * 40.0, 105.0, -look.z * 40.0));
        let target = wrap_ring(
            target as i32,
            self.alive_crystals().is_some_and(|alive| alive > 0),
        );
        let path = self.find_path(current, target, None);
        let mut state = self.state.lock();
        let takeoff = &mut state.phases.takeoff;
        takeoff.path = path;
        // Vanilla `DragonTakeoffPhase.navigateToNextPathNode` advances first.
        if let Some(path) = takeoff.path.as_mut() {
            path.advance();
            if !path.is_done() {
                takeoff.navigate_to_next_node();
            }
        }
    }

    /// Vanilla `DragonSittingFlamingPhase.doServerTick`.
    fn tick_sitting_flaming(&self, world: &Arc<World>, this: &SharedEntity) {
        let (ticks, count) = {
            let mut state = self.state.lock();
            state.phases.flame_ticks += 1;
            (state.phases.flame_ticks, state.phases.flame_count)
        };
        if ticks >= 200 {
            self.set_phase(if count >= 4 {
                DragonPhase::Takeoff
            } else {
                DragonPhase::SittingScanning
            });
        } else if ticks == 10 {
            let flame = self.breathe_flame(world, this);
            self.state.lock().phases.flame = flame.map(|flame| Arc::downgrade(&flame));
        }
    }

    /// The flame cloud of vanilla `DragonSittingFlamingPhase.doServerTick`.
    fn breathe_flame(&self, world: &Arc<World>, this: &SharedEntity) -> Option<SharedEntity> {
        let head = self.part(super::HEAD);
        let head_pos = head.position();
        let position = self.position();
        let look =
            DVec3::new(head_pos.x - position.x, 0.0, head_pos.z - position.z).normalize_or_zero();
        let x = head_pos.x + look.x * 5.0 / 2.0;
        let z = head_pos.z + look.z * 5.0 / 2.0;
        let initial_y = head_pos.y + head.bounding_box().height() * 0.5;
        let mut y = initial_y;
        let mut pos = BlockPos::containing(x, initial_y, z);
        while world.get_block_state(pos).is_air() {
            y -= 1.0;
            if y < 0.0 {
                y = initial_y;
                break;
            }
            pos = BlockPos::containing(x, y, z);
        }
        let y = y.floor() + 1.0;

        let cloud = Arc::new(AreaEffectCloudEntity::new(
            &vanilla_entities::AREA_EFFECT_CLOUD,
            next_entity_id(),
            DVec3::new(x, y, z),
            Arc::downgrade(world),
        ));
        cloud.set_owner(Some(this));
        cloud.set_radius(5.0);
        cloud.set_duration(200);
        cloud.set_custom_particle(ParticleData::new(
            &vanilla_particle_types::DRAGON_BREATH,
            PowerParticleOption::new(1.0),
        ));
        cloud.set_potion_duration_scale(0.25);
        cloud.add_effect(MobEffectInstance::new(
            vanilla_mob_effects::INSTANT_DAMAGE,
            0,
            0,
            false,
            true,
            true,
            None,
        ));
        let cloud: SharedEntity = cloud;
        match world.try_add_entity(Arc::clone(&cloud)) {
            Ok(()) => Some(cloud),
            Err(error) => {
                log::debug!("world rejected the dragon's flame: {error}");
                None
            }
        }
    }

    /// Vanilla `DragonSittingScanningPhase.doServerTick`.
    fn tick_sitting_scanning(&self, world: &Arc<World>) {
        let scanning_time = {
            let mut state = self.state.lock();
            state.phases.scanning_time += 1;
            state.phases.scanning_time
        };
        let dragon_y = self.position().y;
        let scan = TargetingConditions::for_combat()
            .range(20.0)
            .selector(move |target, _| (target.position().y - dragon_y).abs() <= 10.0);
        if let Some(target) = nearest_player(world, self, &scan, self.position()) {
            if scanning_time > 25 {
                self.set_phase(DragonPhase::SittingAttacking);
            } else {
                self.turn_toward_while_perched(target.position());
            }
        } else if scanning_time >= 100 {
            let charge = TargetingConditions::for_combat().range(150.0);
            let target = nearest_player(world, self, &charge, self.position());
            self.set_phase(DragonPhase::Takeoff);
            if let Some(target) = target {
                self.set_phase(DragonPhase::ChargingPlayer);
                self.state.lock().phases.charge_target = Some(target.position());
            }
        }
    }

    /// The turn of vanilla `DragonSittingScanningPhase.doServerTick` toward a
    /// player outside its 10 degree view.
    fn turn_toward_while_perched(&self, target: DVec3) {
        let angle = heading_angle_to(self, target);
        if (0.0..=10.0).contains(&angle) {
            return;
        }
        let head = self.part(super::HEAD).position();
        let x_dist = target.x - head.x;
        let z_dist = target.z - head.z;
        let y_rot = self.rotation().0;
        let y_rot_delta = f64::from(wrap_degrees(
            (180.0 - x_dist.atan2(z_dist) * f64::from(RAD_TO_DEG) - f64::from(y_rot)) as f32,
        ))
        .clamp(-100.0, 100.0);
        let rot_speed = (x_dist * x_dist + z_dist * z_dist).sqrt() as f32 + 1.0;
        let dist = rot_speed.min(40.0);
        let y_rot_a = {
            let mut state = self.state.lock();
            state.y_rot_a *= 0.8;
            state.y_rot_a += y_rot_delta as f32 * (0.7 / dist / rot_speed);
            state.y_rot_a
        };
        self.set_rotation((y_rot + y_rot_a, self.rotation().1));
    }

    /// Vanilla `DragonSittingAttackingPhase.doServerTick`.
    fn tick_sitting_attacking(&self) {
        let ticks = {
            let mut state = self.state.lock();
            let ticks = state.phases.attacking_ticks;
            state.phases.attacking_ticks += 1;
            ticks
        };
        if ticks >= 40 {
            self.set_phase(DragonPhase::SittingFlaming);
        }
    }

    /// Vanilla `DragonChargePlayerPhase.doServerTick`.
    fn tick_charging_player(&self) {
        let (target, since) = {
            let state = self.state.lock();
            (state.phases.charge_target, state.phases.time_since_charge)
        };
        if target.is_none() {
            log::warn!("Aborting charge player as no target was set.");
            self.set_phase(DragonPhase::HoldingPattern);
        } else if since > 0 {
            self.state.lock().phases.time_since_charge += 1;
            if since >= 10 {
                self.set_phase(DragonPhase::HoldingPattern);
            }
        } else if self.should_retarget(target) {
            self.state.lock().phases.time_since_charge += 1;
        }
    }

    /// Vanilla `DragonDeathPhase.doServerTick`: fly home, then let the death
    /// animation start.
    fn tick_dying(&self, world: &Arc<World>) {
        let target = {
            let egg = self.podium_surface_motion_blocking(world);
            let mut state = self.state.lock();
            state.phases.death_time += 1;
            *state
                .phases
                .death_target
                .get_or_insert_with(|| super::bottom_center_of(egg))
        };
        let health = if self.should_retarget(Some(target)) {
            0.0
        } else {
            1.0
        };
        self.set_health(health);
    }
}
