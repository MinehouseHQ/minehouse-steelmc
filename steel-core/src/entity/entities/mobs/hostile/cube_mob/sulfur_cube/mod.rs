//! Vanilla `SulfurCube`: a cube mob that swallows a block and takes on that
//! block's archetype (bouncy, explosive, hot, ...), which players kick around.

mod archetype;
mod fuse;
mod goals;
mod interaction;
mod knockback;
#[cfg(test)]
mod tests;

use std::ops::Not as _;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::vanilla_components::SULFUR_CUBE_CONTENT;
use steel_registry::data_components::vanilla_components::SulfurCubeContent;
use steel_registry::entity_type::EntityAttachment;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::item_stack_template::ItemStackTemplate;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::sulfur_cube_archetype::{
    ContactDamage, DEFAULT_KNOCKBACK_MODIFIERS, ExplosionData, KnockbackModifiers, SoundSettings,
};
use steel_registry::vanilla_entity_data::SulfurCubeEntityData;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _, sound_events, vanilla_attributes, vanilla_items,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use super::CubeMobRandomDirectionGoal;
use super::{AbstractCubeMob, CubeMobBase, CubeMobFloatGoal, CubeMobKeepOnJumpingGoal};
use crate::behavior::InteractionResult;
use crate::entity::damage::DamageSource;
use crate::entity::entities::ItemEntity;
use crate::entity::entity::EntityEventSource as _;
use crate::entity::leash::Leashable as _;
use crate::entity::pushing::EntityPush;
use crate::entity::{
    AgeableMob, AgeableMobBase, Bucketable, Entity, EntityBase, EntityBaseLoad,
    EntityMovementEmission, EntityPose, EntitySpawnReason, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, PathfinderMob, RemovalReason, SharedEntity, SpawnGroupData,
    SpawnRule, sync_dirty_mob_effects,
};
use crate::entity::{
    KnockbackCause, Shearable, load_default_data_from_bucket_tag, save_default_data_to_bucket_tag,
    update_bucket_entity_data,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;

/// Vanilla `SulfurCube.MAX_SIZE`: an adult cube.
const ADULT_SIZE: i32 = 2;
/// Vanilla `SulfurCube.MIN_SIZE`: a baby cube.
const BABY_SIZE: i32 = 1;
/// Vanilla `SPLIT_COUNT`.
const SPLIT_COUNT: i32 = 2;
/// Vanilla `PICKUP_TIMER_DURATION`: ticks after shearing before it swallows again.
const PICKUP_TIMER_DURATION: i32 = 100;
/// Vanilla `getFluidJumpThreshold`: a fifth of the cube's height.
const FLUID_JUMP_THRESHOLD_SCALE: f64 = 0.2;

/// A sulfur cube.
#[entity_behavior(class = "SulfurCube", spawn_rule)]
pub struct SulfurCubeEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    cube_mob_base: CubeMobBase,
    entity_data: SyncMutex<SulfurCubeEntityData>,
    state: SyncMutex<SulfurCubeState>,
}

// SAFETY: The owner-scoped type key uniquely identifies SulfurCubeEntity.
unsafe impl DowncastType for SulfurCubeEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/sulfur_cube");
}

/// What the swallowed block's archetypes give the cube, plus its timers.
#[derive(Debug)]
struct SulfurCubeState {
    pickup_timer: i32,
    push_sound_cooldown: i32,
    floats_in_liquids: bool,
    explosion: Option<ExplosionData>,
    knockback_modifiers: KnockbackModifiers,
    sound_settings: SoundSettings,
    fuse: i32,
    contact_damages: Vec<ContactDamage>,
}

impl Default for SulfurCubeState {
    fn default() -> Self {
        Self {
            pickup_timer: 0,
            push_sound_cooldown: 0,
            floats_in_liquids: false,
            explosion: None,
            knockback_modifiers: DEFAULT_KNOCKBACK_MODIFIERS,
            sound_settings: SoundSettings::vanilla_default(),
            fuse: -1,
            contact_damages: Vec::new(),
        }
    }
}

impl SulfurCubeEntity {
    /// Vanilla `checkSulfurCubeSpawnRules`: always allowed.
    pub(crate) const SPAWN_RULE: SpawnRule =
        |_entity_type, _level, _spawn_reason, _pos, _random| true;

