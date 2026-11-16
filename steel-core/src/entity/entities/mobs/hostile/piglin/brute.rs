//! Vanilla `PiglinBrute` and `PiglinBruteAi`: the axe-wielding bastion guard
//! that never barters and attacks every player in sight.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::PiglinBruteEntityData;
use steel_registry::{sound_events, vanilla_entities, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, GlobalPos};

use super::anger::{anger_target, as_abstract_piglin, maybe_retaliate};
use super::piglin_mob::new_abstract_piglin_mob_base;
use super::sensor::PiglinBruteSpecificSensor;
use super::{AbstractPiglin, convert_to_zombified_piglin};
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::TimedBehavior;
use crate::entity::ai::brain::behaviors::{
    DoNothing, LookAtTargetSink, MoveToTargetSink, interact_with_door, interact_with_type,
    melee_attack, random_stroll, run_one, set_entity_look_target, set_look_and_interact,
    set_walk_target_from_attack_target_if_target_out_of_reach, start_attacking,
    stop_attacking_if_target_invalid_when, stop_being_angry_if_target_dead, stroll_around_poi,
    stroll_to_poi,
};
use crate::entity::ai::brain::memory_types::{
    ATTACK_TARGET, HOME, INTERACTION_TARGET, NEAREST_VISIBLE_ADULT_PIGLINS,
    NEAREST_VISIBLE_ATTACKABLE_PLAYER, NEAREST_VISIBLE_NEMESIS,
};
use crate::entity::ai::brain::nearest_visible::is_entity_attackable_ignoring_line_of_sight;
use crate::entity::ai::brain::sensors::{
    HurtBySensor, NearestItemSensor, NearestLivingEntitySensor, PlayerSensor,
};
use crate::entity::ai::brain::{
    ActivityData, AnyMemoryModuleType, BehaviorControl, Brain, Memories, Sensor,
};
use crate::entity::damage::DamageSource;
use crate::entity::spawn::SpawnGroupData;
use crate::entity::{
    DefaultEntityPush, Entity, EntityBase, EntityBaseLoad, EntityMovementEmission, EntityPose,
    EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, LivingEntitySyncedData,
    Mob, MobBase, Monster, PathfinderMob, SharedEntity, sync_dirty_mob_effects,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoveResult;
use crate::world::{LevelReader, World};

const XP_REWARD: i32 = 20;
/// Vanilla `PiglinBruteAi.MELEE_ATTACK_COOLDOWN`.
const MELEE_ATTACK_COOLDOWN: i32 = 20;
/// Vanilla `PiglinBruteAi.ACTIVITY_SOUND_LIKELIHOOD_PER_TICK`.
const ACTIVITY_SOUND_LIKELIHOOD_PER_TICK: f32 = 0.0125;
/// Vanilla `PiglinBruteAi.MAX_LOOK_DIST`.
const MAX_LOOK_DIST: f32 = 8.0;
/// Vanilla `PiglinBruteAi.INTERACTION_RANGE`.
const INTERACTION_RANGE: i32 = 8;
/// Vanilla `PiglinBruteAi.SPEED_MULTIPLIER_WHEN_IDLING`.
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 0.6;
/// Vanilla `PiglinBruteAi.HOME_CLOSE_ENOUGH_DISTANCE`.
const HOME_CLOSE_ENOUGH_DISTANCE: i32 = 2;
/// Vanilla `PiglinBruteAi.HOME_TOO_FAR_DISTANCE`.
const HOME_TOO_FAR_DISTANCE: i32 = 100;
/// Vanilla `PiglinBruteAi.HOME_STROLL_AROUND_DISTANCE`.
const HOME_STROLL_AROUND_DISTANCE: i32 = 5;
const STEP_VOLUME: f32 = 0.15;

/// Vanilla `PiglinBrute.BRAIN_PROVIDER` memories.
static BRUTE_MEMORIES: [&dyn AnyMemoryModuleType; 1] = [&NEAREST_VISIBLE_ADULT_PIGLINS];

fn is_type(entity: &dyn LivingEntity, entity_type: EntityTypeRef) -> bool {
    entity.entity_type() == entity_type
}

/// A piglin brute.
#[entity_behavior(class = "PiglinBrute")]
pub struct PiglinBruteEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<PiglinBruteEntityData>,
    brain: Brain<Self>,
    time_in_overworld: AtomicI32,
}

