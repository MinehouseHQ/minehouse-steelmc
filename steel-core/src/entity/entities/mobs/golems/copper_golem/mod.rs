//! Vanilla `CopperGolem`: built from a copper block and a carved pumpkin, it
//! carries items from copper chests to chests, oxidizes over time and turns
//! into a statue once fully oxidized.

mod ai;

use std::sync::{Arc, Weak};

use glam::DVec3;
use rand::RngExt as _;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, ChestType, Direction, Pose};
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::{SoundEventHolder, SoundEventRef};
use steel_registry::vanilla_entity_data::CopperGolemEntityData;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    level_events, sound_events, vanilla_attributes, vanilla_blocks, vanilla_entities,
    vanilla_game_events, vanilla_game_rules, vanilla_items,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey};
use text_components::TextComponent;
use uuid::Uuid;

use crate::behavior::InteractionResult;
use crate::behavior::blocks::{WeatherState, connected_direction};
use crate::block_entity::container_openers::ContainerUser;
use crate::block_entity::entities::CopperGolemStatueBlockEntity;
use crate::entity::ai::brain::behaviors::utils::throw_item;
use crate::entity::ai::brain::memory_types::TRANSPORT_ITEMS_COOLDOWN_TICKS;
use crate::entity::ai::brain::{Brain, Memories};
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::entities::LightningBoltEntity;
use crate::entity::leash::Leashable as _;
use crate::entity::spawn::SpawnGroupData;
use crate::entity::{
    DefaultEntityPush, Entity, EntityBase, EntityBaseLoad, EntityMovementEmission, EntityPose,
    EntitySpawnReason, EntitySyncedData, EquipmentSlot, LivingEntity, LivingEntityBase, Mob,
    MobBase, PathfinderMob, RemovalReason, SharedEntity, Shearable, next_entity_id,
    sync_dirty_mob_effects,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;

/// Vanilla `IGNORE_WEATHERING_TICK`: the golem is waxed.
const IGNORE_WEATHERING_TICK: i64 = -2;
/// Vanilla `UNSET_WEATHERING_TICK`: the next oxidation is not scheduled yet.
const UNSET_WEATHERING_TICK: i64 = -1;
const WEATHERING_TICK_FROM: i64 = 504_000;
const WEATHERING_TICK_TO: i64 = 552_000;
const TURN_TO_STATUE_CHANCE: f32 = 0.0058;
const SPAWN_COOLDOWN_MIN: i32 = 60;
const SPAWN_COOLDOWN_MAX: i32 = 100;
/// Vanilla `getContainerInteractionRange`.
const CONTAINER_INTERACTION_RANGE: f64 = 3.0;
/// Vanilla `getRequiredPathLength`.
const REQUIRED_PATH_LENGTH: f32 = 48.0;
const DEFAULT_STEP_HEIGHT: f32 = 0.6;
/// Vanilla `AbstractGolem.getAmbientSoundInterval`.
const GOLEM_AMBIENT_SOUND_INTERVAL: i32 = 120;
/// Vanilla `spawnAtLocation(level, antenna, 1.5F)`.
const ANTENNA_DROP_HEIGHT: f64 = 1.5;
/// Vanilla `BehaviorUtils.throwItem` default throw velocity, `0.3F`.
const THROW_VELOCITY: f32 = 0.3;
/// Vanilla `BehaviorUtils.throwItem` default hand height below the eyes.
const THROW_HAND_Y_DISTANCE_FROM_EYE: f32 = 0.3;
/// Vanilla `EQUIPMENT_SLOT_ANTENNA`: the poppy on its head.
pub const EQUIPMENT_SLOT_ANTENNA: EquipmentSlot = EquipmentSlot::Saddle;

/// Vanilla `CopperGolemState`: what the golem shows doing at a chest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopperGolemState {
    /// Walking around.
    Idle = 0,
    /// Taking items out of a copper chest.
    GettingItem = 1,
    /// Reaching into an empty copper chest.
    GettingNoItem = 2,
    /// Putting items into a chest.
    DroppingItem = 3,
    /// Holding items over a chest that does not take them.
    DroppingNoItem = 4,
}

