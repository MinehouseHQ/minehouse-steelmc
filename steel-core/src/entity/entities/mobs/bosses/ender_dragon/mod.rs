//! Vanilla `EnderDragon`: the End's boss, flying between the obsidian towers
//! on a fixed graph of 24 points, landing on the exit portal to breathe fire,
//! and healing from the End Crystals.
//!
//! The fight around it (boss bar, crystal count, exit portal, egg, respawn)
//! is [`EnderDragonFight`]; a dragon joins it when the fight spawns it or
//! when its UUID is the fight's dragon. Any other dragon, such as one
//! summoned by a command, flies outside a fight.

mod nodes;
mod part;
mod phases;

#[cfg(test)]
mod tests;

use std::f32::consts::TAU;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::{array, ptr};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_math::angle::{DEG_TO_RAD, RAD_TO_DEG, wrap_degrees};
use steel_math::trig;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_damage_type_tags::DamageTypeTag;
use steel_registry::vanilla_entity_data::EnderDragonEntityData;
use steel_registry::vanilla_game_rules::{MOB_DROPS, MOB_GRIEFING};
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _, level_events, sound_events, vanilla_damage_types,
    vanilla_game_events,
};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, DowncastType, DowncastTypeKey, WorldAabb};

use self::nodes::DragonNodes;
pub use self::part::EnderDragonPartEntity;
pub use self::phases::DragonPhase;
use self::phases::DragonPhases;
use crate::chunk::heightmap::HeightmapType;
use crate::enchantment_helper::{self, EnchantmentPostAttackContext};
use crate::entity::ai::node::Node;
use crate::entity::ai::path::Path;
use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::damage::DamageSource;
use crate::entity::entities::{EndCrystalEntity, ExperienceOrbEntity};
use crate::entity::entity_selector::no_creative_or_spectator;
use crate::entity::{
    Enemy, Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, MobEffectInstance, RemovalReason, SharedEntity,
    reserve_entity_ids, sync_dirty_mob_effects,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::{MoveResult, MoverType};
use crate::world::World;
use crate::world::dragon_fight::EnderDragonFight;
use crate::world::explosion::explosion_damage_source;

/// Index of the head in [`EnderDragonEntity::parts`].
const HEAD: usize = 0;
const NECK: usize = 1;
const BODY: usize = 2;
const TAIL_1: usize = 3;
const WING_1: usize = 6;
const WING_2: usize = 7;
/// Vanilla's parts in `subEntities` order: name, width, height.
const PART_SHAPES: [(&str, f32, f32); 8] = [
    ("head", 1.0, 1.0),
    ("neck", 3.0, 3.0),
    ("body", 5.0, 3.0),
    ("tail", 2.0, 2.0),
    ("tail", 2.0, 2.0),
    ("tail", 2.0, 2.0),
    ("wing", 4.0, 2.0),
    ("wing", 4.0, 2.0),
];
/// The dragon and its parts take consecutive ids.
const RESERVED_IDS: i32 = 1 + PART_SHAPES.len() as i32;

const SITTING_ALLOWED_DAMAGE_PERCENTAGE: f32 = 0.25;
const CRYSTAL_SEARCH_RADIUS: f64 = 32.0;
/// Vanilla `CRYSTAL_DESTROY_TARGETING` range.
const CRYSTAL_DESTROY_RANGE: f64 = 64.0;
const DEATH_DURATION: i32 = 200;
/// XP of a dragon that is not the first one killed in its fight.
const XP_REPEAT_KILL: i32 = 500;
/// XP of the first dragon killed in a fight.
const XP_FIRST_KILL: i32 = 12000;

const TAG_DRAGON_PHASE: &str = "DragonPhase";
const TAG_DRAGON_DEATH_TIME: &str = "DragonDeathTime";
const TAG_SITTING_DAMAGE_RECEIVED: &str = "sitting_damage_received";

/// Vanilla `DragonFlightHistory`: the dragon's height and yaw over the last
/// 64 ticks, which the neck and tail trail behind.
struct FlightHistory {
    samples: [(f64, f32); 64],
    head: Option<usize>,
}

impl FlightHistory {
    const fn new() -> Self {
        Self {
            samples: [(0.0, 0.0); 64],
            head: None,
        }
    }

    const fn record(&mut self, y: f64, y_rot: f32) {
        let head = match self.head {
            None => {
                self.samples = [(y, y_rot); 64];
                0
            }
            Some(head) => (head + 1) & 63,
        };
        self.samples[head] = (y, y_rot);
        self.head = Some(head);
    }

    /// Vanilla `get(delay)`: a fresh history reads its first slot as head -1.
    fn get(&self, delay: usize) -> (f64, f32) {
        let head = self.head.unwrap_or(63);
        self.samples[(head + 64 - (delay & 63)) & 63]
    }
}

/// The fields vanilla keeps on the dragon outside its synced data.
struct DragonState {
    phases: DragonPhases,
    flight_history: FlightHistory,
    flap_time: f32,
    o_flap_time: f32,
    in_wall: bool,
    death_time: i32,
    /// Vanilla `yRotA`: the yaw the dragon is turning by.
    y_rot_a: f32,
    nearest_crystal: Option<Weak<dyn Entity>>,
    /// Vanilla `fightOrigin`.
    fight_origin: BlockPos,
    sitting_damage_received: f32,
    nodes: Option<DragonNodes>,
}

/// An Ender Dragon.
#[entity_behavior(class = "EnderDragon")]
pub struct EnderDragonEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<EnderDragonEntityData>,
    state: SyncMutex<DragonState>,
    /// Vanilla `subEntities`: head, neck, body, three tail parts, two wings.
    parts: [Arc<EnderDragonPartEntity>; 8],
    /// Whether the world's dragon index knows this dragon.
    registered: AtomicBool,
    /// Vanilla `dragonFight != null`: whether this dragon is its world's
    /// fight dragon.
    in_fight: AtomicBool,
}

