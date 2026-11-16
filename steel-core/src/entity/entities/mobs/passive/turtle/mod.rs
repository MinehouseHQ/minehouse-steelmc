//! Vanilla `Turtle`: an amphibious animal that always returns to the beach
//! it hatched on to lay its eggs.

mod goals;
#[cfg(test)]
mod tests;

use std::f64::consts::PI;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_macros::entity_behavior;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::{
    EntityAttachmentPoint, EntityAttachments, EntityDimensions, EntityTypeRef,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::loot_table::LootContext;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_data::TurtleEntityData;
use steel_registry::vanilla_game_rules::MOB_DROPS;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _, level_events, sound_events, vanilla_attributes,
    vanilla_blocks, vanilla_damage_types, vanilla_entities, vanilla_game_events,
    vanilla_loot_tables,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use self::goals::{
    TurtleBreedGoal, TurtleGoHomeGoal, TurtleGoToWaterGoal, TurtleLayEggGoal,
    TurtleRandomStrollGoal, TurtleTravelGoal,
};
use crate::behavior::InteractionResult;
use crate::entity::ai::control::MoveControlOperation;
use crate::entity::ai::goal::{LookAtPlayerGoal, PanicGoal, TemptGoal};
use crate::entity::ai::navigation::PathNavigation;
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::entities::{ExperienceOrbEntity, LightningBoltEntity};
use crate::entity::mob::rotlerp;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, DefaultEntityPush, Entity, EntityBase,
    EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, PathfinderMob, SharedEntity, SpawnGroupData, SpawnRule,
    entity_loot_ref, living_loot_equipment, next_entity_id, sync_dirty_mob_effects,
};
use crate::fluid::FluidStateExt as _;
use crate::physics::{MoveResult, MoverType};
use crate::player::Player;
use crate::world::{LevelReader, World};

/// Vanilla `BABY_SCALE`.
const BABY_SCALE: f32 = 0.3;
/// Vanilla `EntityTypes.TURTLE` height, used for the baby rider anchor.
const TURTLE_HEIGHT: f32 = 0.4;
const BABY_PASSENGER_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    [EntityAttachmentPoint::new(0.0, TURTLE_HEIGHT as f64, -0.25)];
const TAG_HOME_POS: &str = "home_pos";
const TAG_HAS_EGG: &str = "has_egg";
/// Vanilla `getAmbientSoundInterval`.
const AMBIENT_SOUND_INTERVAL: i32 = 200;
/// Age, in ticks, of a turtle fresh out of its egg.
pub const HATCHLING_AGE: i32 = -24000;
/// Ticks both parents wait after mating (vanilla `setAge(6000)`).
const PARENT_AGE_AFTER_MATING: i32 = 6000;
/// Vanilla `setInLoveTime(600)` after laying.
const IN_LOVE_AFTER_LAYING: i32 = 600;

/// Server-side turtle state outside the synced data.
#[derive(Debug, Clone, Copy)]
struct TurtleState {
    /// Vanilla `homePos`: the beach the turtle returns to.
    home_pos: BlockPos,
    /// Vanilla `travelPos`: the far-off water target of `TurtleTravelGoal`.
    travel_pos: Option<BlockPos>,
    /// Vanilla `goingHome`.
    going_home: bool,
    /// Vanilla `layEggCounter`.
    lay_egg_counter: i32,
}

/// Vanilla turtle entity.
#[entity_behavior(class = "Turtle", spawn_rule)]
pub struct TurtleEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    state: SyncMutex<TurtleState>,
    entity_data: SyncMutex<TurtleEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `TurtleEntity`.
unsafe impl DowncastType for TurtleEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/turtle");
}

/// Vanilla `TurtleEggBlock.isSand`.
fn is_sand(level: &(impl LevelReader + ?Sized), pos: BlockPos) -> bool {
    level
        .get_block_state(pos)
        .get_block()
        .has_tag(&BlockTag::SAND)
}

/// Vanilla `TurtleEggBlock.onSand`.
fn on_sand(level: &(impl LevelReader + ?Sized), pos: BlockPos) -> bool {
    is_sand(level, pos.below())
}