/// The oxidation stage at a synced ordinal, out-of-range values unaffected
/// as vanilla's id map does.
const fn weather_state_by_id(id: i32) -> WeatherState {
    match id {
        1 => WeatherState::Exposed,
        2 => WeatherState::Weathered,
        3 => WeatherState::Oxidized,
        _ => WeatherState::Unaffected,
    }
}

/// Vanilla `WeatherState.getSerializedName`.
const fn weather_state_name(state: WeatherState) -> &'static str {
    match state {
        WeatherState::Unaffected => "unaffected",
        WeatherState::Exposed => "exposed",
        WeatherState::Weathered => "weathered",
        WeatherState::Oxidized => "oxidized",
    }
}

fn weather_state_from_name(name: &str) -> Option<WeatherState> {
    [
        WeatherState::Unaffected,
        WeatherState::Exposed,
        WeatherState::Weathered,
        WeatherState::Oxidized,
    ]
    .into_iter()
    .find(|state| weather_state_name(*state) == name)
}

/// Vanilla `WeatherState.next`, staying oxidized.
const fn next_weather_state(state: WeatherState) -> WeatherState {
    weather_state_by_id(if state as i32 == 3 {
        3
    } else {
        state as i32 + 1
    })
}

/// Vanilla `WeatherState.previous`, staying unaffected.
const fn previous_weather_state(state: WeatherState) -> WeatherState {
    weather_state_by_id(if state as i32 == 0 {
        0
    } else {
        state as i32 - 1
    })
}

/// Vanilla `CopperGolemOxidationLevels`: the hurt, death, step and head spin
/// sounds of each oxidation stage.
const fn oxidation_sounds(state: WeatherState) -> [SoundEventRef; 4] {
    match state {
        WeatherState::Unaffected | WeatherState::Exposed => [
            &sound_events::ENTITY_COPPER_GOLEM_HURT,
            &sound_events::ENTITY_COPPER_GOLEM_DEATH,
            &sound_events::ENTITY_COPPER_GOLEM_STEP,
            &sound_events::ENTITY_COPPER_GOLEM_SPIN,
        ],
        WeatherState::Weathered => [
            &sound_events::ENTITY_COPPER_GOLEM_WEATHERED_HURT,
            &sound_events::ENTITY_COPPER_GOLEM_WEATHERED_DEATH,
            &sound_events::ENTITY_COPPER_GOLEM_WEATHERED_STEP,
            &sound_events::ENTITY_COPPER_GOLEM_WEATHERED_SPIN,
        ],
        WeatherState::Oxidized => [
            &sound_events::ENTITY_COPPER_GOLEM_OXIDIZED_HURT,
            &sound_events::ENTITY_COPPER_GOLEM_OXIDIZED_DEATH,
            &sound_events::ENTITY_COPPER_GOLEM_OXIDIZED_STEP,
            &sound_events::ENTITY_COPPER_GOLEM_OXIDIZED_SPIN,
        ],
    }
}

/// The golem's own server state.
struct CopperGolemData {
    opened_chest_pos: Option<BlockPos>,
    last_lightning_bolt: Option<Uuid>,
    next_weathering_tick: i64,
}

/// A copper golem.
#[entity_behavior(class = "CopperGolem")]
pub struct CopperGolemEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<CopperGolemEntityData>,
    brain: Brain<Self>,
    data: SyncMutex<CopperGolemData>,
}

// SAFETY: The owner-scoped type key uniquely identifies CopperGolemEntity.
unsafe impl DowncastType for CopperGolemEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/copper_golem");
}

