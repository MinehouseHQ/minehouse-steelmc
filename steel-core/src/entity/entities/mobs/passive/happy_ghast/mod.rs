//! Vanilla `HappyGhast`: the friendly ghast that hatches from a dried ghast.
//! A ghastling follows players on its brain; a grown one floats around on
//! goals and, in a harness, carries up to four riders.

mod ai;
mod controls;

#[cfg(test)]
mod tests;

use std::mem;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_math::{DEG_TO_RAD, trig, wrap_degrees};
use steel_protocol::packets::game::{CEntityPositionSync, PositionPath, SoundSource};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::equipment::EquipmentSlot;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::HappyGhastEntityData;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{REGISTRY, TaggedRegistryExt as _, sound_events, vanilla_attributes};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, WorldAabb};

use self::controls::{HappyGhastFloatGoal, tick_happy_ghast_look_control};
use crate::behavior::{ITEM_BEHAVIORS, InteractionResult};
use crate::entity::ai::brain::{Brain, Memories};
use crate::entity::ai::flying_move::FlyingMoveControl;
use crate::entity::ai::goal::TemptGoal;
use crate::entity::ai::navigation::PathNavigation;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::hostile::ghast::flight::{
    GhastMoveState, RandomFloatAroundGoal, tick_ghast_move_control_with,
};
use crate::entity::leash::Leashable;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad,
    EntityEventSource as _, EntityPose, EntitySyncedData, LivingEntity, LivingEntityBase, Mob,
    MobBase, PathfinderMob, SharedEntity, sync_dirty_mob_effects,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::{LevelReader, World};

/// Vanilla `HappyGhast.BABY_SCALE`.
pub const BABY_SCALE: f32 = 0.2375;
/// Vanilla `BABY_DIMENSIONS` eye height.
const BABY_EYE_HEIGHT: f32 = 0.468_75;
/// Vanilla `HappyGhast.WANDER_GROUND_DISTANCE`.
const WANDER_GROUND_DISTANCE: i32 = 16;
/// Vanilla `SMALL_RESTRICTION_RADIUS`, for ghastlings and harnessed ghasts.
const SMALL_RESTRICTION_RADIUS: i32 = 32;
/// Vanilla `LARGE_RESTRICTION_RADIUS`, for unharnessed grown ghasts.
const LARGE_RESTRICTION_RADIUS: i32 = 64;
/// Vanilla `RESTRICTION_RADIUS_BUFFER`.
const RESTRICTION_RADIUS_BUFFER: i32 = 16;
/// Vanilla `FAST_HEALING_TICKS`: in clouds or rain.
const FAST_HEALING_TICKS: i32 = 20;
/// Vanilla `SLOW_HEALING_TICKS`.
const SLOW_HEALING_TICKS: i32 = 600;
/// Vanilla `MAX_PASSANGERS`.
pub const MAX_PASSENGERS: usize = 4;
/// Vanilla `STILL_TIMEOUT_ON_LOAD_GRACE_PERIOD`.
const STILL_TIMEOUT_ON_LOAD_GRACE_PERIOD: i32 = 60;
/// Vanilla `MAX_STILL_TIMEOUT`.
const MAX_STILL_TIMEOUT: i32 = 10;
/// Ticks a quad-leashed vehicle keeps the leash-holder flag up.
const LEASH_HOLDER_TIME: i32 = 5;
/// Vanilla `leashElasticDistance`.
const LEASH_ELASTIC_DISTANCE: f64 = 10.0;
/// Vanilla `leashSnapDistance`.
const LEASH_SNAP_DISTANCE: f64 = 16.0;
/// The tempt goal's speed and stop distance.
const TEMPT_SPEED: f64 = 1.0;
const TEMPT_STOP_DISTANCE: f64 = 7.0;
/// Vanilla `getRiddenInput`: backwards input is halved.
const BACKWARDS_INPUT_SCALE: f32 = -0.5;
/// Vanilla `getRiddenInput`: jumping lifts by this much.
const JUMP_LIFT: f32 = 0.5;
/// Vanilla `getRiddenInput`: the input is scaled by this times flying speed.
const RIDDEN_INPUT_SCALE: f32 = 3.9;
/// Vanilla `tickRidden`: how quickly the ghast turns toward its rider.
const RIDDEN_TURN_SPEED: f32 = 0.08;
/// Vanilla `getSoundVolume` for a grown ghast.
const ADULT_SOUND_VOLUME: f32 = 4.0;
/// Vanilla `getAmbientSoundInterval` multiplier while carrying riders.
const RIDDEN_AMBIENT_INTERVAL_FACTOR: i32 = 6;
/// The ghastling's `FlyingMoveControl(this, 180, true)`.
const BABY_MOVE_CONTROL: FlyingMoveControl = FlyingMoveControl {
    max_turn: 180,
    hovers_in_place: true,
};
/// Vanilla `BabyFlyingPathNavigation.setRequiredPathLength`.
const BABY_REQUIRED_PATH_LENGTH: f32 = 48.0;