impl TurtleEntity {
    /// Vanilla `Turtle.checkTurtleSpawnRules`: on a sand beach no higher than
    /// three blocks above sea level, in daylight.
    pub(crate) const SPAWN_RULE: SpawnRule = |_entity_type, level, _spawn_reason, pos, _random| {
        pos.y() < level.sea_level() + 4
            && on_sand(level, pos)
            && <Self as Animal>::is_bright_enough_to_spawn(level, pos)
    };

    /// Creates a new turtle.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved turtle.
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
        let ageable_base = AgeableMobBase::new();
        let animal_base = AnimalBase::new();
        AnimalBase::initialize_pathfinding_malus(&mob_base);
        {
            let mut malus = mob_base.pathfinding_malus().lock();
            malus.set(PathType::Water, 0.0);
            malus.set(PathType::DoorIronClosed, -1.0);
            malus.set(PathType::DoorWoodClosed, -1.0);
            malus.set(PathType::DoorOpen, -1.0);
        }
        // Vanilla `TurtlePathNavigation extends AmphibiousPathNavigation`; its
        // `isStableDestination` is `TurtleEntity::is_stable_destination`.
        *mob_base.navigation().lock() = PathNavigation::amphibious();
        let mut entity_data = TurtleEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        {
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, PanicGoal::new(1.2).with_water_seeking(7));
            goal_selector.add_goal(1, TurtleBreedGoal::new(1.0));
            goal_selector.add_goal(1, TurtleLayEggGoal::new(1.0));
            goal_selector.add_goal(
                2,
                TemptGoal::new(
                    1.1,
                    |item_stack| {
                        REGISTRY
                            .items
                            .is_in_tag(item_stack.item(), &ItemTag::TURTLE_FOOD)
                    },
                    false,
                ),
            );
            goal_selector.add_goal(3, TurtleGoToWaterGoal::new(1.0));
            goal_selector.add_goal(4, TurtleGoHomeGoal::new(1.0));
            goal_selector.add_goal(7, TurtleTravelGoal::new(1.0));
            goal_selector.add_goal(8, LookAtPlayerGoal::new(8.0));
            goal_selector.add_goal(9, TurtleRandomStrollGoal::new(1.0, 100));
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
            state: SyncMutex::new(TurtleState {
                home_pos: BlockPos::ZERO,
                travel_pos: None,
                going_home: false,
                lay_egg_counter: 0,
            }),
            entity_data: SyncMutex::new(entity_data),
        }
    }

    /// Vanilla `getHomePos`.
    #[must_use]
    pub fn home_pos(&self) -> BlockPos {
        self.state.lock().home_pos
    }

    /// Vanilla `setHomePos`.
    pub fn set_home_pos(&self, pos: BlockPos) {
        self.state.lock().home_pos = pos;
    }

    /// Vanilla `hasEgg`: the turtle has mated and carries an egg home.
    #[must_use]
    pub fn has_egg(&self) -> bool {
        *self.entity_data.lock().has_egg.get()
    }

    /// Vanilla `setHasEgg`.
    pub fn set_has_egg(&self, has_egg: bool) {
        self.entity_data.lock().has_egg.set(has_egg);
    }

    /// Vanilla `isLayingEgg`.
    #[must_use]
    pub fn is_laying_egg(&self) -> bool {
        *self.entity_data.lock().laying_egg.get()
    }

    /// Vanilla `setLayingEgg`: also restarts the digging counter.
    fn set_laying_egg(&self, laying: bool) {
        self.state.lock().lay_egg_counter = i32::from(laying);
        self.entity_data.lock().laying_egg.set(laying);
    }

    fn lay_egg_counter(&self) -> i32 {
        self.state.lock().lay_egg_counter
    }

    fn increment_lay_egg_counter(&self) {
        self.state.lock().lay_egg_counter += 1;
    }

    fn going_home(&self) -> bool {
        self.state.lock().going_home
    }

    fn set_going_home(&self, going_home: bool) {
        self.state.lock().going_home = going_home;
    }

    fn travel_pos(&self) -> Option<BlockPos> {
        self.state.lock().travel_pos
    }

    fn set_travel_pos(&self, travel_pos: Option<BlockPos>) {
        self.state.lock().travel_pos = travel_pos;
    }

    fn is_food(item_stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::TURTLE_FOOD)
    }

    /// The digging half of vanilla `aiStep`: sand particles every five ticks
    /// while the egg goes in.
    fn tick_digging(&self) {
        let counter = self.lay_egg_counter();
        if !Entity::is_alive(self) || !self.is_laying_egg() || counter < 1 || counter % 5 != 0 {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };
        let pos = self.block_position();
        if on_sand(world.as_ref(), pos) {
            let below = world.get_block_state(pos.below());
            world.level_event(
                level_events::PARTICLES_DESTROY_BLOCK,
                pos,
                level_events::encode_block_state_data(u32::from(below.0)),
                None,
            );
            self.game_event(&vanilla_game_events::ENTITY_ACTION);
        }
    }

    /// Vanilla `LivingEntity.dropFromGiftLootTable` for `gameplay/turtle_grow`.
    fn drop_grow_loot(&self) {
        let position = self.position();
        let mut rng = rand::rng();
        let equipment = living_loot_equipment(self);
        let mut context = LootContext::new(&mut rng)
            .with_origin(position.x, position.y, position.z)
            .with_this_entity(entity_loot_ref(self, Some(&equipment)));
        for item in vanilla_loot_tables::GAMEPLAY_TURTLE_GROW.get_random_items(&mut context) {
            let _ = self.spawn_at_location(item, 0.0);
        }
    }

    /// Vanilla `TurtleMoveControl.updateSpeed`.
    fn update_move_speed(&self) {
        if self.is_in_water() {
            self.set_velocity(self.velocity() + DVec3::new(0.0, 0.005, 0.0));
            if !self.home_pos().closer_to_center_than(self.position(), 16.0) {
                self.set_mob_speed((self.get_speed() / 2.0).max(0.08));
            }
            if AgeableMob::is_baby(self) {
                self.set_mob_speed((self.get_speed() / 3.0).max(0.06));
            }
        } else if self.on_ground() {
            self.set_mob_speed((self.get_speed() / 2.0).max(0.06));
        }
    }

    /// Vanilla `TurtleMoveControl.tick`. Unlike the base move control it
    /// keeps its `MOVE_TO` operation until navigation is done.
    fn tick_turtle_move_control(&self) {
        self.update_move_speed();
        let move_control = self.mob_base().controls().lock().move_control;
        if move_control.operation() != MoveControlOperation::MoveTo
            || self.mob_base().navigation().lock().is_done()
        {
            self.set_mob_speed(0.0);
            return;
        }
        let position = self.position();
        let wanted = move_control.wanted_position();
        let delta = wanted - position;
        let distance = delta.length();
        if distance < f64::from(1.0E-5_f32) {
            self.set_mob_speed(0.0);
            return;
        }
        let yd = delta.y / distance;
        #[expect(clippy::cast_possible_truncation, reason = "vanilla (float) atan2")]
        let y_rot_d = (delta.z.atan2(delta.x) * 180.0 / PI) as f32 - 90.0;
        let (y_rot, x_rot) = self.rotation();
        let y_rot = rotlerp(y_rot, y_rot_d, 90.0);
        self.set_rotation((y_rot, x_rot));
        self.set_y_body_rot(y_rot);
        #[expect(clippy::cast_possible_truncation, reason = "vanilla (float) speed")]
        let target_speed = (move_control.speed_modifier()
            * self
                .attributes()
                .lock()
                .required_value(vanilla_attributes::MOVEMENT_SPEED))
            as f32;
        let speed = self.get_speed() + 0.125 * (target_speed - self.get_speed());
        self.set_mob_speed(speed);
        self.set_velocity(self.velocity() + DVec3::new(0.0, f64::from(speed) * yd * 0.1, 0.0));
    }

    /// Vanilla `Turtle.TurtleBreedGoal.breed`: the mother carries an egg home
    /// instead of a baby appearing.
    fn mate(&self, world: &Arc<World>, partner: &dyn Animal) {
        self.award_bred_animals(world, partner, None);
        self.set_has_egg(true);
        self.set_age(PARENT_AGE_AFTER_MATING);
        partner.set_age(PARENT_AGE_AFTER_MATING);
        self.reset_love();
        partner.reset_love();
        if world.get_game_rule(&MOB_DROPS) {
            let orb: SharedEntity = Arc::new(ExperienceOrbEntity::with_value(
                &vanilla_entities::EXPERIENCE_ORB,
                next_entity_id(),
                self.position(),
                rand::random_range(1..=7),
                Arc::downgrade(world),
            ));
            if let Err(error) = world.try_add_entity(orb) {
                log::debug!("failed to add turtle breeding experience orb: {error}");
            }
        }
    }
}