impl CopperGolemEntity {
    /// Creates a new copper golem.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved copper golem.
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
        let follow_range = living_base
            .attributes()
            .lock()
            .get_value(vanilla_attributes::FOLLOW_RANGE)
            .unwrap_or(vanilla_attributes::FOLLOW_RANGE.default_value);
        {
            let mut navigation = mob_base.navigation().lock();
            navigation.set_required_path_length(REQUIRED_PATH_LENGTH, follow_range);
            navigation.set_can_open_doors(true);
        }
        let mut entity_data = CopperGolemEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        let golem = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            brain: Brain::new(&ai::memories(), ai::sensors(), ai::activities(), None),
            data: SyncMutex::new(CopperGolemData {
                opened_chest_pos: None,
                last_lightning_bolt: None,
                next_weathering_tick: UNSET_WEATHERING_TICK,
            }),
        };
        golem.set_persistence_required();
        golem.set_pathfinding_malus(PathType::FireInNeighbor, 16.0);
        golem.set_pathfinding_malus(PathType::DamagingInNeighbor, 16.0);
        golem.set_pathfinding_malus(PathType::Fire, -1.0);
        golem.brain.memories().lock().set(
            &TRANSPORT_ITEMS_COOLDOWN_TICKS,
            Some(rand::rng().random_range(SPAWN_COOLDOWN_MIN..SPAWN_COOLDOWN_MAX)),
        );
        golem
    }

    /// Vanilla `CopperGolemStatueBlockEntity.removeStatue`: the golem a
    /// statue at `pos` turns back into, facing the statue's way and keeping
    /// its name.
    #[must_use]
    pub fn from_statue(
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        name: Option<TextComponent>,
    ) -> SharedEntity {
        let golem = Self::new(
            &vanilla_entities::COPPER_GOLEM,
            next_entity_id(),
            DVec3::new(
                f64::from(pos.x()) + 0.5,
                f64::from(pos.y()),
                f64::from(pos.z()) + 0.5,
            ),
            Arc::downgrade(world),
        );
        golem.set_custom_name(name);
        let yaw = state
            .get_value(&BlockStateProperties::HORIZONTAL_FACING)
            .to_yaw();
        golem.set_rotation((yaw, 0.0));
        golem.base().set_old_rotation_to_current();
        golem.set_y_head_rot(yaw);
        golem.set_y_body_rot(yaw);
        golem.play_spawn_sound();
        Arc::new(golem)
    }

    pub(super) const fn brain(&self) -> &Brain<Self> {
        &self.brain
    }

    /// Vanilla `getState`.
    #[must_use]
    pub fn state(&self) -> CopperGolemState {
        let state = *self.entity_data.lock().copper_golem_state.get();
        match state {
            1 => CopperGolemState::GettingItem,
            2 => CopperGolemState::GettingNoItem,
            3 => CopperGolemState::DroppingItem,
            4 => CopperGolemState::DroppingNoItem,
            _ => CopperGolemState::Idle,
        }
    }

    /// Vanilla `setState`.
    pub fn set_state(&self, state: CopperGolemState) {
        self.entity_data.lock().copper_golem_state.set(state as i32);
    }

    /// Vanilla `getWeatherState`.
    #[must_use]
    pub fn weather_state(&self) -> WeatherState {
        weather_state_by_id(*self.entity_data.lock().weather_state.get())
    }

    /// Vanilla `setWeatherState`.
    pub fn set_weather_state(&self, state: WeatherState) {
        self.entity_data.lock().weather_state.set(state as i32);
    }

    /// Vanilla `entityData.set(DATA_WEATHER_STATE, state, true)`: resends the
    /// stage even when it did not change.
    fn force_weather_state(&self, state: WeatherState) {
        self.entity_data
            .lock()
            .weather_state
            .set_forced(state as i32);
    }

    /// Vanilla `setOpenedChestPos` / `clearOpenedChestPos`.
    pub fn set_opened_chest_pos(&self, pos: Option<BlockPos>) {
        self.data.lock().opened_chest_pos = pos;
    }

    /// Whether honeycomb stopped the golem from oxidizing.
    #[must_use]
    pub fn is_waxed(&self) -> bool {
        self.data.lock().next_weathering_tick == IGNORE_WEATHERING_TICK
    }

    /// Vanilla `spawn`: a golem just built from a copper block of `state`.
    pub fn spawn(&self, state: WeatherState) {
        self.set_weather_state(state);
        self.play_spawn_sound();
    }

    /// Vanilla `playSpawnSound`.
    pub fn play_spawn_sound(&self) {
        self.play_sound(&sound_events::ENTITY_COPPER_GOLEM_SPAWN, 1.0, 1.0);
    }

    /// Vanilla `level.playSound(null, this, sound, source, 1, 1)`: a sound
    /// that follows the golem.
    fn play_following_sound(&self, world: &World, sound: SoundEventRef, source: SoundSource) {
        world.play_sound_from_entity(
            &SoundEventHolder::Registry(sound),
            source,
            self.id(),
            self.position(),
            1.0,
            1.0,
            None,
        );
    }

    /// Vanilla `updateWeathering`.
    fn update_weathering(&self, world: &Arc<World>) {
        let game_time = world.game_time();
        let mut rng = rand::rng();
        let fully_oxidized = {
            let mut data = self.data.lock();
            match data.next_weathering_tick {
                IGNORE_WEATHERING_TICK => return,
                UNSET_WEATHERING_TICK => {
                    data.next_weathering_tick =
                        game_time + rng.random_range(WEATHERING_TICK_FROM..=WEATHERING_TICK_TO);
                    return;
                }
                next_tick => {
                    let state = self.weather_state();
                    let fully_oxidized = state == WeatherState::Oxidized;
                    if game_time >= next_tick && !fully_oxidized {
                        let new_state = next_weather_state(state);
                        self.set_weather_state(new_state);
                        data.next_weathering_tick = if new_state == WeatherState::Oxidized {
                            0
                        } else {
                            next_tick + rng.random_range(WEATHERING_TICK_FROM..=WEATHERING_TICK_TO)
                        };
                    }
                    fully_oxidized
                }
            }
        };
        if fully_oxidized && self.can_turn_to_statue(world) {
            self.turn_to_statue(world);
        }
    }

    /// Vanilla `canTurnToStatue`.
    fn can_turn_to_statue(&self, world: &World) -> bool {
        world.get_block_state(self.block_position()).is_air()
            && rand::random::<f32>() <= TURN_TO_STATUE_CHANCE
    }

    /// Vanilla `turnToStatue`: leaves an oxidized statue in a random pose,
    /// facing the golem's way.
    fn turn_to_statue(&self, world: &Arc<World>) {
        let pos = self.block_position();
        let poses = [Pose::Standing, Pose::Sitting, Pose::Running, Pose::Star];
        let pose = poses[rand::rng().random_range(0..poses.len())];
        let statue = vanilla_blocks::OXIDIZED_COPPER_GOLEM_STATUE
            .default_state()
            .set_value(&BlockStateProperties::COPPER_GOLEM_POSE, pose)
            .set_value(
                &BlockStateProperties::HORIZONTAL_FACING,
                Direction::from_yaw(self.rotation().0),
            );
        world.set_block(pos, statue, UpdateFlags::UPDATE_ALL);
        let Some(block_entity) = world.get_block_entity(pos) else {
            return;
        };
        let Some(statue) = block_entity.downcast_ref::<CopperGolemStatueBlockEntity>() else {
            return;
        };
        statue.create_statue(self.custom_name());
        self.drop_preserved_equipment(&|_| true);
        self.set_removed(RemovalReason::Discarded);
        self.play_sound(&sound_events::ENTITY_COPPER_GOLEM_BECOME_STATUE, 1.0, 1.0);
        if self.is_leashed() {
            if world.get_game_rule(&vanilla_game_rules::ENTITY_DROPS) {
                self.drop_leash();
            } else {
                self.remove_leash();
            }
        }
    }

    /// The honeycomb, axe and empty-hand uses of vanilla `mobInteract` that
    /// change the golem's oxidation.
    fn interact_with_copper(
        &self,
        world: &World,
        player: &Player,
        hand: InteractionHand,
        item_stack: &ItemStack,
    ) -> InteractionResult {
        let pos = self.block_position();
        let waxed = self.is_waxed();
        if item_stack.is(&vanilla_items::HONEYCOMB) && !waxed {
            world.level_event(level_events::PARTICLES_WAX_ON, pos, 0, None);
            world.play_sound(
                &sound_events::ITEM_HONEYCOMB_WAX_ON,
                SoundSource::Blocks,
                pos,
                1.0,
                1.0,
                None,
            );
            self.data.lock().next_weathering_tick = IGNORE_WEATHERING_TICK;
            self.use_player_item(player, hand);
            return InteractionResult::SuccessServer;
        }
        if !item_stack.item().has_tag(&ItemTag::AXES) {
            return InteractionResult::Pass;
        }
        let level_event = if waxed {
            level_events::PARTICLES_WAX_OFF
        } else {
            let state = self.weather_state();
            if state == WeatherState::Unaffected {
                return InteractionResult::Pass;
            }
            self.force_weather_state(previous_weather_state(state));
            level_events::PARTICLES_SCRAPE
        };
        self.play_following_sound(world, &sound_events::ITEM_AXE_SCRAPE, self.sound_source());
        world.level_event(level_event, pos, 0, None);
        self.data.lock().next_weathering_tick = UNSET_WEATHERING_TICK;
        player.hurt_item_in_hand(hand, 1);
        InteractionResult::SuccessServer
    }

    fn update_dirty_mob_effect_entity_data(&self) {
        sync_dirty_mob_effects(&self.living_base, &self.entity_data);
    }
}