    /// Creates a new sulfur cube of size 1.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved sulfur cube.
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
        let mut entity_data = SulfurCubeEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        let y_rot = base.rotation().0;
        Self::register_goals(&mob_base);

        let cube = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base: AgeableMobBase::new(),
            cube_mob_base: CubeMobBase::new(y_rot),
            entity_data: SyncMutex::new(entity_data),
            state: SyncMutex::new(SulfurCubeState::default()),
        };
        cube.refresh_dimensions();
        cube
    }

    /// Vanilla `AbstractCubeMob.registerGoals` with the sulfur cube's
    /// behavior goals; it has no targeting goals.
    fn register_goals(mob_base: &MobBase) {
        let mut goal_selector = mob_base.goal_selector().lock();
        goal_selector.add_goal(1, CubeMobFloatGoal::new(mob_base));
        goal_selector.add_goal(4, CubeMobRandomDirectionGoal::new());
        goal_selector.add_goal(5, CubeMobKeepOnJumpingGoal);
        goal_selector.add_goal(2, goals::tempt_goal());
        goal_selector.add_goal(3, goals::SearchForItemsGoal::new());
    }

    /// Whether the cube has swallowed a block.
    #[must_use]
    pub fn has_body_item(&self) -> bool {
        self.living_base
            .equipment()
            .lock()
            .get_ref(EquipmentSlot::Body)
            .is_empty()
            .not()
    }

    /// Vanilla `fromBucket`.
    #[must_use]
    pub fn is_from_bucket(&self) -> bool {
        *self.entity_data.lock().from_bucket.get()
    }

    fn set_from_bucket(&self, from_bucket: bool) {
        self.entity_data.lock().from_bucket.set(from_bucket);
    }

    /// Vanilla `isSwallowableItem`.
    #[must_use]
    pub fn is_swallowable_item(stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(stack.item(), &ItemTag::SULFUR_CUBE_SWALLOWABLE)
    }

    pub(super) fn is_food(stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(stack.item(), &ItemTag::SULFUR_CUBE_FOOD)
    }

    /// Vanilla `setItemSlotAndDropWhenKilled`.
    fn set_body_and_drop_when_killed(&self, stack: ItemStack) {
        self.set_item_slot(EquipmentSlot::Body, stack);
        self.set_guaranteed_drop(EquipmentSlot::Body);
        self.set_persistence_required();
    }

    /// Spawns `stack` where a passenger would sit, as vanilla does for the
    /// swallowed block.
    fn spawn_at_passenger_point(&self, stack: ItemStack) {
        let dimensions = self.base.dimensions();
        let offset = dimensions
            .attachments
            .get_average(EntityAttachment::Passenger, dimensions);
        let _ = self.spawn_at_location_with_offset(stack, offset);
    }
}