/// Mutable server-side state of a happy ghast.
#[derive(Debug, Default)]
struct HappyGhastState {
    /// Vanilla `leashHolderTime`.
    leash_holder_time: i32,
    /// Vanilla `serverStillTimeout`.
    server_still_timeout: i32,
    move_state: GhastMoveState,
}

/// A happy ghast or ghastling.
#[entity_behavior(class = "HappyGhast")]
pub struct HappyGhastEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    entity_data: SyncMutex<HappyGhastEntityData>,
    brain: Brain<Self>,
    state: SyncMutex<HappyGhastState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `HappyGhastEntity`.
unsafe impl DowncastType for HappyGhastEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/happy_ghast");
}

impl HappyGhastEntity {
    /// Creates a new happy ghast.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved happy ghast.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mob_base = MobBase::new();
        AnimalBase::initialize_pathfinding_malus(&mob_base);
        let mut entity_data = HappyGhastEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        register_goals(&mob_base);
        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base: AgeableMobBase::new(),
            animal_base: AnimalBase::new(),
            entity_data: SyncMutex::new(entity_data),
            brain: Brain::new(&ai::memories(), ai::sensors(), ai::activities(), None),
            state: SyncMutex::new(HappyGhastState::default()),
        }
    }

    /// Vanilla `isOnStillTimeout`: held in place for a rider climbing on.
    #[must_use]
    pub fn is_on_still_timeout(&self) -> bool {
        self.stays_still() || self.state.lock().server_still_timeout > 0
    }

    /// Vanilla `staysStill`: the synced still flag.
    #[must_use]
    pub fn stays_still(&self) -> bool {
        *self.entity_data.lock().stays_still.get()
    }

    /// Vanilla `isLeashHolder`: the synced flag set while it holds a
    /// quad-leashed vehicle.
    #[must_use]
    pub fn is_leash_holder(&self) -> bool {
        *self.entity_data.lock().is_leash_holder.get()
    }

    /// Vanilla `serverStillTimeout`.
    #[must_use]
    pub fn server_still_timeout(&self) -> i32 {
        self.state.lock().server_still_timeout
    }

    /// Vanilla `setServerStillTimeout`: starting to stay still sends a full
    /// position so watchers see it stop exactly where it is.
    fn set_server_still_timeout(&self, timeout: i32) {
        let previous = mem::replace(&mut self.state.lock().server_still_timeout, timeout);
        if previous <= 0
            && timeout > 0
            && let Some(world) = self.level()
        {
            let position = self.position();
            world
                .entity_tracker()
                .sync_position_codec(self.id(), position);
            let (y_rot, x_rot) = self.rotation();
            world.broadcast_to_entity_trackers(
                self.id(),
                CEntityPositionSync {
                    entity_id: self.id(),
                    path: PositionPath::Linear(position),
                    y_rot,
                    x_rot,
                    on_ground: self.on_ground(),
                },
                None,
            );
        }
        self.entity_data.lock().stays_still.set(timeout > 0);
    }

    /// Vanilla `isWearingBodyArmor`: a harness in the body slot.
    #[must_use]
    pub fn is_wearing_harness(&self) -> bool {
        let mut wearing = false;
        self.with_equipment_slot(EquipmentSlot::Body, &mut |item_stack| {
            wearing = !item_stack.is_empty();
        });
        wearing
    }

    /// Vanilla `scanPlayerAboveGhast`: someone (not riding a happy ghast)
    /// stands on top of it.
    fn scan_player_above_ghast(&self, world: &World) -> bool {
        let bounding_box = self.bounding_box();
        let detection_box = WorldAabb::new(
            bounding_box.min_x() - 1.0,
            bounding_box.max_y() - f64::from(1.0e-5_f32),
            bounding_box.min_z() - 1.0,
            bounding_box.max_x() + 1.0,
            bounding_box.max_y() + bounding_box.height() / 2.0,
            bounding_box.max_z() + 1.0,
        );
        let mut found = false;
        world.players.iter_players(|_, player| {
            if !player.is_spectator() {
                let root = player.root_vehicle();
                let root_position = root
                    .as_ref()
                    .map_or_else(|| player.position(), |root| root.position());
                let rides_happy_ghast = root
                    .as_ref()
                    .is_some_and(|root| root.entity_type() == self.entity_type);
                found = !rides_happy_ghast && detection_box.contains(root_position);
            }
            !found
        });
        found
    }

    /// Vanilla `getHappyGhastRestrictionRadius`.
    fn restriction_radius(&self) -> i32 {
        if !AgeableMob::is_baby(self) && !self.is_wearing_harness() {
            LARGE_RESTRICTION_RADIUS
        } else {
            SMALL_RESTRICTION_RADIUS
        }
    }

    /// Vanilla `checkRestriction`: keeps a home around where it floats.
    fn check_restriction(&self) {
        if self.is_leashed() || self.is_vehicle() {
            return;
        }
        let radius = self.restriction_radius();
        let position = self.block_position();
        let near_home = self.has_home()
            && self
                .home_position()
                .closer_than(position, f64::from(radius + RESTRICTION_RADIUS_BUFFER));
        if !near_home || radius != self.home_radius() {
            self.set_home_to(position, radius);
        }
    }

    /// Vanilla `continuousHeal`: one heart point a second in clouds or
    /// rain, every thirty seconds otherwise.
    fn continuous_heal(&self) {
        let Some(world) = self.level() else {
            return;
        };
        #[expect(clippy::float_cmp, reason = "vanilla compares the health exactly")]
        let full_health = self.get_max_health() == self.get_health();
        if !Entity::is_alive(self) || self.living_base.death_time() != 0 || full_health {
            return;
        }
        let fast_healing = self.is_in_clouds() || world.has_precipitation_at(self.block_position());
        let interval = if fast_healing {
            FAST_HEALING_TICKS
        } else {
            SLOW_HEALING_TICKS
        };
        if self.tick_count() % interval == 0 {
            self.heal(1.0);
        }
    }

    /// Vanilla `tick`'s server half: the leash-holder flag and the still
    /// timeout that keeps it in place while someone climbs on.
    fn tick_server_state(&self, world: &World) {
        let leash_holder = {
            let mut state = self.state.lock();
            if state.leash_holder_time > 0 {
                state.leash_holder_time -= 1;
            }
            state.leash_holder_time > 0
        };
        self.entity_data.lock().is_leash_holder.set(leash_holder);
        let timeout = self.server_still_timeout();
        if timeout > 0 {
            let timeout = if self.tick_count() > STILL_TIMEOUT_ON_LOAD_GRACE_PERIOD {
                timeout - 1
            } else {
                timeout
            };
            self.set_server_still_timeout(timeout);
        }
        if self.scan_player_above_ghast(world) {
            self.set_server_still_timeout(MAX_STILL_TIMEOUT);
        }
    }

    /// Vanilla `adultGhastSetup` and `babyGhastSetup`: a ghastling flies on
    /// its brain with a flying navigation, a grown ghast on goals.
    fn setup_for_age(&self, baby: bool) {
        if baby {
            let mut navigation = PathNavigation::flying();
            navigation.set_can_open_doors(false);
            navigation.set_can_float(true);
            navigation.set_flies_through_fluids(true);
            let follow_range = self
                .attributes()
                .lock()
                .required_value(vanilla_attributes::FOLLOW_RANGE);
            navigation.set_required_path_length(BABY_REQUIRED_PATH_LENGTH, follow_range);
            *self.mob_base.navigation().lock() = navigation;
            self.set_server_still_timeout(0);
            self.remove_all_goals();
        } else {
            *self.mob_base.navigation().lock() = PathNavigation::new();
            self.remove_all_goals();
            register_goals(&self.mob_base);
            if let Some(world) = self.level() {
                self.brain.stop_all(&world, self);
                self.brain.clear_memories();
            }
        }
    }

    fn remove_all_goals(&self) {
        self.mob_base
            .goal_selector()
            .lock()
            .remove_all_goals(self, |_| true);
    }

    fn sound_for(&self, baby: SoundEventRef, adult: SoundEventRef) -> SoundEventRef {
        if AgeableMob::is_baby(self) {
            baby
        } else {
            adult
        }
    }
}

