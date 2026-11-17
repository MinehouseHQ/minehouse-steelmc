//! Vanilla `Piglin`: the gold-loving Nether piglin that barters, admires gold,
//! hunts hoglins and attacks players who wear no gold.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use glam::DVec3;
use rand::Rng;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::data_components::vanilla_components::KINETIC_WEAPON;
use steel_registry::enchantment_effect::EnchantmentEffectComponent;
use steel_registry::entity_type::{
    EntityAttachmentPoint, EntityAttachments, EntityDimensions, EntityTypeRef,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::PiglinEntityData;
use steel_registry::vanilla_game_rules::MOB_GRIEFING;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{sound_events, vanilla_attributes, vanilla_blocks, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::random::Random as _;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::InteractionHand;
use steel_utils::{
    BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey, Identifier,
};

use super::anger::{TIME_BETWEEN_HUNTS, is_idle, is_loved_item};
use super::sensor::PiglinSpecificSensor;
use super::{AbstractPiglin, convert_to_zombified_piglin};
use crate::behavior::InteractionResult;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::memory_types::{
    ATE_RECENTLY, ATTACK_TARGET, HUNTED_RECENTLY, SPEAR_CHARGE_POSITION, SPEAR_ENGAGE_TIME,
    SPEAR_FLEEING_POSITION, SPEAR_FLEEING_TIME, UNIVERSAL_ANGER,
};
use crate::entity::ai::brain::sensors::{
    HurtBySensor, NearestItemSensor, NearestLivingEntitySensor, PlayerSensor,
};
use crate::entity::ai::brain::{AnyMemoryModuleType, Brain, Memories, Sensor};
use crate::entity::ai::path::PathType;
use crate::entity::attribute::{AttributeModifier, AttributeModifierOperation};
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::villagers::{load_inventory, save_inventory};
use crate::entity::entities::{HoglinEntity, ItemEntity};
use crate::entity::mob::spawn_equipment::populate_default_equipment_enchantments;
use crate::entity::spawn::SpawnGroupData;
use crate::entity::{
    CrossbowAttackMob, DefaultEntityPush, Entity, EntityBase, EntityBaseLoad,
    EntityMovementEmission, EntityPose, EntitySpawnReason, EntitySyncedData, LivingEntity,
    LivingEntityBase, LivingEntitySyncedData, Mob, MobBase, Monster, PathfinderMob,
    RangedAttackMob, SharedEntity, SpawnRule, start_riding_entities, sync_dirty_mob_effects,
};
use crate::inventory::container::{Container as _, SimpleContainer};
use crate::inventory::equipment::EquipmentSlot;
use crate::inventory::slot_access::SlotAccess;
use crate::inventory::slot_range::MOB_INVENTORY_SLOT_OFFSET;
use crate::physics::MoveResult;
use crate::player::Player;
use crate::player::advancement::criteria::on_item_pickup;
use crate::world::{LevelReader, ServerLevelAccessor, World};

const XP_REWARD: i32 = 5;
/// Vanilla `Piglin.INVENTORY_SIZE`.
const INVENTORY_SIZE: usize = 8;
/// Vanilla `Piglin.PROBABILITY_OF_SPAWNING_AS_BABY`.
const PROBABILITY_OF_SPAWNING_AS_BABY: f32 = 0.2;
/// Vanilla `Piglin.PROBABILITY_OF_SPAWNING_WITH_CROSSBOW_INSTEAD_OF_SWORD`.
const PROBABILITY_OF_SPAWNING_WITH_CROSSBOW: f32 = 0.5;
/// Vanilla's per-slot chance that a spawned piglin wears golden armor.
const CHANCE_OF_WEARING_EACH_ARMOR_ITEM: f32 = 0.1;
/// Vanilla `Piglin.MAX_PASSENGERS_ON_ONE_HOGLIN`.
const MAX_PASSENGERS_ON_ONE_HOGLIN: i32 = 3;
/// Vanilla `Piglin.performRangedAttack` -> `performCrossbowAttack(this, 1.6F)`.
const MOB_ARROW_POWER: f32 = 1.6;
const SPEED_MODIFIER_BABY_ID: Identifier = Identifier::vanilla_static("baby");
const SPEED_MODIFIER_BABY_AMOUNT: f64 = 0.2;
/// Vanilla `AbstractPiglin` pathfinding malus for fire next to the path.
const FIRE_IN_NEIGHBOR_MALUS: f32 = 16.0;
/// Vanilla `AbstractPiglin` pathfinding malus for walking into fire.
const FIRE_MALUS: f32 = -1.0;
const STEP_VOLUME: f32 = 0.15;

const BABY_VEHICLE_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    [EntityAttachmentPoint::new(0.0, 0.1875, 0.0)];
/// Vanilla `Piglin.BABY_DIMENSIONS`.
const BABY_DIMENSIONS: EntityDimensions = EntityDimensions::new_with_attachments(
    0.49,
    0.98,
    0.78,
    EntityAttachments::new(&[], &BABY_VEHICLE_ATTACHMENTS, &[], &[]),
);

/// Vanilla `Piglin.BRAIN_PROVIDER` memories the behaviors and sensors do not
/// register themselves.
static PIGLIN_MEMORIES: [&dyn AnyMemoryModuleType; 6] = [
    &UNIVERSAL_ANGER,
    &ATE_RECENTLY,
    &SPEAR_FLEEING_TIME,
    &SPEAR_FLEEING_POSITION,
    &SPEAR_CHARGE_POSITION,
    &SPEAR_ENGAGE_TIME,
];

/// A piglin.
#[entity_behavior(class = "Piglin", spawn_rule)]
pub struct PiglinEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<PiglinEntityData>,
    pub(super) brain: Brain<Self>,
    time_in_overworld: AtomicI32,
    inventory: SyncMutex<SimpleContainer>,
    cannot_hunt: SyncMutex<bool>,
}

// SAFETY: The owner-scoped type key uniquely identifies PiglinEntity.
unsafe impl DowncastType for PiglinEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/piglin");
}

