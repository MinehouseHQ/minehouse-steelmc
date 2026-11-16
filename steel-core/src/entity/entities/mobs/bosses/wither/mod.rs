//! Vanilla `WitherBoss`: the three-headed boss built from soul sand and
//! wither skeleton skulls. It charges up invulnerable for 11 seconds, blows
//! up, then flies after its targets firing wither skulls from all three
//! heads, breaking the blocks around it whenever it is hurt.

#[cfg(test)]
mod tests;

use std::f32::consts::PI;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::{BossBarColor, BossBarFlags, BossBarOverlay, SoundSource};
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_damage_type_tags::DamageTypeTag;
use steel_registry::vanilla_entity_data::WitherEntityData;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::vanilla_game_rules::MOB_GRIEFING;
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _, level_events, sound_events, vanilla_entities, vanilla_items,
    vanilla_mob_effects,
};
use steel_utils::locks::SyncMutex;
use steel_utils::translations::ENTITY_MINECRAFT_WITHER;
use steel_utils::types::Difficulty;
use steel_utils::{BlockPos, Downcast as _, DowncastType, DowncastTypeKey};
use text_components::TextComponent;
use uuid::Uuid;

use crate::boss_event::ServerBossEvent;
use crate::entity::ai::flying_move::FlyingMoveControl;
use crate::entity::ai::goal::{
    Goal, GoalControls, HurtByTargetGoal, LookAtPlayerGoal, NearestAttackableTargetGoal,
    RandomLookAroundGoal, RangedAttackGoal, WaterAvoidingRandomFlyingGoal,
};
use crate::entity::ai::navigation::PathNavigation;
use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::damage::DamageSource;
use crate::entity::entities::{WitherSkullEntity, wither_can_destroy};
use crate::entity::living_base::MobEffectInstance;
use crate::entity::{
    AbstractHurtingProjectile, DefaultEntityPush, Entity, EntityBase, EntityBaseLoad,
    EntityMovementEmission, EntityPose, EntitySyncedData, LivingEntity, LivingEntityBase, Mob,
    MobBase, Monster, PathfinderMob, RangedAttackMob, RemovalReason, SharedEntity, next_entity_id,
    sync_dirty_mob_effects,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;
use crate::world::explosion::{ExplosionInteraction, ExplosionOptions};

/// Vanilla `INVULNERABLE_TICKS`: the charge-up after the wither is built.
pub const INVULNERABLE_TICKS: i32 = 220;
const XP_REWARD: i32 = 50;
const TAG_INVUL: &str = "Invul";
/// Vanilla `FlyingMoveControl(this, 10, false)`.
const MOVE_CONTROL: FlyingMoveControl = FlyingMoveControl {
    max_turn: 10,
    hovers_in_place: false,
};
/// Range of the side heads' own target search.
const HEAD_TARGET_RANGE: f64 = 20.0;
const HEAD_SEARCH_VERTICAL: f64 = 8.0;
/// Side heads drop a target farther than 30 blocks.
const HEAD_MAX_DISTANCE_SQR: f64 = 900.0;
const SPAWN_EXPLOSION_POWER: f32 = 7.0;
const HEAD_COUNT: usize = 3;

/// The wither's server-side counters.
#[derive(Debug, Default)]
struct WitherState {
    /// Vanilla `nextHeadUpdate` of the two side heads.
    next_head_update: [i32; 2],
    /// Vanilla `idleHeadUpdates` of the two side heads.
    idle_head_updates: [i32; 2],
    /// Vanilla `destroyBlocksTick`.
    destroy_blocks_tick: i32,
}

/// Vanilla `WitherBoss`.
#[entity_behavior(class = "WitherBoss")]
pub struct WitherBossEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<WitherEntityData>,
    state: SyncMutex<WitherState>,
    boss_event: ServerBossEvent,
}

// SAFETY: The owner-scoped type key uniquely identifies WitherBossEntity.
unsafe impl DowncastType for WitherBossEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/wither");
}