// SAFETY: The owner-scoped type key uniquely identifies EnderDragonEntity.
unsafe impl DowncastType for EnderDragonEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/ender_dragon");
}

/// The center of `pos`.
fn center_of(pos: BlockPos) -> DVec3 {
    let (x, y, z) = pos.get_center();
    DVec3::new(x, y, z)
}

/// Vanilla `Vec3.atBottomCenterOf`.
fn bottom_center_of(pos: BlockPos) -> DVec3 {
    let (x, y, z) = pos.get_bottom_center();
    DVec3::new(x, y, z)
}

/// The corner of `pos`, as vanilla passes block coordinates as doubles.
fn corner_of(pos: BlockPos) -> DVec3 {
    DVec3::new(f64::from(pos.x()), f64::from(pos.y()), f64::from(pos.z()))
}

impl EnderDragonEntity {
    /// Creates a new Ender Dragon. It takes its id and its parts' ids from a
    /// fresh block, as the client numbers the parts after the dragon.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, _id: i32, position: DVec3, world: Weak<World>) -> Self {
        let id = reserve_entity_ids(RESERVED_IDS);
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, Weak::clone(&world)),
            entity_type,
            world,
        )
    }

    /// Loads a saved Ender Dragon.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, mut load: EntityBaseLoad) -> Self {
        load.id = reserve_entity_ids(RESERVED_IDS);
        let world = Weak::clone(&load.world);
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            world,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef, world: Weak<World>) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mob_base = MobBase::new();
        let mut entity_data = EnderDragonEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.phase.set(DragonPhase::Hovering.id());

        let id = base.id();
        let position = base.position();
        let parts = array::from_fn(|index| {
            let (name, width, height) = PART_SHAPES[index];
            Arc::new(EnderDragonPartEntity::new(
                entity_type,
                id + 1 + index as i32,
                id,
                name,
                width,
                height,
                position,
                Weak::clone(&world),
            ))
        });

        let dragon = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            state: SyncMutex::new(DragonState {
                phases: DragonPhases::default(),
                flight_history: FlightHistory::new(),
                flap_time: 0.0,
                o_flap_time: 0.0,
                in_wall: false,
                death_time: 0,
                y_rot_a: 0.0,
                nearest_crystal: None,
                fight_origin: BlockPos::new(0, 0, 0),
                sitting_damage_received: 0.0,
                nodes: None,
            }),
            parts,
            registered: AtomicBool::new(false),
            in_fight: AtomicBool::new(false),
        };
        dragon.set_health(dragon.get_max_health());
        dragon.set_no_physics(true);
        dragon.set_phase(DragonPhase::Hovering);
        dragon
    }

    /// Vanilla `subEntities`.
    #[must_use]
    pub const fn parts(&self) -> &[Arc<EnderDragonPartEntity>; 8] {
        &self.parts
    }

    /// The parts as world entities.
    pub(crate) fn shared_parts(&self) -> Vec<SharedEntity> {
        self.parts
            .iter()
            .map(|part| Arc::clone(part) as SharedEntity)
            .collect()
    }

    fn part(&self, index: usize) -> &EnderDragonPartEntity {
        &self.parts[index]
    }

    /// Vanilla `fightOrigin`.
    #[must_use]
    pub fn fight_origin(&self) -> BlockPos {
        self.state.lock().fight_origin
    }

    /// Vanilla `setFightOrigin`.
    pub fn set_fight_origin(&self, origin: BlockPos) {
        self.state.lock().fight_origin = origin;
    }

    /// The ticks since the dragon started dying.
    #[must_use]
    pub fn death_time(&self) -> i32 {
        self.state.lock().death_time
    }

    /// Vanilla `setDragonFight`: makes this dragon its world's fight dragon.
    pub fn join_fight(&self) {
        self.in_fight.store(true, Ordering::Relaxed);
    }

    /// Vanilla `getDragonFight`: the fight this dragon belongs to.
    fn with_fight<R>(&self, world: &World, f: impl FnOnce(&EnderDragonFight) -> R) -> Option<R> {
        if !self.in_fight.load(Ordering::Relaxed) {
            return None;
        }
        world.dragon_fight().map(f)
    }

    /// The crystals left in the dragon's fight; `None` outside a fight.
    fn alive_crystals(&self) -> Option<i32> {
        let world = self.level()?;
        self.with_fight(&world, EnderDragonFight::alive_crystals)
    }

    /// Vanilla `EndPodiumFeature.getLocation(fightOrigin)` on the surface.
    fn podium_surface(&self, world: &World) -> BlockPos {
        world.heightmap_pos(HeightmapType::MotionBlockingNoLeaves, self.fight_origin())
    }

    /// [`Self::podium_surface`] on the `MOTION_BLOCKING` heightmap, which the
    /// death flight aims for.
    fn podium_surface_motion_blocking(&self, world: &World) -> BlockPos {
        world.heightmap_pos(HeightmapType::MotionBlocking, self.fight_origin())
    }

    /// Builds the node graph on first use, like vanilla `findClosestNode`.
    fn ensure_nodes(&self) {
        if self.state.lock().nodes.is_some() {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };
        let nodes = DragonNodes::build(|x, z| {
            world
                .heightmap_pos(
                    HeightmapType::MotionBlockingNoLeaves,
                    BlockPos::new(x, 0, z),
                )
                .y()
        });
        self.state.lock().nodes.get_or_insert(nodes);
    }

    /// Vanilla `findClosestNode()`.
    fn find_closest_node(&self) -> usize {
        self.find_closest_node_at(self.position())
    }

    /// Vanilla `findClosestNode(x, y, z)`.
    fn find_closest_node_at(&self, pos: DVec3) -> usize {
        self.ensure_nodes();
        let with_crystals = self.alive_crystals().is_some_and(|alive| alive > 0);
        self.state.lock().nodes.as_ref().map_or(0, |nodes| {
            nodes.closest(BlockPos::containing(pos.x, pos.y, pos.z), with_crystals)
        })
    }

    /// Vanilla `findPath`.
    fn find_path(&self, start: usize, end: usize, final_node: Option<Node>) -> Option<Path> {
        self.ensure_nodes();
        let with_crystals = self.alive_crystals().is_some_and(|alive| alive > 0);
        self.state
            .lock()
            .nodes
            .as_mut()?
            .find_path(start, end, final_node, with_crystals)
    }

    /// Vanilla `getViewVector`.
    fn view_vector(&self) -> DVec3 {
        let (y_rot, x_rot) = self.rotation();
        self.calculate_view_vector(x_rot, y_rot)
    }

    /// Vanilla `getHeadLookVector`.
    fn head_look_vector(&self) -> DVec3 {
        let phase = self.phase();
        let (y_rot, x_rot) = self.rotation();
        if matches!(phase, DragonPhase::Landing | DragonPhase::Takeoff) {
            let world = self.level();
            let dist = world.map_or(1.0, |world| {
                let egg = self.podium_surface(&world);
                ((center_of(egg).distance_squared(self.position())).sqrt() as f32 / 4.0).max(1.0)
            });
            let y_offset = 6.0 / dist;
            self.calculate_view_vector(-y_offset * 1.5 * 5.0, y_rot)
        } else if phase.is_sitting() {
            self.calculate_view_vector(-45.0, y_rot)
        } else {
            self.calculate_view_vector(x_rot, y_rot)
        }
    }

    /// Vanilla `hurt(level, part, source, damage)`: damage to the head and
    /// neck counts in full, the rest a quarter, and only players and
    /// dragon-hurting sources hurt at all.
    pub fn hurt_part(
        &self,
        world: &World,
        part: &EnderDragonPartEntity,
        source: &DamageSource,
        damage: f32,
    ) -> bool {
        if self.phase() == DragonPhase::Dying {
            return false;
        }
        let mut damage = self.phase_on_hurt(source, damage);
        if !ptr::eq(part, self.part(HEAD)) && !ptr::eq(part, self.part(NECK)) {
            damage = damage / 4.0 + damage.min(1.0);
        }
        if damage < 0.01 {
            return false;
        }
        let by_player = source
            .causing_entity()
            .is_some_and(|entity| entity.as_player().is_some());
        if by_player || source.is(&DamageTypeTag::ALWAYS_HURTS_ENDER_DRAGONS) {
            let health_before = self.get_health();
            self.default_hurt_server(world, source, damage);
            if self.phase().is_sitting() {
                let take_off = {
                    let mut state = self.state.lock();
                    state.sitting_damage_received += health_before - self.get_health();
                    let take_off = state.sitting_damage_received
                        > SITTING_ALLOWED_DAMAGE_PERCENTAGE * self.get_max_health();
                    if take_off {
                        state.sitting_damage_received = 0.0;
                    }
                    take_off
                };
                if take_off {
                    self.set_phase(DragonPhase::Takeoff);
                }
            }
        }
        true
    }

    /// Vanilla `onCrystalDestroyed`: losing its healing crystal hurts the
    /// dragon, and a circling dragon turns on the player who did it.
    pub fn on_crystal_destroyed(
        &self,
        world: &World,
        crystal: &SharedEntity,
        pos: BlockPos,
        source: &DamageSource,
    ) {
        let player: Option<SharedEntity> = match source.causing_entity() {
            Some(cause) if cause.as_player().is_some() => Some(Arc::clone(cause)),
            _ => {
                let conditions = TargetingConditions::for_combat().range(CRYSTAL_DESTROY_RANGE);
                world
                    .nearest_player(corner_of(pos), -1.0, |player| {
                        conditions.test(world, None, player)
                    })
                    .map(|player| player as SharedEntity)
            }
        };
        let is_nearest = self
            .state
            .lock()
            .nearest_crystal
            .as_ref()
            .is_some_and(|nearest| ptr::addr_eq(nearest.as_ptr(), Arc::as_ptr(crystal)));
        if is_nearest {
            let damage = explosion_damage_source(Some(Arc::clone(crystal)), player.clone());
            self.hurt_part(world, self.part(HEAD), &damage, 10.0);
        }
        self.phase_on_crystal_destroyed(player.as_ref());
    }

    /// Vanilla `checkCrystals`: heal from the nearest crystal and look for a
    /// closer one now and then.
    fn check_crystals(&self, world: &World) {
        let healing = {
            let mut state = self.state.lock();
            match state.nearest_crystal.as_ref().map(Weak::upgrade) {
                Some(Some(crystal)) if !crystal.is_removed() => true,
                Some(_) => {
                    state.nearest_crystal = None;
                    false
                }
                None => false,
            }
        };
        if healing && self.tick_count() % 10 == 0 && self.get_health() < self.get_max_health() {
            self.set_health(self.get_health() + 1.0);
        }

        if rand::random_range(0..10) == 0 {
            let position = self.position();
            let nearest = world
                .get_entities_in_aabb_matching(
                    &self.bounding_box().inflate(CRYSTAL_SEARCH_RADIUS),
                    |entity| {
                        steel_utils::Downcast::downcast_ref::<EndCrystalEntity>(entity).is_some()
                    },
                )
                .into_iter()
                .map(|crystal| (crystal.position().distance_squared(position), crystal))
                .fold(
                    None::<(f64, SharedEntity)>,
                    |nearest, (dist, crystal)| match nearest {
                        Some((best, _)) if best <= dist => nearest,
                        _ => Some((dist, crystal)),
                    },
                );
            self.state.lock().nearest_crystal =
                nearest.map(|(_, crystal)| Arc::downgrade(&crystal));
        }
    }

    /// Vanilla `knockBack`: the wings shove everything under them away and
    /// hurt it unless the dragon is perched.
    fn knock_back(&self, world: &World, this: &SharedEntity, entities: &[SharedEntity]) {
        let body = self.part(BODY).bounding_box();
        let xm = f64::midpoint(body.min_corner().x, body.max_corner().x);
        let zm = f64::midpoint(body.min_corner().z, body.max_corner().z);
        for entity in entities {
            let Some(living) = entity.as_living_entity() else {
                continue;
            };
            let xd = entity.position().x - xm;
            let zd = entity.position().z - zm;
            let dd = (xd * xd + zd * zd).max(0.1);
            entity.push_impulse(DVec3::new(xd / dd * 4.0, f64::from(0.2_f32), zd / dd * 4.0));
            if !self.phase().is_sitting()
                && living.last_hurt_by_mob_timestamp() < entity.tick_count() - 2
                && self.do_teams_allow_damage(entity.as_ref())
            {
                self.melee(world, this, entity, 5.0);
            }
        }
    }

    /// Vanilla `hurt(level, entities)`: the head and neck bite for 10.
    fn bite(&self, world: &World, this: &SharedEntity, entities: &[SharedEntity]) {
        for entity in entities {
            if entity.as_living_entity().is_some() && self.do_teams_allow_damage(entity.as_ref()) {
                self.melee(world, this, entity, 10.0);
            }
        }
    }

    /// A mob attack on `target` and its post-attack enchantment effects.
    fn melee(&self, world: &World, this: &SharedEntity, target: &SharedEntity, damage: f32) {
        let source = DamageSource::environment(&vanilla_damage_types::MOB_ATTACK)
            .with_causing_entity(Arc::clone(this))
            .with_direct_entity(Arc::clone(this));
        target.hurt(world, &source, damage);
        let weapon = self.equipment_in_slot(EquipmentSlot::MainHand);
        let context = EnchantmentPostAttackContext::new(target, &source);
        enchantment_helper::do_post_attack_effects_with_item_source(
            world,
            target.as_ref(),
            &weapon,
            &context,
        );
    }

    /// Vanilla `checkWalls`: with mob griefing on, the dragon eats through
    /// every block that is not dragon-immune; otherwise it is stuck.
    fn check_walls(world: &Arc<World>, bb: WorldAabb) -> bool {
        let min = bb.min_corner();
        let max = bb.max_corner();
        let (x0, y0, z0) = (
            min.x.floor() as i32,
            min.y.floor() as i32,
            min.z.floor() as i32,
        );
        let (x1, y1, z1) = (
            max.x.floor() as i32,
            max.y.floor() as i32,
            max.z.floor() as i32,
        );
        let griefing = world.get_game_rule(&MOB_GRIEFING);
        let mut hit_wall = false;
        let mut destroyed_block = false;
        for x in x0..=x1 {
            for y in y0..=y1 {
                for z in z0..=z1 {
                    let pos = BlockPos::new(x, y, z);
                    let state = world.get_block_state(pos);
                    let block = state.get_block();
                    if state.is_air()
                        || REGISTRY
                            .blocks
                            .is_in_tag(block, &BlockTag::DRAGON_TRANSPARENT)
                    {
                        continue;
                    }
                    if griefing && !REGISTRY.blocks.is_in_tag(block, &BlockTag::DRAGON_IMMUNE) {
                        destroyed_block = world.remove_block(pos, false) || destroyed_block;
                    } else {
                        hit_wall = true;
                    }
                }
            }
        }
        if destroyed_block {
            let pos = BlockPos::new(
                x0 + rand::random_range(0..=x1 - x0),
                y0 + rand::random_range(0..=y1 - y0),
                z0 + rand::random_range(0..=z1 - z0),
            );
            world.level_event(level_events::PARTICLES_DRAGON_BLOCK_BREAK, pos, 0, None);
        }
        hit_wall
    }

    /// The flight half of vanilla `aiStep`: run the phase and steer toward
    /// its target.
    fn fly(&self, world: &Arc<World>, this: &SharedEntity) {
        let phase = self.phase();
        self.tick_phase(world, this);
        let mut current = self.phase();
        if current != phase {
            self.tick_phase(world, this);
            current = self.phase();
        }
        let Some(target) = self.state.lock().phases.fly_target() else {
            return;
        };

        let position = self.position();
        let xdd = target.x - position.x;
        let mut ydd = target.y - position.y;
        let zdd = target.z - position.z;
        let dist_to_target = xdd * xdd + ydd * ydd + zdd * zdd;
        let max = f64::from(current.fly_speed());
        let horizontal_dist = (xdd * xdd + zdd * zdd).sqrt();
        if horizontal_dist > 0.0 {
            ydd = (ydd / horizontal_dist).clamp(-max, max);
        }
        self.set_velocity(self.velocity() + DVec3::new(0.0, ydd * 0.01, 0.0));
        let (y_rot, x_rot) = self.rotation();
        let y_rot = wrap_degrees(y_rot);
        self.set_rotation((y_rot, x_rot));
        let aim = (target - position).normalize_or_zero();
        let dir = DVec3::new(
            f64::from(trig::sin(y_rot * DEG_TO_RAD)),
            self.velocity().y,
            f64::from(-trig::cos(y_rot * DEG_TO_RAD)),
        )
        .normalize_or_zero();
        let dot = ((dir.dot(aim) as f32 + 0.5) / 1.5).max(0.0);
        if xdd.abs() > f64::from(1.0e-5_f32) || zdd.abs() > f64::from(1.0e-5_f32) {
            let y_rot_d =
                wrap_degrees(180.0 - xdd.atan2(zdd) as f32 * RAD_TO_DEG - y_rot).clamp(-50.0, 50.0);
            let turn_speed = self.turn_speed();
            let y_rot_a = {
                let mut state = self.state.lock();
                state.y_rot_a *= 0.8;
                state.y_rot_a += y_rot_d * turn_speed;
                state.y_rot_a
            };
            self.set_rotation((y_rot + y_rot_a * 0.1, x_rot));
        }

        let span = (2.0 / (dist_to_target + 1.0)) as f32;
        self.move_relative(
            0.06 * (dot * span + (1.0 - span)),
            DVec3::new(0.0, 0.0, -1.0),
        );
        let delta = if self.state.lock().in_wall {
            self.velocity() * f64::from(0.8_f32)
        } else {
            self.velocity()
        };
        let _ = Arc::clone(this).move_entity(MoverType::SelfMovement, delta);
        let actual = self.velocity().normalize_or_zero();
        let slide = 0.8 + 0.15 * (actual.dot(dir) + 1.0) / 2.0;
        self.set_velocity(self.velocity() * DVec3::new(slide, f64::from(0.91_f32), slide));
    }

    /// Vanilla `tickPart`.
    fn tick_part(&self, index: usize, x: f64, y: f64, z: f64) {
        let position = self.position() + DVec3::new(x, y, z);
        if let Err(error) = self.part(index).try_set_position(position) {
            log::debug!("Ender Dragon part rejected its position: {error}");
        }
    }

    /// Vanilla `getHeadYOffset`.
    fn head_y_offset(&self) -> f32 {
        if self.phase().is_sitting() {
            return -1.0;
        }
        let state = self.state.lock();
        (state.flight_history.get(5).0 - state.flight_history.get(0).0) as f32
    }

    /// The body half of vanilla `aiStep`: place the parts along the flight
    /// history, let the wings and head hit what they touch, and chew through
    /// walls.
    fn tick_parts(&self, world: &Arc<World>, this: &SharedEntity) {
        let old_positions = self.parts.each_ref().map(|part| part.position());
        let (y_rot, _) = self.rotation();
        let (tilt, p1, tails) = {
            let state = self.state.lock();
            let history = &state.flight_history;
            let tilt = (history.get(5).0 - history.get(10).0) as f32 * 10.0 * DEG_TO_RAD;
            let tails = [history.get(12), history.get(14), history.get(16)];
            (tilt, history.get(5), tails)
        };
        let cc_tilt = trig::cos(tilt);
        let ss_tilt = trig::sin(tilt);
        let rot1 = y_rot * DEG_TO_RAD;
        let ss1 = trig::sin(rot1);
        let cc1 = trig::cos(rot1);
        self.tick_part(BODY, f64::from(ss1 * 0.5), 0.0, f64::from(-cc1 * 0.5));
        self.tick_part(WING_1, f64::from(cc1 * 4.5), 2.0, f64::from(ss1 * 4.5));
        self.tick_part(WING_2, f64::from(cc1 * -4.5), 2.0, f64::from(ss1 * -4.5));
        if self.hurt_time() <= 0 {
            self.hit_with_parts(world, this);
        }

        let y_rot_a = self.state.lock().y_rot_a;
        let ss2 = trig::sin(y_rot * DEG_TO_RAD - y_rot_a * 0.01);
        let cc2 = trig::cos(y_rot * DEG_TO_RAD - y_rot_a * 0.01);
        let y_offset = self.head_y_offset();
        self.tick_part(
            HEAD,
            f64::from(ss2 * 6.5 * cc_tilt),
            f64::from(y_offset + ss_tilt * 6.5),
            f64::from(-cc2 * 6.5 * cc_tilt),
        );
        self.tick_part(
            NECK,
            f64::from(ss2 * 5.5 * cc_tilt),
            f64::from(y_offset + ss_tilt * 5.5),
            f64::from(-cc2 * 5.5 * cc_tilt),
        );
        for (i, p0) in tails.into_iter().enumerate() {
            let rot = y_rot * DEG_TO_RAD + wrap_degrees(p0.1 - p1.1) * DEG_TO_RAD;
            let ss = trig::sin(rot);
            let cc = trig::cos(rot);
            let dd = (i as f32 + 1.0) * 2.0;
            self.tick_part(
                TAIL_1 + i,
                f64::from(-(ss1 * 1.5 + ss * dd) * cc_tilt),
                p0.0 - p1.0 - f64::from((dd + 1.5) * ss_tilt) + 1.5,
                f64::from((cc1 * 1.5 + cc * dd) * cc_tilt),
            );
        }

        let in_wall = Self::check_walls(world, self.part(HEAD).bounding_box())
            | Self::check_walls(world, self.part(NECK).bounding_box())
            | Self::check_walls(world, self.part(BODY).bounding_box());
        self.state.lock().in_wall = in_wall;
        self.with_fight(world, |fight| fight.update_dragon(self));

        for (part, old) in self.parts.iter().zip(old_positions) {
            part.base().set_old_position(old);
        }
    }

    /// The wing knockback and head bites of vanilla `aiStep`.
    fn hit_with_parts(&self, world: &World, this: &SharedEntity) {
        let id = Some(self.id());
        let near = |index: usize, bb: WorldAabb| {
            let _ = index;
            world.get_entities_in_aabb_with_parts(id, &bb, no_creative_or_spectator)
        };
        let wing_box = |index: usize| {
            self.part(index)
                .bounding_box()
                .inflate_xyz(4.0, 2.0, 4.0)
                .translate(DVec3::new(0.0, -2.0, 0.0))
        };
        self.knock_back(world, this, &near(WING_1, wing_box(WING_1)));
        self.knock_back(world, this, &near(WING_2, wing_box(WING_2)));
        self.bite(
            world,
            this,
            &near(HEAD, self.part(HEAD).bounding_box().inflate(1.0)),
        );
        self.bite(
            world,
            this,
            &near(NECK, self.part(NECK).bounding_box().inflate(1.0)),
        );
    }

    /// The flap clock of vanilla `aiStep`.
    fn tick_flap(&self) {
        let movement = self.velocity();
        let mut flap_speed = 0.2 / (movement.x.hypot(movement.z) as f32 * 10.0 + 1.0);
        flap_speed *= 2.0_f64.powf(movement.y) as f32;
        let sitting = self.phase().is_sitting();
        let mut state = self.state.lock();
        state.flap_time += if sitting {
            0.1
        } else if state.in_wall {
            flap_speed * 0.5
        } else {
            flap_speed
        };
    }

    /// Server side of vanilla `aiStep`.
    fn dragon_ai_step(&self, world: &Arc<World>, this: &SharedEntity) {
        if !self.registered.swap(true, Ordering::Relaxed) {
            world.register_ender_dragon(this);
        }
        if !self.in_fight.load(Ordering::Relaxed)
            && world
                .dragon_fight()
                .is_some_and(|fight| fight.dragon_uuid() == Some(self.uuid()))
        {
            self.join_fight();
        }
        {
            let mut state = self.state.lock();
            state.o_flap_time = state.flap_time;
        }
        if self.is_dead_or_dying() {
            return;
        }
        self.check_crystals(world);
        self.tick_flap();
        let (y_rot, x_rot) = self.rotation();
        self.set_rotation((wrap_degrees(y_rot), x_rot));
        if Mob::is_no_ai(self) {
            self.state.lock().flap_time = 0.5;
            return;
        }
        {
            let (y_rot, _) = self.rotation();
            let y = self.position().y;
            self.state.lock().flight_history.record(y, y_rot);
        }
        self.fly(world, this);
        self.apply_effects_from_blocks();
        self.set_y_body_rot(self.rotation().0);
        self.tick_parts(world, this);
    }
}