impl ContainerUser for CopperGolemEntity {
    /// Vanilla `hasContainerOpen`: the chest it opened, or that chest's other
    /// half.
    fn has_container_open(&self, world: &World, pos: BlockPos) -> bool {
        let Some(opened) = self.data.lock().opened_chest_pos else {
            return false;
        };
        if opened == pos {
            return true;
        }
        let state = world.get_block_state(opened);
        state
            .try_get_value(&BlockStateProperties::CHEST_TYPE)
            .is_some_and(|chest_type| chest_type != ChestType::Single)
            && opened.relative(connected_direction(state)) == pos
    }

    fn container_interaction_range(&self) -> f64 {
        CONTAINER_INTERACTION_RANGE
    }
}

impl Shearable for CopperGolemEntity {
    /// Vanilla `CopperGolem.shear`: drops the flower on its head.
    fn shear(&self, world: &Arc<World>, sound_source: SoundSource, _tool: &ItemStack) {
        self.play_following_sound(
            world,
            &sound_events::ENTITY_COPPER_GOLEM_SHEAR,
            sound_source,
        );
        let antenna = self.equipment_in_slot(EQUIPMENT_SLOT_ANTENNA);
        self.set_item_slot(EQUIPMENT_SLOT_ANTENNA, ItemStack::empty());
        let _ = self.spawn_at_location(antenna, ANTENNA_DROP_HEIGHT);
    }