impl Entity for SulfurCubeEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn movement_emission(&self) -> EntityMovementEmission {
        EntityMovementEmission::All
    }

    /// Vanilla `SulfurCube.isInvulnerableToPiercingWeapon`: a primed cube can
    /// always be pierced.
    fn is_invulnerable_to_piercing_weapon(&self) -> bool {
        self.is_invulnerable() && !self.is_primed()
    }

    fn tick(self: Arc<Self>) {
        self.tick_fuse();
        if self.is_removed() {
            return;
        }
        self.prime_when_on_powered_position();
        let entity = Arc::clone(&self).into_shared_entity();
        if let Some(living) = entity.as_living_entity() {
            living.tick_living_entity(&entity);
        }
        self.tick_landing();
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    fn set_removed(&self, reason: RemovalReason) {
        self.split_on_death();
        self.default_set_removed(reason);
    }

    fn push_entity(&self, entity: &dyn Entity) {
        self.cube_push(entity);
    }

    fn player_touch(self: Arc<Self>, player: &Arc<Player>) {
        let attacker: SharedEntity = Arc::clone(&self) as SharedEntity;
        self.cube_player_touch(&attacker, player.as_ref());
        self.player_push(player);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        self.cube_dimensions()
    }

    fn refresh_dimensions(&self) {
        self.refresh_cube_dimensions();
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn update_data_before_sync(&self) {
        sync_dirty_mob_effects(&self.living_base, &self.entity_data);
    }

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    fn max_up_step(&self) -> f32 {
        if self.has_body_item() {
            return 0.0;
        }
        self.attributes()
            .lock()
            .get_value(vanilla_attributes::STEP_HEIGHT)
            .unwrap_or(0.6) as f32
    }

    fn can_freeze(&self) -> bool {
        !self.has_body_item() && self.default_living_can_freeze()
    }

    fn get_fluid_jump_threshold(&self) -> f64 {
        f64::from(self.base.dimensions().height) * FLUID_JUMP_THRESHOLD_SCALE
    }

    fn play_step_sound(&self, _pos: BlockPos, block_state: BlockStateId) {
        if !self.has_body_item() {
            self.play_block_step_sound(block_state);
        }
    }

    /// Vanilla `applyImplicitComponents`: a bucket's swallowed block.
    fn apply_implicit_item_components(&self, item_stack: &ItemStack) {
        if let Some(content) = item_stack.get(SULFUR_CUBE_CONTENT) {
            self.set_body_and_drop_when_killed(content.absorbed_block_item_stack().create());
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_cube_mob(nbt);
        let state = self.state.lock();
        nbt.insert("pickup_timer", state.pickup_timer);
        nbt.insert("from_bucket", i8::from(self.is_from_bucket()));
        nbt.insert("fuse", state.fuse);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        {
            let mut state = self.state.lock();
            state.pickup_timer = nbt.int("pickup_timer").unwrap_or(0);
            state.fuse = nbt.int("fuse").unwrap_or(-1);
        }
        self.set_from_bucket(nbt.byte("from_bucket").is_some_and(|value| value != 0));
        let fuse = self.fuse();
        self.entity_data.lock().max_fuse.set(fuse);
        self.load_cube_mob_size(nbt);
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_cube_mob_was_on_ground(nbt);
    }
}

impl LivingEntity for SulfurCubeEntity {
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

    fn sound_volume(&self) -> f32 {
        self.cube_sound_volume()
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(if self.is_tiny() {
            &sound_events::ENTITY_SMALL_SULFUR_CUBE_HURT
        } else {
            &sound_events::ENTITY_SULFUR_CUBE_HURT
        })
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(if self.is_tiny() {
            &sound_events::ENTITY_SMALL_SULFUR_CUBE_DEATH
        } else {
            &sound_events::ENTITY_SULFUR_CUBE_DEATH
        })
    }

    fn jump_from_ground(&self) {
        self.cube_jump_from_ground(0.0);
    }

    fn hurt_server(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        if self.hurt_with_body_item(source, amount) {
            return true;
        }
        self.default_hurt_server(world, source, amount)
    }

    fn knockback_with_source(&self, power: f64, xd: f64, zd: f64, cause: &KnockbackCause<'_>) {
        if !self.knockback_with_body_item(power, xd, zd, cause) {
            self.knockback(power, xd, zd);
        }
    }

    fn before_equipment_changes(&self, changes: &[(EquipmentSlot, ItemStack, ItemStack)]) {
        for (slot, previous, current) in changes {
            if *slot == EquipmentSlot::Body {
                self.apply_body_item_change(previous, current);
            }
        }
    }

    fn can_breathe_underwater(&self) -> bool {
        self.has_body_item() || self.entity_type().flags.can_breathe_underwater
    }

    /// Vanilla `doPush`: pushing also applies the block's contact damage.
    fn push_entities_in_order(&self, entities: &[SharedEntity]) {
        EntityPush::push_entities_in_order(self.as_entity_event_source(), entities);
        for entity in entities {
            self.apply_contact_damage(entity.as_ref());
        }
    }

    fn travel_in_fluid(&self, entity: &SharedEntity, input: DVec3) -> Option<MoveResult> {
        let result = self.default_travel_in_fluid(entity, input);
        self.float_in_liquid();
        result
    }

    fn omnidirectional_air_mover(&self) -> bool {
        self.has_body_item()
    }

    fn get_equipment_slot_for_item(&self, item_stack: &ItemStack) -> EquipmentSlot {
        if Self::is_swallowable_item(item_stack) {
            return EquipmentSlot::Body;
        }
        match item_stack.get_equippable() {
            Some(equippable) if self.can_use_slot(equippable.slot) => equippable.slot,
            _ => EquipmentSlot::MainHand,
        }
    }

    fn is_equippable_in_slot(&self, item_stack: &ItemStack, slot: EquipmentSlot) -> bool {
        if slot == EquipmentSlot::Body {
            return Self::is_swallowable_item(item_stack);
        }
        let Some(equippable) = item_stack.get_equippable() else {
            return slot == EquipmentSlot::MainHand && self.can_use_slot(EquipmentSlot::MainHand);
        };
        slot == equippable.slot
            && self.can_use_slot(equippable.slot)
            && equippable.can_be_equipped_by(self.entity_type())
    }

    fn can_use_slot(&self, slot: EquipmentSlot) -> bool {
        slot != EquipmentSlot::Body || (LivingEntity::is_alive(self) && !AgeableMob::is_baby(self))
    }

    fn can_dispenser_equip_into_slot(&self, slot: EquipmentSlot) -> bool {
        slot == EquipmentSlot::Body
    }

    fn base_experience_reward(&self) -> i32 {
        if AgeableMob::is_baby(self) {
            0
        } else {
            1 + rand::random_range(0..2)
        }
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        let result = Mob::mob_ai_step(self, entity);
        AgeableMob::tick_ageable_mob(self);
        result
    }
}

impl Mob for SulfurCubeEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `SulfurCubeMobMoveControl`: a cube holding a block does not hop.
    fn tick_move_control(&self) {
        if !self.has_body_item() {
            self.tick_cube_move_control();
        }
    }

    /// Vanilla `SulfurCubeLookControl`: a cube holding a block squares up to
    /// the nearest right angle instead of looking around.
    fn tick_look_control(&self) {
        if !self.has_body_item() {
            self.default_tick_look_control();
            return;
        }
        let y_rot = self.rotation().0;
        let snapped = y_rot - steel_math::wrap_degrees_90(y_rot);
        self.set_rotation((snapped, self.rotation().1));
        self.set_y_head_rot(snapped);
    }

    fn max_head_x_rot(&self) -> f32 {
        0.0
    }

    fn custom_server_ai_step(&self) {
        let mut state = self.state.lock();
        if state.pickup_timer > 0 {
            state.pickup_timer -= 1;
        }
        if state.push_sound_cooldown > 0 {
            state.push_sound_cooldown -= 1;
        }
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let data = self.finalize_spawn_ageable_mob(world, spawn_reason, group_data);
        self.set_spawn_size(world);
        data
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        self.interact_with_player(player, hand)
    }

    fn requires_custom_persistence(&self) -> bool {
        self.is_passenger() || self.is_leashed() || self.has_body_item() || self.is_from_bucket()
    }

    fn mob_can_be_leashed(&self) -> bool {
        self.has_body_item()
    }

    fn can_pick_up_loot(&self) -> bool {
        !self.has_body_item()
    }

    fn can_hold_item(&self, item_stack: &ItemStack) -> bool {
        !self.has_body_item() && Self::is_swallowable_item(item_stack) && !AgeableMob::is_baby(self)
    }

    fn pick_up_item(&self, world: &Arc<World>, item_entity: &ItemEntity) {
        self.swallow_item_entity(world, item_entity);
    }
}

impl AgeableMob for SulfurCubeEntity {
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

    /// Vanilla `ageBoundaryReached`: a grown-up cube becomes full size.
    fn age_boundary_changed(&self, baby: bool) {
        self.refresh_dimensions();
        if !baby {
            self.set_size(ADULT_SIZE, true);
        }
    }

    /// Vanilla `getBreedOffspring`: a baby-sized cube.
    fn initialize_breed_offspring(&self, _partner: &dyn AgeableMob, offspring: &dyn AgeableMob) {
        if let Some(cube) = offspring.as_cube_mob() {
            cube.set_size(BABY_SIZE, true);
        }
    }
}

impl AbstractCubeMob for SulfurCubeEntity {
    fn cube_mob_base(&self) -> &CubeMobBase {
        &self.cube_mob_base
    }

    fn get_size(&self) -> i32 {
        *self.entity_data.lock().abstract_cube_mob.id_size.get()
    }

    fn set_synced_size(&self, size: i32) {
        self.entity_data.lock().abstract_cube_mob.id_size.set(size);
    }

    fn jump_sound(&self) -> SoundEventRef {
        if self.is_tiny() {
            &sound_events::ENTITY_SMALL_SULFUR_CUBE_JUMP
        } else {
            &sound_events::ENTITY_SULFUR_CUBE_JUMP
        }
    }

    fn squish_sound(&self) -> SoundEventRef {
        if self.is_tiny() {
            &sound_events::ENTITY_SMALL_SULFUR_CUBE_SQUISH
        } else if self.has_body_item() {
            &sound_events::ENTITY_SULFUR_CUBE_BOUNCE
        } else {
            &sound_events::ENTITY_SULFUR_CUBE_SQUISH
        }
    }

    fn apply_size_attributes(&self, _actual_size: i32, _requested_size: i32) {}

    /// Vanilla `SulfurCube.setSize`: a size-1 adult turns into a baby.
    fn set_size(&self, size: i32, update_health: bool) {
        self.cube_set_size(size, update_health);
        if update_health && size == BABY_SIZE && !AgeableMob::is_baby(self) {
            self.set_baby(true);
        }
    }

    /// Vanilla `setCubeMobHealth`: 4 health per size.
    fn set_cube_mob_health(&self, actual_size: i32) {
        self.attributes()
            .lock()
            .set_base_value(vanilla_attributes::MAX_HEALTH, f64::from(4 * actual_size));
    }

    /// Vanilla `setUpSplitCube`: the halves are babies.
    fn set_up_split_cube(&self, cube: &dyn AbstractCubeMob, half_size: i32, position: DVec3) {
        cube.set_size(half_size, true);
        cube.snap_to(position, rand::random::<f32>() * 360.0, 0.0);
        cube.set_baby(true);
    }

    fn get_split_count(&self) -> i32 {
        if self.is_primed() { 0 } else { SPLIT_COUNT }
    }

    fn can_deal_damage(&self) -> bool {
        false
    }

    /// Vanilla `setSpawnSize`: babies are size 1, adults size 2.
    fn set_spawn_size(&self, _world: &World) {
        if AgeableMob::is_baby(self) {
            self.set_size(BABY_SIZE, true);
        } else {
            self.set_size(ADULT_SIZE, true);
        }
    }
}

impl PathfinderMob for SulfurCubeEntity {}

impl Bucketable for SulfurCubeEntity {
    fn set_from_bucket_flag(&self, from_bucket: bool) {
        self.set_from_bucket(from_bucket);
    }

    /// Vanilla `SulfurCube.saveToBucketTag`.
    fn save_to_bucket_tag(&self, bucket: &mut ItemStack) {
        save_default_data_to_bucket_tag(self, bucket);
        let body = self.equipment_in_slot(EquipmentSlot::Body);
        if body.is_empty() {
            bucket.remove(SULFUR_CUBE_CONTENT);
        } else if let Ok(template) = ItemStackTemplate::from_stack(&body) {
            bucket.set(SULFUR_CUBE_CONTENT, SulfurCubeContent::new(template));
        }
        let age = self.get_age();
        let age_locked = self.is_age_locked();
        update_bucket_entity_data(bucket, |tag| {
            tag.insert("age", age);
            tag.insert("age_locked", i8::from(age_locked));
        });
    }

    /// Vanilla `SulfurCube.loadFromBucketTag`.
    fn load_from_bucket_tag(&self, tag: &NbtCompound) {
        load_default_data_from_bucket_tag(self, tag);
        self.set_age(tag.int("age").unwrap_or(0));
        self.set_age_locked(tag.byte("age_locked").is_some_and(|value| value != 0));
    }

    fn bucket_item_stack(&self) -> ItemStack {
        ItemStack::new(&vanilla_items::SULFUR_CUBE_BUCKET)
    }

    fn bucket_pickup_sound(&self) -> SoundEventRef {
        &sound_events::ITEM_BUCKET_FILL_SULFUR_CUBE
    }

    fn can_be_picked_up_with_bucket(&self, stack: &ItemStack) -> bool {
        stack.is(&vanilla_items::BUCKET)
    }
}

impl Shearable for SulfurCubeEntity {
    /// Vanilla `SulfurCube.shear`: spits the block back out.
    fn shear(&self, _world: &Arc<World>, _source: SoundSource, _tool: &ItemStack) {
        let swallowed = self.equipment_in_slot(EquipmentSlot::Body);
        self.set_item_slot(EquipmentSlot::Body, ItemStack::empty());
        self.spawn_at_passenger_point(swallowed);
        self.play_sound(&sound_events::ENTITY_SULFUR_CUBE_EJECT, 1.0, 1.0);
        self.state.lock().pickup_timer = PICKUP_TIMER_DURATION;
    }

    fn ready_for_shearing(&self) -> bool {
        self.has_body_item()
    }
}