// SAFETY: The owner-scoped type key uniquely identifies PiglinBruteEntity.
unsafe impl DowncastType for PiglinBruteEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/piglin_brute");
}

impl PiglinBruteEntity {
    /// Creates a new piglin brute.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved piglin brute.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mob_base = new_abstract_piglin_mob_base();
        mob_base.set_xp_reward(XP_REWARD);
        let mut entity_data = PiglinBruteEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            brain: Brain::new(&BRUTE_MEMORIES, Self::sensors(), Self::activities(), None),
            time_in_overworld: AtomicI32::new(0),
        }
    }

    fn sensors() -> Vec<Box<dyn Sensor<Self>>> {
        vec![
            Box::new(NearestLivingEntitySensor),
            Box::new(PlayerSensor),
            Box::new(NearestItemSensor),
            Box::new(HurtBySensor),
            Box::new(PiglinBruteSpecificSensor),
        ]
    }

    /// The brute's brain memories.
    #[must_use]
    pub const fn memories(&self) -> &SyncMutex<Memories> {
        self.brain.memories()
    }

    /// Vanilla `setTimeInOverworld`.
    pub fn set_time_in_overworld(&self, ticks: i32) {
        self.time_in_overworld.store(ticks, Ordering::Relaxed);
    }

    /// Vanilla `PiglinBruteAi.getActivities`.
    fn activities() -> Vec<ActivityData<Self>> {
        vec![
            ActivityData::with_first_priority(
                Activity::CORE,
                0,
                vec![
                    Box::new(TimedBehavior::new(LookAtTargetSink::new(45, 90))),
                    Box::new(TimedBehavior::new(MoveToTargetSink::new())),
                    Box::new(interact_with_door()),
                    Box::new(stop_being_angry_if_target_dead()),
                ],
            ),
            ActivityData::with_first_priority(
                Activity::IDLE,
                10,
                vec![
                    Box::new(start_attacking(|world: &Arc<World>, brute: &Self| {
                        brute.find_nearest_valid_attack_target(world)
                    })),
                    Box::new(Self::idle_look_behaviors()),
                    Box::new(Self::idle_movement_behaviors()),
                    Box::new(set_look_and_interact(
                        |entity: &SharedEntity| entity.entity_type() == &vanilla_entities::PLAYER,
                        4,
                    )),
                ],
            ),
            ActivityData::with_first_priority(
                Activity::FIGHT,
                10,
                vec![
                    Box::new(stop_attacking_if_target_invalid_when(
                        |world: &Arc<World>, brute: &Self, target: &dyn LivingEntity| {
                            !brute.is_nearest_valid_attack_target(world, target)
                        },
                        |_, _, _| {},
                    )),
                    Box::new(set_walk_target_from_attack_target_if_target_out_of_reach(
                        |_| 1.0,
                    )),
                    Box::new(melee_attack(MELEE_ATTACK_COOLDOWN)),
                ],
            )
            .while_memory_present(&ATTACK_TARGET),
        ]
    }

    /// Vanilla `PiglinBruteAi.createIdleLookBehaviors`.
    fn idle_look_behaviors() -> impl BehaviorControl<Self> {
        let behaviors: Vec<(Box<dyn BehaviorControl<Self>>, i32)> = vec![
            (
                Box::new(set_entity_look_target(MAX_LOOK_DIST, |entity| {
                    is_type(entity, &vanilla_entities::PLAYER)
                })),
                1,
            ),
            (
                Box::new(set_entity_look_target(MAX_LOOK_DIST, |entity| {
                    is_type(entity, &vanilla_entities::PIGLIN)
                })),
                1,
            ),
            (
                Box::new(set_entity_look_target(MAX_LOOK_DIST, |entity| {
                    is_type(entity, &vanilla_entities::PIGLIN_BRUTE)
                })),
                1,
            ),
            (Box::new(set_entity_look_target(MAX_LOOK_DIST, |_| true)), 1),
            (Box::new(DoNothing::new(30, 60)), 1),
        ];
        run_one(Vec::new(), behaviors)
    }

    /// Vanilla `PiglinBruteAi.createIdleMovementBehaviors`.
    fn idle_movement_behaviors() -> impl BehaviorControl<Self> {
        let behaviors: Vec<(Box<dyn BehaviorControl<Self>>, i32)> = vec![
            (
                Box::new(random_stroll(SPEED_MULTIPLIER_WHEN_IDLING, true)),
                2,
            ),
            (
                Box::new(interact_with_type(
                    |entity: &SharedEntity| entity.entity_type() == &vanilla_entities::PIGLIN,
                    INTERACTION_RANGE,
                    &INTERACTION_TARGET,
                    SPEED_MULTIPLIER_WHEN_IDLING,
                    2,
                )),
                2,
            ),
            (
                Box::new(interact_with_type(
                    |entity: &SharedEntity| entity.entity_type() == &vanilla_entities::PIGLIN_BRUTE,
                    INTERACTION_RANGE,
                    &INTERACTION_TARGET,
                    SPEED_MULTIPLIER_WHEN_IDLING,
                    2,
                )),
                2,
            ),
            (
                Box::new(stroll_to_poi(
                    &HOME,
                    SPEED_MULTIPLIER_WHEN_IDLING,
                    HOME_CLOSE_ENOUGH_DISTANCE,
                    HOME_TOO_FAR_DISTANCE,
                )),
                2,
            ),
            (
                Box::new(stroll_around_poi(
                    &HOME,
                    SPEED_MULTIPLIER_WHEN_IDLING,
                    HOME_STROLL_AROUND_DISTANCE,
                )),
                2,
            ),
            (Box::new(DoNothing::new(30, 60)), 1),
        ];
        run_one(Vec::new(), behaviors)
    }

    /// Vanilla `PiglinBruteAi.updateActivity`.
    fn update_activity(&self) {
        let old_activity = self.brain.active_non_core_activity();
        self.brain
            .set_active_activity_to_first_valid(&[Activity::FIGHT, Activity::IDLE]);
        if old_activity != self.brain.active_non_core_activity() {
            self.play_activity_sound();
        }
        let has_target = self.brain.memories().lock().has_value(&ATTACK_TARGET);
        self.set_aggressive(has_target);
    }

    /// Vanilla `PiglinBruteAi.playActivitySound`: an angry grunt while
    /// fighting.
    fn play_activity_sound(&self) {
        if self.brain.active_non_core_activity() == Some(Activity::FIGHT) {
            self.make_sound(Some(&sound_events::ENTITY_PIGLIN_BRUTE_ANGRY));
        }
    }

    /// Vanilla `PiglinBruteAi.findNearestValidAttackTarget`.
    fn find_nearest_valid_attack_target(&self, world: &World) -> Option<SharedEntity> {
        if let Some(angry_at) = anger_target(world, self)
            && angry_at.as_living_entity().is_some_and(|living| {
                is_entity_attackable_ignoring_line_of_sight(world, self, living)
            })
        {
            return Some(angry_at);
        }
        let memories = self.brain.memories().lock();
        memories
            .get(&NEAREST_VISIBLE_ATTACKABLE_PLAYER)
            .or_else(|| memories.get(&NEAREST_VISIBLE_NEMESIS))
    }

    /// Vanilla `PiglinBruteAi.isNearestValidAttackTarget`.
    fn is_nearest_valid_attack_target(&self, world: &World, target: &dyn LivingEntity) -> bool {
        self.find_nearest_valid_attack_target(world)
            .is_some_and(|nearest| nearest.id() == target.id())
    }
}