/// Vanilla `HappyGhast.registerGoals`.
fn register_goals(mob_base: &MobBase) {
    let mut goal_selector = mob_base.goal_selector().lock();
    goal_selector.add_goal(3, HappyGhastFloatGoal::new(mob_base));
    goal_selector.add_goal(
        4,
        TemptGoal::for_non_pathfinders(TEMPT_SPEED, is_tempted_by, false, TEMPT_STOP_DISTANCE),
    );
    goal_selector.add_goal(
        5,
        RandomFloatAroundGoal::with_distance_to_blocks(WANDER_GROUND_DISTANCE),
    );
}

/// The tempt goal's item check: an unharnessed grown ghast follows its tempt
/// items, the others only food.
fn is_tempted_by(mob: &dyn PathfinderMob, item_stack: &ItemStack) -> bool {
    let mut harnessed = false;
    mob.with_equipment_slot(EquipmentSlot::Body, &mut |body| {
        harnessed = !body.is_empty();
    });
    let tag = if !harnessed && !mob.is_baby() {
        &ItemTag::HAPPY_GHAST_TEMPT_ITEMS
    } else {
        &ItemTag::HAPPY_GHAST_FOOD
    };
    REGISTRY.items.is_in_tag(item_stack.item(), tag)
}

impl Entity for HappyGhastEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        let entity = Arc::clone(&self).into_shared_entity();
        self.tick_living_entity(&entity);
        if let Some(world) = self.level() {
            self.tick_server_state(&world);
        }
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let dimensions = if AgeableMob::is_baby(self) {
            let scaled = self.entity_type.dimensions.scale(BABY_SCALE);
            EntityDimensions::new_with_attachments(
                scaled.width,
                scaled.height,
                BABY_EYE_HEIGHT,
                scaled.attachments,
            )
        } else {
            self.entity_type.dimensions
        };
        dimensions.scale(LivingEntity::get_scale(self))
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn update_data_before_sync(&self) {
        if let Some(display) = sync_dirty_mob_effects(&self.living_base, &self.entity_data) {
            self.entity_data
                .set_base_glowing_flag(self.has_glowing_tag() || display.glowing);
        }
    }

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {}

    fn check_fall_damage(
        self: Arc<Self>,
        _vertical_movement: f64,
        _on_ground: bool,
        _on_state: BlockStateId,
        _pos: BlockPos,
        _world: &Arc<World>,
    ) {
    }

    fn on_climbable(&self) -> bool {
        false
    }

    fn controlling_passenger(&self) -> Option<SharedEntity> {
        if self.is_wearing_harness() && !self.is_on_still_timeout() {
            let first = self.first_passenger();
            if first
                .as_ref()
                .is_some_and(|first| first.as_player().is_some())
            {
                return first;
            }
        }
        self.controlling_passenger_mob()
    }

    fn can_add_passenger(&self, _passenger: &dyn Entity) -> bool {
        self.passengers().len() < MAX_PASSENGERS
    }

    /// Vanilla `getDismountLocationForPassenger`: riders step off on top.
    fn dismount_location_for_passenger(&self, _passenger: &dyn Entity) -> DVec3 {
        let position = self.position();
        DVec3::new(position.x, self.bounding_box().max_y(), position.z)
    }

    fn is_flying_vehicle(&self) -> bool {
        !AgeableMob::is_baby(self)
    }

    /// Vanilla `canBeCollidedWith`, server side: a grown ghast is solid
    /// while it stays still, and to other happy ghasts while it carries
    /// riders.
    fn can_be_collided_with(&self, other: Option<&dyn Entity>) -> bool {
        if AgeableMob::is_baby(self) || !Entity::is_alive(self) {
            return false;
        }
        if self.is_vehicle() && other.is_some_and(|other| other.entity_type() == self.entity_type) {
            return true;
        }
        self.is_on_still_timeout()
    }

    /// Vanilla `addPassenger`: lowers the goggles for the first rider and
    /// stops still unless someone stands on top.
    fn on_passenger_added(&self, was_vehicle: bool) {
        if !was_vehicle {
            self.play_sound(
                &sound_events::ENTITY_HAPPY_GHAST_HARNESS_GOGGLES_DOWN,
                1.0,
                1.0,
            );
        }
        let Some(world) = self.level() else {
            return;
        };
        if !self.scan_player_above_ghast(&world) {
            self.set_server_still_timeout(0);
        } else if self.server_still_timeout() > MAX_STILL_TIMEOUT {
            self.set_server_still_timeout(MAX_STILL_TIMEOUT);
        }
    }

    /// Vanilla `removePassenger`: waits for the rider to step off, and
    /// lifts the goggles once it carries nobody.
    fn on_passenger_removed(&self) {
        self.set_server_still_timeout(MAX_STILL_TIMEOUT);
        if !self.is_vehicle() {
            self.clear_home();
            self.play_sound(
                &sound_events::ENTITY_HAPPY_GHAST_HARNESS_GOGGLES_UP,
                1.0,
                1.0,
            );
        }
    }

    fn supports_quad_leash_as_holder(&self) -> bool {
        true
    }

    /// Vanilla `notifyLeashHolder`: holding a quad-leashed vehicle raises
    /// the leash-holder flag for a few ticks.
    fn notify_leash_holder(&self, leashable: &dyn Entity) {
        if leashable
            .as_leashable()
            .is_some_and(Leashable::supports_quad_leash)
        {
            self.state.lock().leash_holder_time = LEASH_HOLDER_TIME;
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        nbt.insert("still_timeout", self.server_still_timeout());
        nbt.insert("Brain", self.brain.save());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        self.set_server_still_timeout(nbt.int("still_timeout").unwrap_or(0));
        if let Some(brain) = nbt.compound("Brain") {
            self.brain.memories().lock().load(brain);
        }
    }
}