impl DefaultEntityPush for TurtleEntity {}

impl Entity for TurtleEntity {
    fn as_default_entity_push(&self) -> Option<&dyn DefaultEntityPush> {
        Some(self)
    }

    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    /// Vanilla `getDefaultDimensions`: hatchlings are 0.3 scale with the
    /// rider anchored on the shell.
    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if AgeableMob::is_baby(self) {
            let dimensions = self.entity_type.dimensions;
            EntityDimensions::new_with_attachments(
                dimensions.width,
                dimensions.height,
                dimensions.eye_height,
                EntityAttachments::new(&BABY_PASSENGER_ATTACHMENTS, &[], &[], &[]),
            )
            .scale(BABY_SCALE)
            .scale(scale)
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

    fn is_pushed_by_fluid(&self) -> bool {
        false
    }

    fn swim_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_TURTLE_SWIM
    }

    fn play_swim_sound(&self, volume: f32) {
        let pitch = 1.0 + (rand::random::<f32>() - rand::random::<f32>()) * 0.4;
        self.play_sound(self.swim_sound(), volume * 1.5, pitch);
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        let sound = if AgeableMob::is_baby(self) {
            &sound_events::ENTITY_TURTLE_SHAMBLE_BABY
        } else {
            &sound_events::ENTITY_TURTLE_SHAMBLE
        };
        self.play_sound(sound, 0.15, 1.0);
    }