impl Enemy for EnderDragonEntity {}

impl Entity for EnderDragonEntity {
    fn as_enemy(&self) -> Option<&dyn Enemy> {
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

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        self.entity_type.dimensions
    }

    /// Vanilla `isFlapping`: the wing passes the bottom of its beat.
    fn is_flapping(&self) -> bool {
        let (flap_time, o_flap_time) = {
            let state = self.state.lock();
            (state.flap_time, state.o_flap_time)
        };
        trig::cos(o_flap_time * TAU) <= -0.3 && trig::cos(flap_time * TAU) >= -0.3
    }

    /// Vanilla `isPickable`: players aim at the parts, not the dragon.
    fn is_pickable(&self) -> bool {
        false
    }

    /// Vanilla `checkDespawn` does nothing.
    fn check_despawn(&self) {}

    fn can_ride(&self, _vehicle: &dyn Entity) -> bool {
        false
    }

    fn can_use_portal(&self, _ignore_passenger: bool) -> bool {
        false
    }

    /// Vanilla `hurtServer`: a hit on the dragon itself lands on the body.
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        self.hurt_part(world, self.part(BODY), source, amount)
    }

    /// Vanilla `kill`: removed at once, without the death animation.
    fn kill(&self, _world: &World) {
        self.set_removed(RemovalReason::Killed);
        self.game_event(&vanilla_game_events::ENTITY_DIE);
        if let Some(world) = self.level() {
            self.with_fight(&world, |fight| {
                fight.update_dragon(self);
                fight.set_dragon_killed(&world, self);
            });
        }
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn update_data_before_sync(&self) {
        sync_dirty_mob_effects(&self.living_base, &self.entity_data);
    }

    fn sound_source(&self) -> SoundSource {
        SoundSource::Hostile
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        let state = self.state.lock();
        nbt.insert(
            TAG_DRAGON_PHASE,
            state.phases.current.unwrap_or(DragonPhase::Hovering).id(),
        );
        nbt.insert(TAG_DRAGON_DEATH_TIME, state.death_time);
        nbt.insert(TAG_SITTING_DAMAGE_RECEIVED, state.sitting_damage_received);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        if let Some(phase) = nbt.int(TAG_DRAGON_PHASE) {
            self.set_phase(DragonPhase::from_id(phase));
        }
        let mut state = self.state.lock();
        state.death_time = nbt.int(TAG_DRAGON_DEATH_TIME).unwrap_or(0);
        state.sitting_damage_received = nbt.float(TAG_SITTING_DAMAGE_RECEIVED).unwrap_or(0.0);
    }
}