impl PiglinEntity {
    /// Vanilla `Piglin.checkPiglinSpawnRules`: anywhere but on a nether wart
    /// block.
    pub(crate) const SPAWN_RULE: SpawnRule = Self::check_piglin_spawn_rules;

    fn check_piglin_spawn_rules(
        _entity_type: EntityTypeRef,
        level: &dyn ServerLevelAccessor,
        _spawn_reason: EntitySpawnReason,
        pos: BlockPos,
        _random: &mut dyn Rng,
    ) -> bool {
        level.get_block_state(pos.below()).get_block() != &vanilla_blocks::NETHER_WART_BLOCK
    }

    /// Creates a new piglin.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved piglin.
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
        let mut entity_data = PiglinEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            brain: Brain::new(&PIGLIN_MEMORIES, Self::sensors(), Self::activities(), None),
            time_in_overworld: AtomicI32::new(0),
            inventory: SyncMutex::new(SimpleContainer::new(INVENTORY_SIZE)),
            cannot_hunt: SyncMutex::new(false),
        }
    }

    fn sensors() -> Vec<Box<dyn Sensor<Self>>> {
        vec![
            Box::new(NearestLivingEntitySensor),
            Box::new(PlayerSensor),
            Box::new(NearestItemSensor),
            Box::new(HurtBySensor),
            Box::new(PiglinSpecificSensor),
        ]
    }

    /// The piglin's brain memories.
    #[must_use]
    pub const fn memories(&self) -> &SyncMutex<Memories> {
        self.brain.memories()
    }

    /// Vanilla `InventoryCarrier.getInventory`.
    #[must_use]
    pub const fn inventory(&self) -> &SyncMutex<SimpleContainer> {
        &self.inventory
    }

    /// Vanilla `isDancing`.
    #[must_use]
    pub fn is_dancing(&self) -> bool {
        *self.entity_data.lock().is_dancing.get()
    }

    /// Vanilla `setDancing`.
    pub fn set_dancing(&self, dancing: bool) {
        self.entity_data.lock().is_dancing.set(dancing);
    }

    /// Vanilla `isChargingCrossbow`.
    #[must_use]
    pub fn is_charging_crossbow(&self) -> bool {
        *self.entity_data.lock().is_charging_crossbow.get()
    }

    /// Vanilla `setTimeInOverworld`.
    pub fn set_time_in_overworld(&self, ticks: i32) {
        self.time_in_overworld.store(ticks, Ordering::Relaxed);
    }

    /// Vanilla `holdInMainHand`.
    pub(super) fn hold_in_main_hand(&self, item_stack: ItemStack) {
        self.set_item_slot(EquipmentSlot::MainHand, item_stack);
        self.set_guaranteed_drop(EquipmentSlot::MainHand);
        self.set_persistence_required();
    }

    /// Vanilla `holdInOffHand`: anything but a gold ingot keeps the piglin
    /// from despawning.
    pub(super) fn hold_in_off_hand(&self, item_stack: ItemStack) {
        let barter_currency = item_stack.is(&vanilla_items::GOLD_INGOT);
        self.set_item_slot(EquipmentSlot::OffHand, item_stack);
        self.set_guaranteed_drop(EquipmentSlot::OffHand);
        if !barter_currency {
            self.set_persistence_required();
        }
    }

    /// Vanilla `canReplaceCurrentItem(newItemStack)`: against whatever is in
    /// the slot the item would go to.
    pub(super) fn can_replace_current_item_in_its_slot(&self, item_stack: &ItemStack) -> bool {
        let slot = self.get_equipment_slot_for_item(item_stack);
        let current = self.equipment_in_slot(slot);
        self.can_replace_current_item(item_stack, &current, slot)
    }

    /// Vanilla `removeAllItems` on the inventory, dropping each stack.
    fn drop_inventory(&self) {
        let items = {
            let mut inventory = self.inventory.lock();
            let items = inventory.store_as_item_list();
            let _ = inventory.clear_content();
            items
        };
        for item_stack in items {
            let _ = self.spawn_at_location(item_stack, 0.0);
        }
    }

    /// Vanilla `createSpawnWeapon`.
    fn create_spawn_weapon(random: &mut LegacyRandom) -> ItemStack {
        if random.next_f32() < PROBABILITY_OF_SPAWNING_WITH_CROSSBOW {
            ItemStack::new(&vanilla_items::CROSSBOW)
        } else if random.next_i32_bounded(10) == 0 {
            ItemStack::new(&vanilla_items::GOLDEN_SPEAR)
        } else {
            ItemStack::new(&vanilla_items::GOLDEN_SWORD)
        }
    }

    /// Vanilla `populateDefaultEquipmentSlots`: each gold armor piece at 10%.
    fn populate_piglin_armor(&self, random: &mut LegacyRandom) {
        if self.is_baby() {
            return;
        }
        for (slot, item) in [
            (EquipmentSlot::Head, &vanilla_items::GOLDEN_HELMET),
            (EquipmentSlot::Chest, &vanilla_items::GOLDEN_CHESTPLATE),
            (EquipmentSlot::Legs, &vanilla_items::GOLDEN_LEGGINGS),
            (EquipmentSlot::Feet, &vanilla_items::GOLDEN_BOOTS),
        ] {
            if random.next_f32() < CHANCE_OF_WEARING_EACH_ARMOR_ITEM {
                self.set_item_slot(slot, ItemStack::new(item));
            }
        }
    }

    /// Vanilla `getTopPassenger`: a baby boards the top of a hoglin's stack,
    /// at most three riders high.
    fn top_passenger(vehicle: &SharedEntity, counter: i32) -> SharedEntity {
        match vehicle.passengers().into_iter().next() {
            Some(passenger) if counter != 1 => Self::top_passenger(&passenger, counter - 1),
            _ => Arc::clone(vehicle),
        }
    }
}