    /// Vanilla `nextStep`: a shamble sound every 0.15 blocks.
    fn next_step(&self) -> f32 {
        self.base().movement_progress().move_dist() + 0.15
    }

    /// Vanilla `thunderHit`: lightning always kills a turtle.
    fn thunder_hit(&self, world: &World, _lightning: &LightningBoltEntity) {
        self.hurt_server(
            world,
            &DamageSource::environment(&vanilla_damage_types::LIGHTNING_BOLT),
            f32::MAX,
        );
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        let home = self.home_pos();
        nbt.insert(
            TAG_HOME_POS,
            NbtTag::IntArray(vec![home.x(), home.y(), home.z()]),
        );
        nbt.insert(TAG_HAS_EGG, i8::from(self.has_egg()));
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        let home = nbt
            .int_array(TAG_HOME_POS)
            .filter(|values| values.len() == 3)
            .map_or_else(
                || self.block_position(),
                |values| BlockPos::new(values[0], values[1], values[2]),
            );
        self.set_home_pos(home);
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        self.set_has_egg(nbt.byte(TAG_HAS_EGG).is_some_and(|value| value != 0));
    }
}

impl LivingEntity for TurtleEntity {
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
        Some(if AgeableMob::is_baby(self) {
            &sound_events::ENTITY_TURTLE_HURT_BABY
        } else {
            &sound_events::ENTITY_TURTLE_HURT
        })
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(if AgeableMob::is_baby(self) {
            &sound_events::ENTITY_TURTLE_DEATH_BABY
        } else {
            &sound_events::ENTITY_TURTLE_DEATH
        })
    }

    fn get_age_scale(&self) -> f32 {
        if AgeableMob::is_baby(self) {
            BABY_SCALE
        } else {
            1.0
        }
    }

    fn base_experience_reward(&self) -> i32 {
        Animal::base_experience_reward_animal(self)
    }

    /// Vanilla `travelInWater`: slow paddling, and sinking unless chasing
    /// something or nearly home.
    fn travel_in_water(
        &self,
        entity: &SharedEntity,
        input: DVec3,
        _base_gravity: f64,
        _is_falling: bool,
        _old_y: f64,
    ) -> Option<MoveResult> {
        self.move_relative(0.1, input);
        let result = Arc::clone(entity).move_entity(MoverType::SelfMovement, self.velocity())?;
        self.set_velocity(self.velocity() * 0.9);
        let near_home = self.home_pos().closer_to_center_than(self.position(), 20.0);
        if self.target().is_none() && (!self.going_home() || !near_home) {
            self.set_velocity(self.velocity() + DVec3::new(0.0, -0.005, 0.0));
        }
        Some(result)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        let result = Mob::mob_ai_step(self, entity);
        AgeableMob::tick_ageable_mob(self);
        Animal::tick_animal_love(self);
        self.tick_digging();
        result
    }
}