impl LivingEntity for HappyGhastEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn brain_memories(&self) -> Option<&SyncMutex<Memories>> {
        Some(self.brain.memories())
    }

    fn get_health(&self) -> f32 {
        *self.entity_data.lock().living_entity().health.get()
    }

    fn set_health(&self, health: f32) {
        let clamped = health.clamp(0.0, self.get_max_health());
        self.entity_data
            .lock()
            .living_entity_mut()
            .health
            .set(clamped);
    }

    /// Vanilla `getScale` with `sanitizeScale`: never larger than normal.
    fn get_scale(&self) -> f32 {
        let scale = self
            .attributes()
            .lock()
            .get_value(vanilla_attributes::SCALE)
            .unwrap_or(1.0) as f32;
        scale.min(1.0)
    }

    fn get_age_scale(&self) -> f32 {
        if AgeableMob::is_baby(self) {
            BABY_SCALE
        } else {
            1.0
        }
    }

    fn sound_volume(&self) -> f32 {
        if AgeableMob::is_baby(self) {
            1.0
        } else {
            ADULT_SOUND_VOLUME
        }
    }

    fn voice_pitch(&self) -> f32 {
        1.0
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(self.sound_for(
            &sound_events::ENTITY_GHASTLING_HURT,
            &sound_events::ENTITY_HAPPY_GHAST_HURT,
        ))
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(self.sound_for(
            &sound_events::ENTITY_GHASTLING_DEATH,
            &sound_events::ENTITY_HAPPY_GHAST_DEATH,
        ))
    }

    fn can_breathe_underwater(&self) -> bool {
        AgeableMob::is_baby(self) || self.entity_type.flags.can_breathe_underwater
    }

    fn can_use_slot(&self, slot: EquipmentSlot) -> bool {
        slot != EquipmentSlot::Body || (Entity::is_alive(self) && !AgeableMob::is_baby(self))
    }

    fn can_dispenser_equip_into_slot(&self, slot: EquipmentSlot) -> bool {
        slot == EquipmentSlot::Body
    }

    /// Vanilla `travel`: always flies.
    fn travel(&self, entity: &SharedEntity, input: DVec3) -> Option<MoveResult> {
        let speed = self.flying_speed() * 5.0 / 3.0;
        self.travel_flying(entity, input, speed, speed, speed)
    }

    /// Vanilla `getRiddenInput`: flies where the rider looks, slower
    /// backwards, and up while jumping.
    fn ridden_input(&self, controller: &Player, _self_input: DVec3) -> DVec3 {
        let input = controller.last_client_input();
        let strafe = input.left_intent();
        let mut forward = 0.0;
        let mut up = 0.0;
        if input.forward_intent() != 0.0 {
            let pitch = controller.rotation().1 * DEG_TO_RAD;
            let mut forward_look = trig::cos(pitch);
            let mut up_look = -trig::sin(pitch);
            if input.forward_intent() < 0.0 {
                forward_look *= BACKWARDS_INPUT_SCALE;
                up_look *= BACKWARDS_INPUT_SCALE;
            }
            up = up_look;
            forward = forward_look;
        }
        if input.jump() {
            up += JUMP_LIFT;
        }
        let flying_speed = self
            .attributes()
            .lock()
            .required_value(vanilla_attributes::FLYING_SPEED);
        DVec3::new(f64::from(strafe), f64::from(up), f64::from(forward))
            * (f64::from(RIDDEN_INPUT_SCALE) * flying_speed)
    }

    /// Vanilla `tickRidden`: slowly turns to where the rider looks.
    fn tick_ridden(&self, controller: &Player, _ridden_input: DVec3) {
        let (rider_yaw, rider_pitch) = controller.rotation();
        let mut yaw = self.rotation().0;
        yaw += wrap_degrees(rider_yaw - yaw) * RIDDEN_TURN_SPEED;
        self.set_rotation((yaw, rider_pitch * 0.5));
        self.base().set_old_yaw_to_current();
        self.set_y_body_rot(yaw);
        self.set_y_head_rot(yaw);
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    /// Vanilla `aiStep`: watchers get exact positions while it stays
    /// still, and it heals over time.
    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        self.base
            .set_requires_precise_position(self.is_on_still_timeout());
        let result = Mob::mob_ai_step(self, entity);
        AgeableMob::tick_ageable_mob(self);
        Animal::tick_animal_love(self);
        self.continuous_heal();
        result
    }
}

