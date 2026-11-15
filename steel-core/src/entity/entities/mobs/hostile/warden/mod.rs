//! Vanilla `Warden`: the blind guardian of the deep dark, summoned by sculk
//! shriekers and drawn by vibrations.

mod ai;
mod anger;
mod sensor;
#[cfg(test)]
mod tests;

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::game_events::GameEventRef;
use steel_registry::position_source::{EntityPositionSource, PositionSource};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::WardenEntityData;
use steel_registry::vanilla_game_event_tags::GameEventTag;
use steel_registry::{
    sound_events, vanilla_entities, vanilla_mob_effects, vanilla_position_source_types,
};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier};

pub use anger::{AngerLevel, AngerManagement};

use crate::enchantment_helper;
use crate::entity::ai::brain::memory_types::{
    ATTACK_TARGET, CANT_REACH_WALK_TARGET_SINCE, DIG_COOLDOWN, IS_EMERGING,
    NEAREST_VISIBLE_NEMESIS, RECENT_PROJECTILE, ROAR_TARGET, TOUCH_COOLDOWN, VIBRATION_COOLDOWN,
};
use crate::entity::ai::brain::sensors::PlayerSensor;
use crate::entity::ai::brain::{AnyMemoryModuleType, Brain, Memories, Sensor};
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::entity::EntityEventSource as _;
use crate::entity::entity_selector::no_creative_or_spectator;
use crate::entity::mob_effect::add_effect_to_players_around;
use crate::entity::pushing::EntityPush;
use crate::entity::spawn_placements::spawn_aabb;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityMovementEmission, EntityPose, EntitySpawnReason,
    EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase, MobEffectInstance, Monster,
    PathfinderMob, SharedEntity, SpawnGroupData, SpawnRule, level_contains_any_liquid,
    sync_dirty_mob_effects,
};
use crate::physics::MoveResult;
use crate::physics::collision::{WorldCollisionProvider, has_collision};
use crate::world::World;
use crate::world::explosion::Explosion;
use crate::world::game_event::vibrations::{
    VIBRATION_LISTENER_TAG, VibrationData, VibrationListener, VibrationSystem, VibrationUser,
    tick_vibrations,
};
use crate::world::game_event::{DynamicGameEventListener, DynamicListenerAction, GameEventContext};
use crate::world::{LevelReader, ServerLevelAccessor};

/// Vanilla `xpReward`.
const XP_REWARD: i32 = 5;
/// Vanilla `VIBRATION_COOLDOWN_TICKS`.
const VIBRATION_COOLDOWN_TICKS: i64 = 40;
/// Vanilla `TIME_TO_USE_MELEE_UNTIL_SONIC_BOOM`.
const TIME_TO_USE_MELEE_UNTIL_SONIC_BOOM: i64 = 200;
/// Vanilla `DARKNESS_DISPLAY_LIMIT`.
const DARKNESS_DISPLAY_LIMIT: i32 = 200;
/// Vanilla `DARKNESS_DURATION`.
const DARKNESS_DURATION: i32 = 260;
/// Vanilla `DARKNESS_RADIUS`.
const DARKNESS_RADIUS: i32 = 20;
/// Vanilla `DARKNESS_INTERVAL`.
const DARKNESS_INTERVAL: i32 = 120;
/// Vanilla `ANGERMANAGEMENT_TICK_DELAY`.
const ANGER_MANAGEMENT_TICK_DELAY: i32 = 20;
/// Vanilla `DEFAULT_ANGER`.
const DEFAULT_ANGER: i32 = 35;
/// Vanilla `PROJECTILE_ANGER`.
const PROJECTILE_ANGER: i32 = 10;
/// Vanilla `ON_HURT_ANGER_BOOST`.
const ON_HURT_ANGER_BOOST: i32 = 20;
/// Vanilla `RECENT_PROJECTILE_TICK_THRESHOLD`.
const RECENT_PROJECTILE_TICK_THRESHOLD: i64 = 100;
/// Vanilla `TOUCH_COOLDOWN_TICKS`.
const TOUCH_COOLDOWN_TICKS: i64 = 20;
/// Vanilla `PROJECTILE_ANGER_DISTANCE`.
const PROJECTILE_ANGER_DISTANCE: f64 = 30.0;
/// Vanilla `VibrationUser.GAME_EVENT_LISTENER_RANGE`.
const GAME_EVENT_LISTENER_RANGE: i32 = 16;
/// Vanilla `doHurtTarget`: a melee hit holds the sonic boom back.
const MELEE_SONIC_BOOM_COOLDOWN: i64 = 40;
/// Vanilla `hurtServer`: an attacker this close becomes the target even
/// through a projectile.
const CLOSE_ATTACKER_DISTANCE: f64 = 5.0;
/// Vanilla `getSoundVolume`.
const SOUND_VOLUME: f32 = 4.0;
/// Vanilla `getSecondsToDisableBlocking`.
const SECONDS_TO_DISABLE_BLOCKING: f32 = 5.0;
/// Vanilla `nextStep`: `moveDist + 0.55`.
const STEP_DISTANCE: f32 = 0.55;
/// Height while digging or emerging (vanilla `getDefaultDimensions`).
const BURIED_HEIGHT: f32 = 1.0;