/// The `AbstractPiglin` constructor's mob setup: picks up loot, opens doors
/// and keeps away from fire.
pub(super) fn new_abstract_piglin_mob_base() -> MobBase {
    let mob_base = MobBase::new();
    *mob_base.can_pick_up_loot().lock() = true;
    mob_base.navigation().lock().set_can_open_doors(true);
    {
        let mut malus = mob_base.pathfinding_malus().lock();
        malus.set(PathType::FireInNeighbor, FIRE_IN_NEIGHBOR_MALUS);
        malus.set(PathType::Fire, FIRE_MALUS);
    }
    mob_base
}

impl DefaultEntityPush for PiglinEntity {}

impl Entity for PiglinEntity {
    /// Vanilla `Piglin.getSlot`: the inventory from slot 300.
    fn get_slot(&self, slot: i32) -> Option<SlotAccess<'_>> {
        SlotAccess::for_container(&self.inventory, slot - MOB_INVENTORY_SLOT_OFFSET)
            .or_else(|| self.default_living_slot(slot))
    }

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
        if self.is_baby() {
            BABY_DIMENSIONS.scale(scale)
        } else if self.entity_type.fixed {
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
        self.play_sound(&sound_events::ENTITY_PIGLIN_STEP, STEP_VOLUME, 1.0);
    }

    /// Vanilla `Piglin.startRiding`: a baby riding a hoglin climbs onto the
    /// top of its rider stack.
    fn start_riding(&self, entity_to_ride: &SharedEntity, force: bool) -> bool {
        let Some(world) = self.level() else {
            return false;
        };
        let Some(passenger) = world.get_entity_by_id(self.id()) else {
            return false;
        };
        let vehicle = if self.is_baby() && entity_to_ride.downcast_ref::<HoglinEntity>().is_some() {
            Self::top_passenger(entity_to_ride, MAX_PASSENGERS_ON_ONE_HOGLIN)
        } else {
            Arc::clone(entity_to_ride)
        };
        start_riding_entities(&passenger, &vehicle, force)
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
        nbt.insert("IsBaby", i8::from(self.is_baby()));
        nbt.insert("CannotHunt", i8::from(*self.cannot_hunt.lock()));
        save_inventory(&self.inventory, nbt);
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
        Mob::set_baby(self, nbt.byte("IsBaby").is_some_and(|value| value != 0));
        *self.cannot_hunt.lock() = nbt.byte("CannotHunt").is_some_and(|value| value != 0);
        load_inventory(&self.inventory, nbt);
    }
}