impl LivingEntity for EnderDragonEntity {
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

    fn hurt_server(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        self.hurt_part(world, self.part(BODY), source, amount)
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_ENDER_DRAGON_HURT)
    }

    fn sound_volume(&self) -> f32 {
        5.0
    }

    /// Vanilla `sanitizeScale`: the dragon is never scaled.
    fn get_scale(&self) -> f32 {
        1.0
    }

    /// Vanilla `addEffect` refuses every effect.
    fn add_mob_effect_with_source(
        &self,
        _effect: MobEffectInstance,
        _source: Option<&dyn Entity>,
    ) -> bool {
        false
    }

    /// Vanilla `handleKillingBlow`: a flying dragon does not die on the spot
    /// but flies home to die over the portal.
    fn handle_killing_blow(&self) {
        if !self.phase().is_sitting() {
            self.set_health(1.0);
            self.set_phase(DragonPhase::Dying);
        }
    }

    fn knockback(&self, power: f64, xd: f64, zd: f64) {
        if !self.phase().is_sitting() {
            self.default_knockback(power, xd, zd);
        }
    }

    /// Vanilla `tickDeath`: rise for ten seconds dropping experience, then
    /// vanish.
    fn tick_death(&self) {
        let Some(world) = self.level() else {
            return;
        };
        self.with_fight(&world, |fight| fight.update_dragon(self));
        let death_time = {
            let mut state = self.state.lock();
            state.death_time += 1;
            state.death_time
        };
        let first_kill = self
            .with_fight(&world, |fight| !fight.has_previously_killed_dragon())
            .unwrap_or(false);
        let xp_count = if first_kill {
            XP_FIRST_KILL
        } else {
            XP_REPEAT_KILL
        };
        let drops = world.get_game_rule(&MOB_DROPS);
        if death_time > 150 && death_time % 5 == 0 && drops {
            ExperienceOrbEntity::award(&world, self.position(), (xp_count as f32 * 0.08) as i32);
        }
        if death_time == 1 && !self.is_silent() {
            world.global_level_event(level_events::SOUND_DRAGON_DEATH, self.block_position(), 0);
        }

        let death_move = DVec3::new(0.0, f64::from(0.1_f32), 0.0);
        if let Some(this) = world.get_entity_by_id(self.id()) {
            let _ = this.move_entity(MoverType::SelfMovement, death_move);
        }
        for part in &self.parts {
            part.set_old_position_to_current();
            if let Err(error) = part.try_set_position(part.position() + death_move) {
                log::debug!("Ender Dragon part rejected its position: {error}");
            }
        }

        if death_time >= DEATH_DURATION {
            if drops {
                ExperienceOrbEntity::award(&world, self.position(), (xp_count as f32 * 0.2) as i32);
            }
            self.with_fight(&world, |fight| fight.set_dragon_killed(&world, self));
            self.set_removed(RemovalReason::Killed);
            self.game_event(&vanilla_game_events::ENTITY_DIE);
        }
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        let world = self.level()?;
        self.dragon_ai_step(&world, entity);
        None
    }
}

impl Mob for EnderDragonEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    /// Vanilla `canAttack`: anything that can be seen as an enemy.
    fn can_attack(&self, target: &dyn LivingEntity) -> bool {
        target.can_be_seen_as_enemy()
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_ENDER_DRAGON_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}