/// The memories vanilla's `BRAIN_PROVIDER` lists, besides the ones its
/// sensors and behaviors register.
static MEMORY_TYPES: [&dyn AnyMemoryModuleType; 4] = [
    &NEAREST_VISIBLE_NEMESIS,
    &RECENT_PROJECTILE,
    &TOUCH_COOLDOWN,
    &VIBRATION_COOLDOWN,
];

/// A warden.
#[entity_behavior(class = "Warden", spawn_rule)]
pub struct WardenEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<WardenEntityData>,
    brain: Brain<Self>,
    anger: SyncMutex<AngerManagement>,
    vibration_data: SyncMutex<VibrationData>,
    dynamic_vibration_listener: DynamicGameEventListener,
}

// SAFETY: The owner-scoped type key uniquely identifies WardenEntity.
unsafe impl DowncastType for WardenEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/warden");
}

impl WardenEntity {
    /// Vanilla `SpawnPlacements` predicate: `Monster.checkMonsterSpawnRules`.
    pub(crate) const SPAWN_RULE: SpawnRule = |entity_type, level, spawn_reason, pos, random| {
        <Self as Monster>::check_monster_spawn_rules(entity_type, level, spawn_reason, pos, random)
    };

    /// Creates a new warden.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved warden.
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
        mob_base.set_xp_reward(XP_REWARD);
        {
            let mut navigation = mob_base.navigation().lock();
            navigation.set_can_float(true);
            navigation.set_horizontal_step_distance(true);
        }
        let mut entity_data = WardenEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        let warden = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            brain: Brain::new(&MEMORY_TYPES, Self::sensors(), ai::activities(), None),
            anger: SyncMutex::new(AngerManagement::default()),
            vibration_data: SyncMutex::new(VibrationData::default()),
            dynamic_vibration_listener: DynamicGameEventListener::new(),
        };
        warden.set_pathfinding_malus(PathType::UnpassableRail, 0.0);
        warden.set_pathfinding_malus(PathType::Damaging, 8.0);
        warden.set_pathfinding_malus(PathType::PowderSnow, 8.0);
        warden.set_pathfinding_malus(PathType::Lava, 8.0);
        warden.set_pathfinding_malus(PathType::Fire, 0.0);
        warden.set_pathfinding_malus(PathType::FireInNeighbor, 0.0);
        warden
    }

    fn sensors() -> Vec<Box<dyn Sensor<Self>>> {
        vec![Box::new(PlayerSensor), Box::new(sensor::WardenEntitySensor)]
    }

    /// The warden's brain memories.
    #[must_use]
    pub const fn memories(&self) -> &SyncMutex<Memories> {
        self.brain.memories()
    }

    /// The warden's grudges.
    #[must_use]
    pub const fn anger_management(&self) -> &SyncMutex<AngerManagement> {
        &self.anger
    }

    /// Vanilla `isDiggingOrEmerging`.
    fn is_digging_or_emerging(&self) -> bool {
        matches!(self.pose(), EntityPose::Digging | EntityPose::Emerging)
    }

    /// Vanilla `getClientAngerLevel`.
    #[must_use]
    pub fn client_anger_level(&self) -> i32 {
        *self.entity_data.lock().client_anger_level.get()
    }

    /// Vanilla `syncClientAngerLevel`.
    fn sync_client_anger_level(&self) {
        let anger = self.active_anger();
        self.entity_data.lock().client_anger_level.set(anger);
    }

    /// Vanilla `canTargetEntity`.
    #[must_use]
    pub fn can_target_entity(&self, entity: &dyn Entity) -> bool {
        let Some(living) = entity.as_living_entity() else {
            return false;
        };
        let same_level = match (self.level(), entity.level()) {
            (Some(own), Some(other)) => Arc::ptr_eq(&own, &other),
            _ => false,
        };
        same_level
            && no_creative_or_spectator(entity)
            && !self.is_allied_to(entity)
            && entity.entity_type() != &vanilla_entities::ARMOR_STAND
            && entity.entity_type() != &vanilla_entities::WARDEN
            && !entity.is_invulnerable()
            && !living.is_dead_or_dying()
            && self.level().is_some_and(|world| {
                world
                    .world_border_snapshot()
                    .is_within_bounds(entity.bounding_box())
            })
    }

    /// Vanilla `applyDarknessAround`: darkness for the players within
    /// `radius` of `position`, shared with the sculk shrieker.
    pub(crate) fn apply_darkness_around(
        world: &World,
        position: DVec3,
        source: Option<&dyn Entity>,
        radius: i32,
    ) {
        add_effect_to_players_around(
            world,
            source,
            position,
            f64::from(radius),
            &MobEffectInstance::with_duration(vanilla_mob_effects::DARKNESS, DARKNESS_DURATION, 0)
                .with_ambient(false)
                .with_visible(false)
                .with_show_icon(false),
            DARKNESS_DISPLAY_LIMIT,
        );
    }

    /// Vanilla `getAngerLevel`.
    #[must_use]
    pub fn anger_level(&self) -> AngerLevel {
        AngerLevel::by_anger(self.active_anger())
    }

    /// Vanilla `getActiveAnger`: the anger at the target, or the highest one.
    fn active_anger(&self) -> i32 {
        let target = Mob::target(self);
        self.anger.lock().active_anger(target.as_deref())
    }

    /// Vanilla `clearAnger`.
    pub fn clear_anger(&self, entity: &dyn Entity) {
        self.anger.lock().clear_anger(entity);
    }

    /// Vanilla `increaseAngerAt(entity)`.
    pub fn increase_anger_at(&self, entity: Option<&SharedEntity>) {
        self.increase_anger_at_by(entity, DEFAULT_ANGER, true);
    }

    /// Vanilla `increaseAngerAt(entity, amount, playSound)`.
    pub fn increase_anger_at_by(
        &self,
        entity: Option<&SharedEntity>,
        amount: i32,
        play_sound: bool,
    ) {
        let Some(entity) = entity else {
            return;
        };
        if self.is_no_ai() || !self.can_target_entity(entity.as_ref()) {
            return;
        }
        ai::set_dig_cooldown(self);
        let maybe_switch_target = !Mob::target(self)
            .is_some_and(|target| target.entity_type() == &vanilla_entities::PLAYER);
        let new_anger = self.anger.lock().increase_anger(entity, amount);
        if entity.entity_type() == &vanilla_entities::PLAYER
            && maybe_switch_target
            && AngerLevel::by_anger(new_anger).is_angry()
        {
            self.brain.memories().lock().erase(&ATTACK_TARGET);
        }
        if play_sound {
            self.play_listening_sound();
        }
    }

    /// Vanilla `playListeningSound`.
    fn play_listening_sound(&self) {
        if self.pose() != EntityPose::Roaring {
            let pitch = self.voice_pitch();
            self.play_sound(self.anger_level().listening_sound(), 10.0, pitch);
        }
    }

    /// Vanilla `getEntityAngryAt`: the top suspect, once angry.
    #[must_use]
    pub fn entity_angry_at(&self) -> Option<SharedEntity> {
        if !self.anger_level().is_angry() {
            return None;
        }
        self.anger
            .lock()
            .active_entity(|entity| self.can_target_entity(entity.as_ref()))
    }

    /// Vanilla `setAttackTarget`.
    pub fn set_attack_target(&self, target: SharedEntity) {
        {
            let mut memories = self.brain.memories().lock();
            memories.erase(&ROAR_TARGET);
            memories.set(&ATTACK_TARGET, Some(target));
            memories.erase(&CANT_REACH_WALK_TARGET_SINCE);
        }
        ai::set_sonic_boom_cooldown(self, TIME_TO_USE_MELEE_UNTIL_SONIC_BOOM);
    }

    /// Vanilla `doPush`: bumping into the warden angers it.
    fn on_push(&self, entity: &SharedEntity) {
        if self.is_no_ai() {
            return;
        }
        {
            let mut memories = self.brain.memories().lock();
            if memories.has_value(&TOUCH_COOLDOWN) {
                return;
            }
            memories.set_with_expiry(&TOUCH_COOLDOWN, (), TOUCH_COOLDOWN_TICKS);
        }
        self.increase_anger_at(Some(entity));
        ai::set_disturbance_location(self, entity.block_position());
    }

    fn update_dirty_mob_effect_entity_data(&self) {
        sync_dirty_mob_effects(&self.living_base, &self.entity_data);
    }
}