impl DefaultEntityPush for PiglinBruteEntity {}

impl Entity for PiglinBruteEntity {
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
        let scale = LivingEntity::get_scale(self);
        if self.entity_type.fixed {
            self.entity_type.dimensions
        } else {
            self.entity_type.dimensions.scale(scale)
        }
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

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(&sound_events::ENTITY_PIGLIN_BRUTE_STEP, STEP_VOLUME, 1.0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("Brain", self.brain.save());
        nbt.insert(
            "IsImmuneToZombification",
            i8::from(self.is_immune_to_zombification()),
        );
        nbt.insert(
            "TimeInOverworld",
            self.time_in_overworld.load(Ordering::Relaxed),
        );
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        if let Some(brain) = nbt.compound("Brain") {
            self.brain.memories().lock().load(brain);
        }
        self.set_can_pick_up_loot(nbt.byte("CanPickUpLoot").is_none_or(|value| value != 0));
        self.set_immune_to_zombification(
            nbt.byte("IsImmuneToZombification")
                .is_some_and(|value| value != 0),
        );
        self.time_in_overworld
            .store(nbt.int("TimeInOverworld").unwrap_or(0), Ordering::Relaxed);
    }
}

impl LivingEntity for PiglinBruteEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn living_synced_data(&self) -> Option<&dyn LivingEntitySyncedData> {
        Some(&self.entity_data)
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

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_PIGLIN_BRUTE_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_PIGLIN_BRUTE_DEATH)
    }

    /// Vanilla `PiglinBrute.hurtServer` -> `PiglinBruteAi.wasHurtBy`: fights
    /// back unless a fellow piglin hit it.
    fn hurt_server(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        let was_hurt = self.default_hurt_server(world, source, amount);
        if was_hurt
            && let Some(attacker) = source.causing_entity()
            && attacker.is_living_entity()
            && as_abstract_piglin(attacker).is_none()
        {
            maybe_retaliate(world, self, attacker);
        }
        was_hurt
    }

    fn should_drop_experience(&self) -> bool {
        Monster::should_drop_experience_monster(self)
    }

    fn should_drop_loot(&self, world: &World) -> bool {
        Monster::should_drop_loot_monster(self, world)
    }

    fn base_experience_reward(&self) -> i32 {
        self.xp_reward()
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        Monster::monster_ai_step(self, entity)
    }
}