impl WitherBossEntity {
    /// Creates a new wither.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved wither.
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
        let mut entity_data = WitherEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        {
            let mut navigation = PathNavigation::flying();
            navigation.set_can_open_doors(false);
            navigation.set_can_float(true);
            *mob_base.navigation().lock() = navigation;
        }
        {
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, WitherDoNothingGoal);
            goal_selector.add_goal(2, RangedAttackGoal::new(1.0, 40, 20.0));
            goal_selector.add_goal(5, WaterAvoidingRandomFlyingGoal::new(1.0));
            goal_selector.add_goal(6, LookAtPlayerGoal::new(8.0));
            goal_selector.add_goal(7, RandomLookAroundGoal::new());

            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(1, HurtByTargetGoal::new());
            target_selector.add_goal(
                2,
                NearestAttackableTargetGoal::new_with_interval(0, false, false, |target, _| {
                    is_wither_target(target)
                }),
            );
        }
        let boss_event = ServerBossEvent::new(
            Uuid::new_v4(),
            TextComponent::translated(ENTITY_MINECRAFT_WITHER.msg()),
            BossBarColor::Purple,
            BossBarOverlay::Progress,
        );
        boss_event.set_flags(BossBarFlags {
            darken_screen: true,
            play_boss_music: false,
            create_world_fog: false,
        });

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            state: SyncMutex::new(WitherState::default()),
            boss_event,
        }
    }

    /// The boss bar shown to players that see the wither.
    #[must_use]
    pub const fn boss_event(&self) -> &ServerBossEvent {
        &self.boss_event
    }

    /// Vanilla `getInvulnerableTicks`: the charge-up left.
    #[must_use]
    pub fn invulnerable_ticks(&self) -> i32 {
        *self.entity_data.lock().wither_boss.id_inv.get()
    }

    /// Vanilla `setInvulnerableTicks`.
    pub fn set_invulnerable_ticks(&self, ticks: i32) {
        self.entity_data.lock().wither_boss.id_inv.set(ticks);
    }

    /// Vanilla `getAlternativeTarget`: the entity id head `head` aims at,
    /// or 0.
    #[must_use]
    pub fn alternative_target(&self, head: usize) -> i32 {
        let data = self.entity_data.lock();
        let targets = &data.wither_boss;
        *match head {
            0 => &targets.target_a,
            1 => &targets.target_b,
            _ => &targets.target_c,
        }
        .get()
    }

    /// Vanilla `setAlternativeTarget`.
    pub fn set_alternative_target(&self, head: usize, entity_id: i32) {
        let mut data = self.entity_data.lock();
        let targets = &mut data.wither_boss;
        match head {
            0 => &mut targets.target_a,
            1 => &mut targets.target_b,
            _ => &mut targets.target_c,
        }
        .set(entity_id);
    }

    /// Vanilla `isPowered`: below half health the wither gets its armor and
    /// shrugs off arrows.
    #[must_use]
    pub fn is_powered(&self) -> bool {
        self.get_health() <= self.get_max_health() / 2.0
    }

    /// Vanilla `makeInvulnerable`: the charge-up of a freshly built wither.
    pub fn make_invulnerable(&self) {
        self.set_invulnerable_ticks(INVULNERABLE_TICKS);
        self.boss_event.set_progress(0.0);
        self.set_health(self.get_max_health() / 3.0);
    }

    /// Vanilla `getHeadX`.
    fn head_x(&self, head: usize) -> f64 {
        let x = self.position().x;
        if head == 0 {
            return x;
        }
        x + f64::from(self.head_angle(head).cos()) * 1.3 * f64::from(self.get_scale())
    }

    /// Vanilla `getHeadY`.
    fn head_y(&self, head: usize) -> f64 {
        let height = if head == 0 { 3.0 } else { 2.2 };
        self.position().y + f64::from(height * self.get_scale())
    }

    /// Vanilla `getHeadZ`.
    fn head_z(&self, head: usize) -> f64 {
        let z = self.position().z;
        if head == 0 {
            return z;
        }
        z + f64::from(self.head_angle(head).sin()) * 1.3 * f64::from(self.get_scale())
    }

    /// Vanilla `headAngle`: `yBodyRot + 180 * (index - 1)` in radians. The
    /// side heads fire as indices 2 and 3, so vanilla aims them from the
    /// left side and the front (one full turn) rather than both sides.
    fn head_angle(&self, head: usize) -> f32 {
        #[expect(clippy::cast_precision_loss, reason = "head index is at most 3")]
        let turn = 180.0 * (head as f32 - 1.0);
        (self.y_body_rot() + turn) * (PI / 180.0)
    }

    /// Vanilla `performRangedAttack(head, target)`: the middle head's skull
    /// is blue one time in a thousand.
    fn shoot_at(&self, head: usize, target: &dyn Entity) {
        let dangerous = head == 0 && rand::random::<f32>() < 0.001;
        let position = target.position();
        let eye_height = target.get_eye_height();
        self.shoot_skull(
            head,
            DVec3::new(position.x, position.y + eye_height * 0.5, position.z),
            dangerous,
        );
    }

    /// Vanilla `performRangedAttack(head, x, y, z, dangerous)`.
    fn shoot_skull(&self, head: usize, at: DVec3, dangerous: bool) {
        let Some(world) = self.level() else {
            return;
        };
        if !self.is_silent() {
            world.level_event(
                level_events::SOUND_WITHER_BOSS_SHOOT,
                self.block_position(),
                0,
                None,
            );
        }
        let Some(this) = world.get_entity_by_id(self.id()) else {
            return;
        };
        let head_pos = DVec3::new(self.head_x(head), self.head_y(head), self.head_z(head));
        let skull = WitherSkullEntity::new(
            &vanilla_entities::WITHER_SKULL,
            next_entity_id(),
            head_pos,
            Arc::downgrade(&world),
        );
        skull.shoot_from_mob(&this, (at - head_pos).normalize_or_zero());
        if dangerous {
            skull.set_dangerous(true);
        }
        if let Err(error) = skull.try_set_position(head_pos) {
            log::debug!("wither skull could not move to its head: {error}");
        }
        if let Err(error) = world.try_add_entity(Arc::new(skull)) {
            log::debug!("wither skull was not added to the world: {error}");
        }
    }

    /// The flying half of vanilla `aiStep`: rise above and close in on the
    /// middle head's target.
    fn steer_towards_target(&self, world: &World) {
        let mut movement = self.velocity() * DVec3::new(1.0, 0.6, 1.0);
        let target_id = self.alternative_target(0);
        if target_id > 0
            && let Some(target) = world.get_entity_by_id(target_id)
        {
            let position = self.position();
            let target_position = target.position();
            let mut yd = movement.y;
            if position.y < target_position.y
                || !self.is_powered() && position.y < target_position.y + 5.0
            {
                yd = yd.max(0.0);
                yd += 0.3 - yd * f64::from(0.6_f32);
            }
            movement.y = yd;
            let delta = DVec3::new(
                target_position.x - position.x,
                0.0,
                target_position.z - position.z,
            );
            if delta.x * delta.x + delta.z * delta.z > 9.0 {
                let scale = delta.normalize();
                movement += DVec3::new(
                    scale.x * 0.3 - movement.x * 0.6,
                    0.0,
                    scale.z * 0.3 - movement.z * 0.6,
                );
            }
        }
        self.set_velocity(movement);
        if movement.x * movement.x + movement.z * movement.z > 0.05 {
            #[expect(clippy::cast_possible_truncation, reason = "vanilla (float) Mth.atan2")]
            let y_rot = (movement.z.atan2(movement.x) as f32) * (180.0 / PI) - 90.0;
            let (_, x_rot) = self.rotation();
            self.set_rotation((y_rot, x_rot));
        }
    }

    /// The charge-up half of vanilla `customServerAiStep`.
    fn tick_charge_up(&self, world: &Arc<World>) {
        let ticks = self.invulnerable_ticks() - 1;
        #[expect(clippy::cast_precision_loss, reason = "vanilla int / float")]
        self.boss_event
            .set_progress(1.0 - ticks as f32 / INVULNERABLE_TICKS as f32);
        if ticks <= 0 {
            let mut options = ExplosionOptions::new(
                DVec3::new(self.position().x, self.get_eye_y(), self.position().z),
                SPAWN_EXPLOSION_POWER,
                ExplosionInteraction::Mob,
            );
            options.source = world.get_entity_by_id(self.id());
            world.explode(options);
            if !self.is_silent() {
                world.global_level_event(
                    level_events::SOUND_WITHER_BOSS_SPAWN,
                    self.block_position(),
                    0,
                );
            }
        }
        self.set_invulnerable_ticks(ticks);
        if self.tick_count() % 10 == 0 {
            self.heal(10.0);
        }
    }

    /// The side heads of vanilla `customServerAiStep`: each picks its own
    /// target and fires at it, or now and then at a random spot.
    fn tick_side_heads(&self, world: &World) {
        let tick_count = self.tick_count();
        for head in 1..HEAD_COUNT {
            let side = head - 1;
            {
                let mut state = self.state.lock();
                if tick_count < state.next_head_update[side] {
                    continue;
                }
                state.next_head_update[side] = tick_count + 10 + rand::random_range(0..10);
            }
            let difficulty = world.difficulty();
            let idle = {
                let mut state = self.state.lock();
                let idle = state.idle_head_updates[side];
                if matches!(difficulty, Difficulty::Normal | Difficulty::Hard) {
                    state.idle_head_updates[side] += 1;
                }
                idle
            };
            if matches!(difficulty, Difficulty::Normal | Difficulty::Hard) && idle > 15 {
                let position = self.position();
                let at = DVec3::new(
                    position.x + rand::random_range(-10.0..10.0),
                    position.y + rand::random_range(-5.0..5.0),
                    position.z + rand::random_range(-10.0..10.0),
                );
                self.shoot_skull(head + 1, at, true);
                self.state.lock().idle_head_updates[side] = 0;
            }
            self.tick_head_target(world, head);
        }
    }

    /// One side head keeps, shoots at or looks for its own target.
    fn tick_head_target(&self, world: &World, head: usize) {
        let side = head - 1;
        let target_id = self.alternative_target(head);
        if target_id > 0 {
            let current = world.get_entity_by_id(target_id);
            let usable = current.as_ref().is_some_and(|current| {
                current.as_living_entity().is_some_and(|living| {
                    Mob::can_attack(self, living)
                        && self.distance_to_sqr(current.position()) <= HEAD_MAX_DISTANCE_SQR
                        && self.has_line_of_sight(current.as_ref())
                })
            });
            if let (true, Some(current)) = (usable, current) {
                self.shoot_at(head + 1, current.as_ref());
                let mut state = self.state.lock();
                state.next_head_update[side] = self.tick_count() + 40 + rand::random_range(0..20);
                state.idle_head_updates[side] = 0;
            } else {
                self.set_alternative_target(head, 0);
            }
            return;
        }
        let conditions = TargetingConditions::for_combat()
            .range(HEAD_TARGET_RANGE)
            .selector(|target, _| is_wither_target(target));
        let search = self.bounding_box().inflate_xyz(
            HEAD_TARGET_RANGE,
            HEAD_SEARCH_VERTICAL,
            HEAD_TARGET_RANGE,
        );
        let candidates = world.get_entities_in_aabb_matching(&search, |entity| {
            entity
                .as_living_entity()
                .is_some_and(|living| conditions.test(world, Some(self), living))
        });
        if !candidates.is_empty() {
            let selected = &candidates[rand::random_range(0..candidates.len())];
            self.set_alternative_target(head, selected.id());
        }
    }

    /// Vanilla `destroyBlocksTick`: a second after being hurt the wither
    /// breaks every block it overlaps, as mob griefing allows.
    fn tick_destroy_blocks(&self, world: &Arc<World>) {
        let destroy = {
            let mut state = self.state.lock();
            if state.destroy_blocks_tick <= 0 {
                return;
            }
            state.destroy_blocks_tick -= 1;
            state.destroy_blocks_tick == 0
        };
        if !destroy || !world.get_game_rule(&MOB_GRIEFING) {
            return;
        }
        let dimensions = self.dimensions_for_pose(self.pose());
        #[expect(clippy::cast_possible_truncation, reason = "vanilla Mth.floor")]
        let (width, height) = (
            (dimensions.width / 2.0 + 1.0).floor() as i32,
            dimensions.height.floor() as i32,
        );
        let pos = self.block_position();
        let mut destroyed = false;
        for block in BlockPos::between_closed_coords(
            pos.x() - width,
            pos.y(),
            pos.z() - width,
            pos.x() + width,
            pos.y() + height,
            pos.z() + width,
        ) {
            if wither_can_destroy(world.get_block_state(block)) {
                destroyed = world.destroy_block_by_entity(block, true, self) || destroyed;
            }
        }
        if destroyed {
            world.level_event(level_events::SOUND_WITHER_BLOCK_BREAK, pos, 0, None);
        }
    }

    /// Vanilla `customServerAiStep`.
    fn wither_server_ai_step(&self) {
        let Some(world) = self.level() else {
            return;
        };
        if self.invulnerable_ticks() > 0 {
            self.tick_charge_up(&world);
            return;
        }
        self.tick_side_heads(&world);
        let target = self.target().map_or(0, |target| target.id());
        self.set_alternative_target(0, target);
        self.tick_destroy_blocks(&world);
        if self.tick_count() % 20 == 0 {
            self.heal(1.0);
        }
        self.boss_event
            .set_progress(self.get_health() / self.get_max_health());
    }
}