impl Entity for WardenEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn movement_emission(&self) -> EntityMovementEmission {
        EntityMovementEmission::All
    }

    /// Vanilla `Warden.tick`: vibrations travel, and a persistent warden
    /// never digs away.
    fn tick(self: Arc<Self>) {
        if let Some(world) = self.level() {
            tick_vibrations(&world, self.as_ref());
            if self.is_persistence_required() || self.requires_custom_persistence() {
                ai::set_dig_cooldown(&self);
            }
        }
        let entity = Arc::clone(&self).into_shared_entity();
        self.tick_living_entity(&entity);
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    /// Vanilla `getDefaultDimensions`: one block tall while underground.
    fn dimensions_for_pose(&self, pose: EntityPose) -> EntityDimensions {
        let dimensions = self.entity_type.dimensions;
        if matches!(pose, EntityPose::Digging | EntityPose::Emerging) {
            EntityDimensions::with_default_eye_height(dimensions.width, BURIED_HEIGHT)
        } else if self.entity_type.fixed {
            dimensions
        } else {
            dimensions.scale(LivingEntity::get_scale(self))
        }
    }

    /// Vanilla `getAddEntityPacket`: an emerging warden spawns underground.
    fn spawn_data(&self) -> i32 {
        i32::from(self.pose() == EntityPose::Emerging)
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn update_data_before_sync(&self) {
        self.update_dirty_mob_effect_entity_data();
    }

    fn sound_source(&self) -> SoundSource {
        Monster::sound_source_monster(self)
    }

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        Monster::fall_sounds_monster(self)
    }

    fn swim_sound(&self) -> SoundEventRef {
        Monster::swim_sound_monster(self)
    }

    fn swim_splash_sound(&self) -> SoundEventRef {
        Monster::swim_splash_sound_monster(self)
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(&sound_events::ENTITY_WARDEN_STEP, 10.0, 1.0);
    }

    fn next_step(&self) -> f32 {
        self.base().movement_progress().move_dist() + STEP_DISTANCE
    }

    fn can_ride(&self, _vehicle: &dyn Entity) -> bool {
        false
    }

    fn dampens_vibrations(&self) -> bool {
        true
    }

    fn ignore_explosion(&self, _explosion: &dyn Explosion) -> bool {
        self.is_digging_or_emerging()
    }

    fn is_pushable(&self) -> bool {
        !self.is_digging_or_emerging()
            && LivingEntity::is_alive(self)
            && !self.is_spectator()
            && !self.on_climbable()
    }

    fn as_vibration_system(&self) -> Option<&dyn VibrationSystem> {
        Some(self)
    }

    fn update_dynamic_game_event_listeners(
        &self,
        world: &Arc<World>,
        this: &SharedEntity,
        action: DynamicListenerAction,
    ) {
        self.dynamic_vibration_listener
            .apply(world, action, || Arc::new(VibrationListener::new(this)));
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("Brain", self.brain.save());
        nbt.insert("anger", self.anger.lock().save());
        nbt.insert(VIBRATION_LISTENER_TAG, self.vibration_data.lock().save());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        if let Some(brain) = nbt.compound("Brain") {
            self.brain.memories().lock().load(brain);
        }
        *self.anger.lock() = nbt
            .compound("anger")
            .map_or_else(AngerManagement::default, AngerManagement::load);
        self.sync_client_anger_level();
        *self.vibration_data.lock() = nbt
            .compound(VIBRATION_LISTENER_TAG)
            .and_then(VibrationData::load)
            .unwrap_or_default();
    }
}