impl Mob for PiglinBruteEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `AbstractPiglin.getTarget`: the brain's attack target.
    fn target(&self) -> Option<SharedEntity> {
        self.brain
            .memories()
            .lock()
            .get(&ATTACK_TARGET)
            .filter(|target| {
                target
                    .as_living_entity()
                    .is_some_and(|living| self.is_valid_target(living))
            })
    }

    fn custom_server_ai_step(&self) {
        if let Some(world) = self.level() {
            self.brain.tick(&world, self);
        }
        self.update_activity();
        // Vanilla `PiglinBruteAi.maybePlayActivitySound`.
        if rand::random::<f32>() < ACTIVITY_SOUND_LIKELIHOOD_PER_TICK {
            self.play_activity_sound();
        }
        self.tick_zombification();
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_PIGLIN_BRUTE_AMBIENT)
    }

    /// Vanilla `AbstractPiglin.playAmbientSound`: only while idle.
    fn play_ambient_sound(&self) {
        if self.is_activity_active(Activity::IDLE) {
            self.make_sound(self.ambient_sound());
        }
    }

    /// Vanilla `PiglinBrute.wantsToPickUp`: golden axes only.
    fn wants_to_pick_up(&self, item_stack: &ItemStack) -> bool {
        item_stack.is(&vanilla_items::GOLDEN_AXE) && self.can_hold_item(item_stack)
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        // Vanilla `PiglinBruteAi.initMemories`.
        self.brain.memories().lock().set(
            &HOME,
            Some(GlobalPos::new(world.key.clone(), self.block_position())),
        );
        self.set_item_slot(
            EquipmentSlot::MainHand,
            ItemStack::new(&vanilla_items::GOLDEN_AXE),
        );
        self.finalize_spawn_mob_base(world, spawn_reason, group_data)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().abstract_piglin.mob.mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data
            .lock()
            .abstract_piglin
            .mob
            .mob_flags
            .set(flags);
    }
}

impl PathfinderMob for PiglinBruteEntity {
    fn get_walk_target_value_in(&self, pos: BlockPos, level: &dyn LevelReader) -> f32 {
        Monster::get_walk_target_value_monster(self, pos, level)
    }
}

impl Monster for PiglinBruteEntity {}

impl AbstractPiglin for PiglinBruteEntity {
    fn can_hunt(&self) -> bool {
        false
    }

    fn is_activity_active(&self, activity: Activity) -> bool {
        self.brain.is_active(activity)
    }

    fn is_immune_to_zombification(&self) -> bool {
        *self
            .entity_data
            .lock()
            .abstract_piglin
            .immune_to_zombification
            .get()
    }

    fn set_immune_to_zombification(&self, immune: bool) {
        self.entity_data
            .lock()
            .abstract_piglin
            .immune_to_zombification
            .set(immune);
    }

    fn time_in_overworld(&self) -> &AtomicI32 {
        &self.time_in_overworld
    }

    fn converted_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_PIGLIN_BRUTE_CONVERTED_TO_ZOMBIFIED
    }

    fn finish_conversion(&self) {
        convert_to_zombified_piglin(self);
    }
}