/// Vanilla `LIVING_ENTITY_SELECTOR`: anything attackable outside the
/// wither's friends.
fn is_wither_target(target: &dyn LivingEntity) -> bool {
    !REGISTRY
        .entity_types
        .is_in_tag(target.entity_type(), &EntityTypeTag::WITHER_FRIENDS)
        && target.attackable()
}

impl DefaultEntityPush for WitherBossEntity {}

impl Entity for WitherBossEntity {
    fn as_default_entity_push(&self) -> Option<&dyn DefaultEntityPush> {
        Some(self)
    }

    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn movement_emission(&self) -> EntityMovementEmission {
        EntityMovementEmission::All
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        self.entity_type
            .dimensions
            .scale(LivingEntity::get_scale(self))
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn update_data_before_sync(&self) {
        sync_dirty_mob_effects(&self.living_base, &self.entity_data);
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

    /// Vanilla `setCustomName`: the boss bar shows the wither's name.
    fn set_custom_name(&self, custom_name: Option<TextComponent>) {
        self.default_set_custom_name(custom_name);
        self.boss_event.set_name(self.display_name());
    }

    /// Vanilla `startSeenByPlayer`: the boss bar follows the wither's viewers.
    fn start_seen_by_player(&self, player: &Player) {
        if let Some(world) = self.level()
            && let Some(player) = world.players.get_by_entity_id(player.id())
        {
            self.boss_event.add_player(&player);
        }
    }

    /// Vanilla `stopSeenByPlayer`.
    fn stop_seen_by_player(&self, player: &Player) {
        self.boss_event.remove_player(player);
    }

    /// Vanilla `makeStuckInBlock`: cobwebs and berry bushes do not slow it.
    fn make_stuck_in_block(&self, _state: steel_utils::BlockStateId, _speed_multiplier: DVec3) {}

    fn can_ride(&self, _vehicle: &dyn Entity) -> bool {
        false
    }

    fn can_use_portal(&self, _ignore_passenger: bool) -> bool {
        false
    }

    /// Vanilla `checkDespawn`: only peaceful removes the wither.
    fn check_despawn(&self) {
        let Some(world) = self.level() else {
            return;
        };
        if world.difficulty() == Difficulty::Peaceful && !self.entity_type.allowed_in_peaceful {
            self.set_removed(RemovalReason::Discarded);
        } else {
            self.set_no_action_time(0);
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert(TAG_INVUL, self.invulnerable_ticks());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.set_invulnerable_ticks(nbt.int(TAG_INVUL).unwrap_or(0));
        if self.custom_name().is_some() {
            self.boss_event.set_name(self.display_name());
        }
    }
}

impl LivingEntity for WitherBossEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
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

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_WITHER_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_WITHER_DEATH)
    }