impl LivingEntity for WardenEntity {
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
        let max_health = self.get_max_health();
        self.entity_data
            .lock()
            .living_entity_mut()
            .health
            .set(health.clamp(0.0, max_health));
    }

    fn sound_volume(&self) -> f32 {
        SOUND_VOLUME
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_WARDEN_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_WARDEN_DEATH)
    }

    fn seconds_to_disable_blocking(&self) -> f32 {
        SECONDS_TO_DISABLE_BLOCKING
    }

    /// Vanilla `isInvulnerableTo`: nothing but the void hurts a buried warden.
    fn is_invulnerable_to(&self, _world: &World, source: &DamageSource) -> bool {
        (self.is_digging_or_emerging() && !source.bypasses_invulnerability())
            || self.default_is_invulnerable_to(source)
            || enchantment_helper::is_immune_to_damage(self, source)
    }

    fn hurt_server(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        let was_hurt = self.default_hurt_server(world, source, amount);
        if !self.is_no_ai() && !self.is_digging_or_emerging() {
            let attacker = source.causing_entity();
            self.increase_anger_at_by(
                attacker,
                AngerLevel::Angry.minimum_anger() + ON_HURT_ANGER_BOOST,
                false,
            );
            let has_target = self.brain.memories().lock().has_value(&ATTACK_TARGET);
            if !has_target
                && let Some(attacker) = attacker
                && attacker.as_living_entity().is_some()
                && (source.is_direct()
                    || attacker.position().distance_squared(self.position())
                        < CLOSE_ATTACKER_DISTANCE * CLOSE_ATTACKER_DISTANCE)
            {
                self.set_attack_target(SharedEntity::clone(attacker));
            }
        }
        was_hurt
    }

    fn should_drop_experience(&self) -> bool {
        Monster::should_drop_experience_monster(self)
    }

    fn should_drop_loot(&self, world: &World) -> bool {
        Monster::should_drop_loot_monster(self, world)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        Monster::monster_ai_step(self, entity)
    }

    /// Vanilla `doPush` runs for each pushed entity before the push itself.
    fn push_entities_in_order(&self, entities: &[SharedEntity]) {
        for entity in entities {
            self.on_push(entity);
        }
        EntityPush::push_entities_in_order(self, entities);
    }
}