    fn ready_for_shearing(&self) -> bool {
        self.equipment_in_slot(EQUIPMENT_SLOT_ANTENNA)
            .item()
            .has_tag(&ItemTag::SHEARABLE_FROM_COPPER_GOLEM)
    }
}

impl DefaultEntityPush for CopperGolemEntity {}

impl Entity for CopperGolemEntity {
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
        self.update_dirty_mob_effect_entity_data();
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "vanilla reads the step height attribute as a float"
    )]
    fn max_up_step(&self) -> f32 {
        self.attributes()
            .lock()
            .get_value(vanilla_attributes::STEP_HEIGHT)
            .unwrap_or(f64::from(DEFAULT_STEP_HEIGHT)) as f32
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(oxidation_sounds(self.weather_state())[2], 1.0, 1.0);
    }

    /// Vanilla `CopperGolem.thunderHit`: each bolt scrapes one stage off.
    fn thunder_hit(&self, world: &World, lightning: &LightningBoltEntity) {
        self.default_thunder_hit(world);
        let bolt = lightning.uuid();
        let mut data = self.data.lock();
        if data.last_lightning_bolt == Some(bolt) {
            return;
        }
        data.last_lightning_bolt = Some(bolt);
        let state = self.weather_state();
        if state != WeatherState::Unaffected {
            data.next_weathering_tick = UNSET_WEATHERING_TICK;
            self.force_weather_state(previous_weather_state(state));
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("next_weather_age", self.data.lock().next_weathering_tick);
        nbt.insert("weather_state", weather_state_name(self.weather_state()));
        nbt.insert("Brain", self.brain.save());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.data.lock().next_weathering_tick = nbt
            .long("next_weather_age")
            .unwrap_or(UNSET_WEATHERING_TICK);
        let state = nbt
            .string("weather_state")
            .and_then(|name| weather_state_from_name(&name.to_str()))
            .unwrap_or(WeatherState::Unaffected);
        self.set_weather_state(state);
        if let Some(brain) = nbt.compound("Brain") {
            self.brain.memories().lock().load(brain);
        }
    }
}