    fn should_drop_experience(&self) -> bool {
        Monster::should_drop_experience_monster(self)
    }

    fn should_drop_loot(&self, world: &World) -> bool {
        Monster::should_drop_loot_monster(self, world)
    }

    /// Vanilla `hurtServer`.
    fn hurt_server(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        if self.is_invulnerable_to(world, source)
            || source.is(&DamageTypeTag::WITHER_IMMUNE_TO)
            || source
                .causing_entity()
                .is_some_and(|entity| entity.entity_type() == &vanilla_entities::WITHER)
        {
            return false;
        }
        if self.invulnerable_ticks() > 0 && !source.is(&DamageTypeTag::BYPASSES_INVULNERABILITY) {
            return false;
        }
        if self.is_powered()
            && source.direct_entity().is_some_and(|direct| {
                direct.is_abstract_arrow() || direct.entity_type() == &vanilla_entities::WIND_CHARGE
            })
        {
            return false;
        }
        if source.causing_entity().is_some_and(|entity| {
            REGISTRY
                .entity_types
                .is_in_tag(entity.entity_type(), &EntityTypeTag::WITHER_FRIENDS)
        }) {
            return false;
        }
        {
            let mut state = self.state.lock();
            if state.destroy_blocks_tick <= 0 {
                state.destroy_blocks_tick = 20;
            }
            for idle in &mut state.idle_head_updates {
                *idle += 3;
            }
        }
        self.default_hurt_server(world, source, amount)
    }