impl LivingEntity for PiglinEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn living_synced_data(&self) -> Option<&dyn LivingEntitySyncedData> {
        Some(&self.entity_data)
    }

    fn brain_memories(&self) -> Option<&SyncMutex<Memories>> {
        Some(self.brain.memories())
    }

    fn is_baby(&self) -> bool {
        *self.entity_data.lock().baby.get()
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
        Some(&sound_events::ENTITY_PIGLIN_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_PIGLIN_DEATH)
    }

    fn hurt_server(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        let was_hurt = self.default_hurt_server(world, source, amount);
        if was_hurt
            && let Some(attacker) = source.causing_entity()
            && attacker.is_living_entity()
        {
            self.was_hurt_by(world, attacker);
        }
        was_hurt
    }

    /// Vanilla `Piglin.dropCustomDeathLoot`: the inventory spills out.
    fn drop_custom_death_loot(&self, _source: &DamageSource, _killed_by_player: bool) {
        self.drop_inventory();
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

impl Mob for PiglinEntity {
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

    /// Vanilla `Piglin.setBaby`: the synced flag plus a transient `+20%`
    /// speed modifier.
    fn set_baby(&self, baby: bool) {
        let changed = {
            let mut entity_data = self.entity_data.lock();
            let changed = *entity_data.baby.get() != baby;
            entity_data.baby.set(baby);
            changed
        };
        {
            let mut attributes = self.attributes().lock();
            let _ = attributes
                .remove_modifier(vanilla_attributes::MOVEMENT_SPEED, &SPEED_MODIFIER_BABY_ID);
            if baby {
                let _ = attributes.add_modifier(
                    vanilla_attributes::MOVEMENT_SPEED,
                    AttributeModifier {
                        id: SPEED_MODIFIER_BABY_ID,
                        amount: SPEED_MODIFIER_BABY_AMOUNT,
                        operation: AttributeModifierOperation::AddMultipliedBase,
                    },
                    false,
                );
            }
        }
        // Vanilla `onSyncedDataUpdated(DATA_BABY_ID)`.
        if changed {
            self.refresh_dimensions();
        }
    }

    fn custom_server_ai_step(&self) {
        if let Some(world) = self.level() {
            self.brain.tick(&world, self);
        }
        self.update_activity();
        self.tick_zombification();
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        self.sound_for_current_activity()
    }

    /// Vanilla `AbstractPiglin.playAmbientSound`: only while idle.
    fn play_ambient_sound(&self) {
        if is_idle(self) {
            self.make_sound(self.ambient_sound());
        }
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        self.mob_interact_piglin(player, hand)
    }

    /// Vanilla `Piglin.wantsToPickUp`.
    fn wants_to_pick_up(&self, item_stack: &ItemStack) -> bool {
        self.level()
            .is_some_and(|world| world.get_game_rule(&MOB_GRIEFING))
            && self.can_pick_up_loot()
            && self.wants_to_pickup(item_stack)
    }

    fn pick_up_item(&self, world: &Arc<World>, item_entity: &ItemEntity) {
        on_item_pickup(self, item_entity, &item_entity.get_item());
        self.pick_up_item_piglin(world, item_entity);
    }

    /// Vanilla `Piglin.canReplaceCurrentItem`: loved items and the preferred
    /// weapons beat anything else.
    fn can_replace_current_item(
        &self,
        new_item_stack: &ItemStack,
        current_item_stack: &ItemStack,
        slot: EquipmentSlot,
    ) -> bool {
        if current_item_stack.has_enchantment_effect(EnchantmentEffectComponent::PreventArmorChange)
        {
            return false;
        }
        let preferred = self.get_preferred_weapon_type();
        let wanted = |item_stack: &ItemStack| {
            is_loved_item(item_stack)
                || preferred
                    .as_ref()
                    .is_some_and(|tag| item_stack.item().has_tag(tag))
        };
        match (wanted(new_item_stack), wanted(current_item_stack)) {
            (true, false) => true,
            (false, true) => false,
            _ => self.default_can_replace_current_item(new_item_stack, current_item_stack, slot),
        }
    }

    fn get_preferred_weapon_type(&self) -> Option<Identifier> {
        (!self.is_baby()).then_some(ItemTag::PIGLIN_PREFERRED_WEAPONS)
    }

    /// Vanilla `Piglin.canUseNonMeleeWeapon`: crossbows and spears.
    fn can_use_non_melee_weapon(&self, item: &ItemStack) -> bool {
        item.is(&vanilla_items::CROSSBOW) || item.has(KINETIC_WEAPON)
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let mut random = LegacyRandom::from_seed(rand::random());
        if spawn_reason != EntitySpawnReason::Structure {
            if random.next_f32() < PROBABILITY_OF_SPAWNING_AS_BABY {
                Mob::set_baby(self, true);
            } else if !self.is_baby() {
                self.set_item_slot(
                    EquipmentSlot::MainHand,
                    Self::create_spawn_weapon(&mut random),
                );
            }
        }
        // Vanilla `PiglinAi.initMemories`.
        self.memories().lock().set_with_expiry(
            &HUNTED_RECENTLY,
            true,
            i64::from(TIME_BETWEEN_HUNTS.sample()),
        );
        self.populate_piglin_armor(&mut random);
        let difficulty = world.current_difficulty_at(self.block_position());
        populate_default_equipment_enchantments(self, &mut random, difficulty);
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

impl PathfinderMob for PiglinEntity {
    fn get_walk_target_value_in(&self, pos: BlockPos, level: &dyn LevelReader) -> f32 {
        Monster::get_walk_target_value_monster(self, pos, level)
    }
}

impl Monster for PiglinEntity {}

impl AbstractPiglin for PiglinEntity {
    fn can_hunt(&self) -> bool {
        !*self.cannot_hunt.lock()
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
        &sound_events::ENTITY_PIGLIN_CONVERTED_TO_ZOMBIFIED
    }

    /// Vanilla `Piglin.finishConversion`: drops the admired item and the
    /// inventory first.
    fn finish_conversion(&self) {
        self.cancel_admiring();
        self.drop_inventory();
        convert_to_zombified_piglin(self);
    }
}

impl RangedAttackMob for PiglinEntity {
    fn perform_ranged_attack(&self, entity: &SharedEntity, _target: &SharedEntity, _power: f32) {
        self.perform_crossbow_attack(entity, MOB_ARROW_POWER);
    }
}

impl CrossbowAttackMob for PiglinEntity {
    fn set_charging_crossbow(&self, is_charging: bool) {
        self.entity_data
            .lock()
            .is_charging_crossbow
            .set(is_charging);
    }

    fn on_crossbow_attack_performed(&self) {
        self.set_no_action_time(0);
    }
}