impl LivingEntity for CopperGolemEntity {
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

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(oxidation_sounds(self.weather_state())[0])
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(oxidation_sounds(self.weather_state())[1])
    }

    /// Vanilla `AbstractGolem.decreaseAirSupply`: golems never drown.
    fn decrease_air_supply(&self, current_supply: i32) -> i32 {
        current_supply
    }

    /// Vanilla `CopperGolem.actuallyHurt`: a hit interrupts the chest work.
    fn actually_hurt(&self, world: &World, source: &DamageSource, amount: f32) {
        self.default_actually_hurt(world, source, amount);
        self.set_state(CopperGolemState::Idle);
    }

    /// Vanilla `CopperGolem.dropEquipment`: drops the flower and what it
    /// carries.
    fn drop_equipment(&self) {
        self.drop_preserved_equipment(&|_| true);
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        let result = Mob::mob_ai_step(self, entity);
        if let Some(world) = self.level()
            && !self.is_removed()
        {
            self.update_weathering(&world);
        }
        result
    }
}

impl Mob for CopperGolemEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn custom_server_ai_step(&self) {
        if let Some(world) = self.level() {
            self.brain.tick(&world, self);
            ai::update_activity(self);
        }
    }

    fn ambient_sound_interval(&self) -> i32 {
        GOLEM_AMBIENT_SOUND_INTERVAL
    }

    fn remove_when_far_away(&self, _dist_sqr: f64) -> bool {
        false
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.play_spawn_sound();
        self.finalize_spawn_mob_base(world, spawn_reason, group_data)
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let item_stack = player.inventory.lock().get_item_in_hand(hand).clone();
        if item_stack.is_empty() {
            let carried = self.equipment_in_slot(EquipmentSlot::MainHand);
            if !carried.is_empty() {
                throw_item(
                    self,
                    carried,
                    player.position(),
                    DVec3::splat(f64::from(THROW_VELOCITY)),
                    THROW_HAND_Y_DISTANCE_FROM_EYE,
                );
                self.set_item_slot(EquipmentSlot::MainHand, ItemStack::empty());
                return InteractionResult::Success;
            }
        }
        let Some(world) = self.level() else {
            return InteractionResult::Pass;
        };
        if item_stack.is(&vanilla_items::SHEARS) && self.ready_for_shearing() {
            self.shear(&world, SoundSource::Players, &item_stack);
            self.game_event_with_source_entity(&vanilla_game_events::SHEAR, Some(player));
            player.hurt_item_in_hand(hand, 1);
            return InteractionResult::Success;
        }
        self.interact_with_copper(&world, player, hand, &item_stack)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for CopperGolemEntity {}

#[cfg(test)]
mod tests;