    /// Vanilla `dropCustomDeathLoot`: a Nether Star that lasts ten minutes.
    fn drop_custom_death_loot(&self, source: &DamageSource, killed_by_player: bool) {
        self.drop_custom_death_loot_mob(source, killed_by_player);
        if let Some(star) = self.spawn_at_location(ItemStack::new(&vanilla_items::NETHER_STAR), 0.0)
        {
            star.set_extended_lifetime();
        }
    }

    /// Vanilla `addEffect`: no effect sticks to the wither.
    fn add_mob_effect_with_source(
        &self,
        _effect: MobEffectInstance,
        _source: Option<&dyn Entity>,
    ) -> bool {
        false
    }

    /// Vanilla `canBeAffected`: immune to Wither.
    fn can_be_affected(&self, effect: &MobEffectInstance) -> bool {
        effect.effect() != vanilla_mob_effects::WITHER && self.default_can_be_affected(effect)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    /// Vanilla `aiStep`. The head rotations and particles it also updates
    /// are client-side.
    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        if let Some(world) = self.level() {
            self.steer_towards_target(&world);
        }
        Monster::monster_ai_step(self, entity)
    }
}

impl Mob for WitherBossEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn tick_move_control(&self) {
        MOVE_CONTROL.tick(self);
    }

    fn custom_server_ai_step(&self) {
        self.wither_server_ai_step();
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_WITHER_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl Monster for WitherBossEntity {}

impl PathfinderMob for WitherBossEntity {}

impl RangedAttackMob for WitherBossEntity {
    /// Vanilla `performRangedAttack(target, power)`: the middle head fires.
    fn perform_ranged_attack(&self, _entity: &SharedEntity, target: &SharedEntity, _power: f32) {
        self.shoot_at(0, target.as_ref());
    }
}

/// Vanilla `WitherBoss.WitherDoNothingGoal`: the wither holds still while it
/// charges up.
struct WitherDoNothingGoal;

impl Goal for WitherDoNothingGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::JUMP | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        mob.downcast_ref::<WitherBossEntity>()
            .is_some_and(|wither| wither.invulnerable_ticks() > 0)
    }
}