impl HappyGhastEntity {
    fn flying_speed(&self) -> f32 {
        self.attributes()
            .lock()
            .required_value(vanilla_attributes::FLYING_SPEED) as f32
    }
}

impl AgeableMob for HappyGhastEntity {
    fn ageable_base(&self) -> &AgeableMobBase {
        &self.ageable_base
    }

    fn is_age_locked(&self) -> bool {
        *self.entity_data.lock().ageable_mob().age_locked.get()
    }

    fn set_age_locked(&self, age_locked: bool) {
        self.entity_data
            .lock()
            .ageable_mob_mut()
            .age_locked
            .set(age_locked);
    }

    fn set_synced_baby(&self, baby: bool) {
        self.entity_data.lock().ageable_mob_mut().baby.set(baby);
    }

    /// Vanilla `ageBoundaryReached`.
    fn age_boundary_changed(&self, baby: bool) {
        self.setup_for_age(baby);
        self.refresh_dimensions();
    }
}

impl Animal for HappyGhastEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::HAPPY_GHAST_FOOD)
    }

    /// Vanilla `canFallInLove`: happy ghasts never breed.
    fn can_fall_in_love(&self) -> bool {
        false
    }
}

impl Mob for HappyGhastEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// A ghastling's `FlyingMoveControl`, or a grown ghast's careful
    /// `GhastMoveControl` that holds still on its timeout.
    fn tick_move_control(&self) {
        if AgeableMob::is_baby(self) {
            BABY_MOVE_CONTROL.tick(self);
            return;
        }
        let stopped = self.is_on_still_timeout();
        let mut state = self.state.lock();
        tick_ghast_move_control_with(self, &mut state.move_state, true, stopped);
    }

    fn tick_look_control(&self) {
        if AgeableMob::is_baby(self) {
            self.default_tick_look_control();
        } else {
            tick_happy_ghast_look_control(self);
        }
    }

    /// Vanilla `HappyGhastBodyRotationControl`: a ridden ghast faces where
    /// it flies.
    fn tick_body_rotation_control(&self) {
        if self.is_vehicle() {
            let yaw = self.rotation().0;
            self.set_y_head_rot(yaw);
            self.set_y_body_rot(yaw);
        }
        self.default_tick_body_rotation_control();
    }

    fn custom_server_ai_step(&self) {
        if AgeableMob::is_baby(self)
            && let Some(world) = self.level()
        {
            self.brain.tick(&world, self);
            ai::update_activity(self);
        }
        self.check_restriction();
        Animal::custom_server_ai_step_animal(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(self.sound_for(
            &sound_events::ENTITY_GHASTLING_AMBIENT,
            &sound_events::ENTITY_HAPPY_GHAST_AMBIENT,
        ))
    }

    fn ambient_sound_interval(&self) -> i32 {
        let interval = self.ambient_sound_interval_animal();
        if self.is_vehicle() {
            interval * RIDDEN_AMBIENT_INTERVAL_FACTOR
        } else {
            interval
        }
    }

    fn max_spawn_cluster_size(&self) -> i32 {
        1
    }

    /// Vanilla `mobInteract`: a harnessed ghast takes the player on board.
    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        if AgeableMob::is_baby(self) {
            return Animal::mob_interact_animal(self, player, hand);
        }
        let held_item = {
            let inventory = player.inventory.lock();
            let item_stack = inventory.get_item_in_hand(hand);
            (!item_stack.is_empty()).then(|| item_stack.item())
        };
        if let Some(held_item) = held_item {
            let result = self.interact_living_entity_with_equippable(player, hand);
            if result.consumes_action() {
                return result;
            }
            let result = {
                let mut inventory = player.inventory.lock();
                let held = inventory.get_item_in_hand_mut(hand);
                ITEM_BEHAVIORS
                    .get_behavior(held_item)
                    .interact_living_entity(held, player, self, hand)
            };
            if result.consumes_action() {
                return result;
            }
        }
        if self.is_wearing_harness() && !player.is_secondary_use_active() {
            if let Some(world) = self.level()
                && let Some(ghast) = world.get_entity_by_id(self.id())
            {
                player.start_riding(&ghast, false);
            }
            return InteractionResult::Success;
        }
        Animal::mob_interact_animal(self, player, hand)
    }

    /// Vanilla `onElasticLeashPull`: a pulled ghast stops steering.
    fn mob_on_elastic_leash_pull(&self) {
        self.mob_base.controls().lock().move_control.set_wait();
    }

    fn mob_leash_snap_distance(&self) -> f64 {
        LEASH_SNAP_DISTANCE
    }

    fn mob_leash_elastic_distance(&self) -> f64 {
        LEASH_ELASTIC_DISTANCE
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for HappyGhastEntity {
    /// Vanilla `getWalkTargetValue`: open air, best right above the ground.
    fn get_walk_target_value_in(&self, pos: BlockPos, level: &dyn LevelReader) -> f32 {
        if !level.get_block_state(pos).is_air() {
            return 0.0;
        }
        if level.get_block_state(pos.below()).is_air()
            && !level.get_block_state(pos.below_n(2)).is_air()
        {
            10.0
        } else {
            5.0
        }
    }

    fn should_stay_close_to_leash_holder(&self) -> bool {
        false
    }
}