impl AgeableMob for TurtleEntity {
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

    /// Vanilla `ageBoundaryReached`: a grown-up turtle sheds a scute.
    fn age_boundary_changed(&self, baby: bool) {
        self.refresh_dimensions();
        if !baby
            && let Some(world) = self.level()
            && world.get_game_rule(&MOB_DROPS)
        {
            self.drop_grow_loot();
        }
    }
}

impl Animal for TurtleEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        Self::is_food(item_stack)
    }

    fn can_fall_in_love(&self) -> bool {
        self.in_love_time() <= 0 && !self.has_egg()
    }

    fn spawn_child_from_breeding(&self, world: &Arc<World>, partner: &dyn Animal) {
        self.mate(world, partner);
    }
}

impl Mob for TurtleEntity {
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
        self.tick_turtle_move_control();
    }

    fn custom_server_ai_step(&self) {
        Animal::custom_server_ai_step_animal(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        (!self.is_in_water() && self.on_ground() && !AgeableMob::is_baby(self))
            .then_some(&sound_events::ENTITY_TURTLE_AMBIENT_LAND)
    }

    fn ambient_sound_interval(&self) -> i32 {
        AMBIENT_SOUND_INTERVAL
    }

    fn mob_can_be_leashed(&self) -> bool {
        false
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.set_home_pos(self.block_position());
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        Animal::mob_interact_animal(self, player, hand)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for TurtleEntity {
    /// Vanilla `TurtlePathNavigation.isStableDestination`: water while
    /// traveling, otherwise anything with ground under it.
    fn is_stable_destination(&self, pos: BlockPos) -> bool {
        let Some(world) = self.level() else {
            return false;
        };
        if self.travel_pos().is_some() {
            world.get_block_state(pos).get_block() == &vanilla_blocks::WATER
        } else {
            !world.get_block_state(pos.below()).is_air()
        }
    }

    /// Vanilla `getWalkTargetValue`: water (unless heading home) and sand
    /// beat everything else.
    fn get_walk_target_value_in(&self, pos: BlockPos, level: &dyn LevelReader) -> f32 {
        let water = !self.going_home() && level.get_block_state(pos).get_fluid_state().is_water();
        if water || on_sand(level, pos) {
            10.0
        } else {
            level.pathfinding_cost_from_light_levels(pos)
        }
    }
}

/// Vanilla `TurtleEggBlock` hatching: one hatchling per egg, homed on the
/// nest.
pub(crate) fn spawn_hatchling(world: &Arc<World>, nest: BlockPos, index: i32) {
    let turtle = TurtleEntity::new(
        &vanilla_entities::TURTLE,
        next_entity_id(),
        DVec3::new(
            f64::from(nest.x()) + 0.3 + f64::from(index) * 0.2,
            f64::from(nest.y()),
            f64::from(nest.z()) + 0.3,
        ),
        Arc::downgrade(world),
    );
    turtle.set_age(HATCHLING_AGE);
    turtle.set_home_pos(nest);
    turtle.set_rotation((0.0, 0.0));
    if let Err(error) = world.try_add_entity(Arc::new(turtle)) {
        log::debug!("world rejected a hatched turtle: {error}");
    }
}