impl Mob for WardenEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `getTarget`: the brain's attack target.
    fn target(&self) -> Option<SharedEntity> {
        self.brain.memories().lock().get(&ATTACK_TARGET)
    }

    /// Vanilla `canAttack`.
    fn can_attack(&self, target: &dyn LivingEntity) -> bool {
        self.can_target_entity(target.as_entity_event_source())
    }

    fn custom_server_ai_step(&self) {
        let Some(world) = self.level() else {
            return;
        };
        self.brain.tick(&world, self);
        let tick_count = self.tick_count();
        if (tick_count + self.id()) % DARKNESS_INTERVAL == 0 {
            Self::apply_darkness_around(
                &world,
                self.position(),
                Some(self.as_entity_event_source()),
                DARKNESS_RADIUS,
            );
        }
        if tick_count % ANGER_MANAGEMENT_TICK_DELAY == 0 {
            self.anger
                .lock()
                .tick(&world, |entity| self.can_target_entity(entity.as_ref()));
            self.sync_client_anger_level();
        }
        ai::update_activity(self);
    }

    fn do_hurt_target(&self, entity: &SharedEntity, world: &World, target: &SharedEntity) -> bool {
        self.broadcast_entity_event(EntityStatus::StartAttacking);
        let pitch = self.voice_pitch();
        self.play_sound(&sound_events::ENTITY_WARDEN_ATTACK_IMPACT, 10.0, pitch);
        ai::set_sonic_boom_cooldown(self, MELEE_SONIC_BOOM_COOLDOWN);
        self.default_do_hurt_target(entity, world, target)
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        (self.pose() != EntityPose::Roaring && !self.is_digging_or_emerging())
            .then(|| self.anger_level().ambient_sound())
    }

    fn remove_when_far_away(&self, _dist_sqr: f64) -> bool {
        false
    }

    /// Vanilla `checkSpawnObstruction`: also room for the full-height warden.
    fn check_spawn_obstruction(&self, level: &dyn ServerLevelAccessor) -> bool {
        let position = self.position();
        let spawn_box = spawn_aabb(self.entity_type, position.x, position.y, position.z);
        level.is_unobstructed_by_entities(self)
            && !level_contains_any_liquid(level, self.bounding_box())
            && self.level().is_some_and(|world| {
                !has_collision(&WorldCollisionProvider::for_entity(&world, self), spawn_box)
            })
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.brain
            .memories()
            .lock()
            .set_with_expiry(&DIG_COOLDOWN, (), ai::DIGGING_COOLDOWN);
        if spawn_reason == EntitySpawnReason::Triggered {
            self.set_pose(EntityPose::Emerging);
            self.brain.memories().lock().set_with_expiry(
                &IS_EMERGING,
                (),
                i64::from(ai::EMERGE_DURATION),
            );
            self.play_sound(&sound_events::ENTITY_WARDEN_AGITATED, 5.0, 1.0);
        }
        self.finalize_spawn_mob_base(world, spawn_reason, group_data)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for WardenEntity {
    /// Vanilla `getWalkTargetValue`: the warden has no favorite ground.
    fn get_walk_target_value_in(&self, _pos: BlockPos, _level: &dyn LevelReader) -> f32 {
        0.0
    }
}

impl Monster for WardenEntity {}

/// Vanilla `Warden.VibrationUser`.
impl VibrationUser for WardenEntity {
    fn vibration_listener_radius(&self) -> i32 {
        GAME_EVENT_LISTENER_RANGE
    }

    fn vibration_position_source(&self) -> PositionSource {
        PositionSource::new(
            &vanilla_position_source_types::ENTITY,
            EntityPositionSource::new(self.id(), self.get_eye_height() as f32),
        )
    }

    fn vibration_listener_pos(&self) -> Option<DVec3> {
        Some(self.position() + DVec3::new(0.0, self.get_eye_height(), 0.0))
    }

    fn can_receive_vibration(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        _event: GameEventRef,
        context: &GameEventContext<'_>,
    ) -> bool {
        !self.is_no_ai()
            && !self.is_dead_or_dying()
            && !self.brain.memories().lock().has_value(&VIBRATION_COOLDOWN)
            && !self.is_digging_or_emerging()
            && world.world_border_snapshot().is_block_within_bounds(pos)
            && !context.source_entity().is_some_and(|source| {
                source.as_living_entity().is_some() && !self.can_target_entity(source)
            })
    }

    fn on_receive_vibration(
        &self,
        _world: &Arc<World>,
        pos: BlockPos,
        _event: GameEventRef,
        source_entity: Option<&SharedEntity>,
        projectile_owner: Option<&SharedEntity>,
        _receiving_distance: f32,
    ) {
        if self.is_dead_or_dying() {
            return;
        }
        self.brain.memories().lock().set_with_expiry(
            &VIBRATION_COOLDOWN,
            (),
            VIBRATION_COOLDOWN_TICKS,
        );
        self.broadcast_entity_event(EntityStatus::TendrilsShiver);
        let pitch = self.voice_pitch();
        self.play_sound(&sound_events::ENTITY_WARDEN_TENDRIL_CLICKS, 5.0, pitch);
        let mut suspicious_pos = pos;
        if let Some(owner) = projectile_owner {
            if owner.position().distance_squared(self.position())
                < PROJECTILE_ANGER_DISTANCE * PROJECTILE_ANGER_DISTANCE
            {
                if self.brain.memories().lock().has_value(&RECENT_PROJECTILE) {
                    if self.can_target_entity(owner.as_ref()) {
                        suspicious_pos = owner.block_position();
                    }
                    self.increase_anger_at(Some(owner));
                } else {
                    self.increase_anger_at_by(Some(owner), PROJECTILE_ANGER, true);
                }
            }
            self.brain.memories().lock().set_with_expiry(
                &RECENT_PROJECTILE,
                (),
                RECENT_PROJECTILE_TICK_THRESHOLD,
            );
        } else {
            self.increase_anger_at(source_entity);
        }
        if !self.anger_level().is_angry() {
            let active = self
                .anger
                .lock()
                .active_entity(|entity| self.can_target_entity(entity.as_ref()));
            let investigate = projectile_owner.is_some()
                || active.is_none()
                || active
                    .zip(source_entity)
                    .is_some_and(|(active, source)| active.id() == source.id());
            if investigate {
                ai::set_disturbance_location(self, suspicious_pos);
            }
        }
    }

    fn listenable_events(&self) -> Identifier {
        GameEventTag::WARDEN_CAN_LISTEN
    }

    fn can_trigger_avoid_vibration(&self) -> bool {
        true
    }
}

impl VibrationSystem for WardenEntity {
    fn vibration_data(&self) -> &SyncMutex<VibrationData> {
        &self.vibration_data
    }
}
