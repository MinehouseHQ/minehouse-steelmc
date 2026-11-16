//! Vanilla `EnderDragonFight`: the End's boss fight, kept as the End's saved
//! data. It spawns the dragon, counts the crystals on the obsidian towers,
//! shows the boss bar to nearby players, opens the exit portal and a new end
//! gateway when the dragon dies, and runs the respawn ritual.

mod portal;
mod respawn;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use glam::DVec3;
use serde::{Deserialize, Serialize};
use steel_protocol::packets::game::{BossBarColor, BossBarFlags, BossBarOverlay};
use steel_registry::feature::data::EndSpike;
use steel_registry::{vanilla_blocks, vanilla_entities};
use steel_utils::locks::SyncMutex;
use steel_utils::random::Random as _;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::saved_data::{SavedDataManager, names as saved_data_names};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos, Downcast as _, Identifier, WorldAabb, translations};
use text_components::TextComponent;
use uuid::Uuid;

use self::portal::exit_portal_pattern;
pub use self::respawn::DragonRespawnStage;
use crate::behavior::block_pattern::BlockPattern;
use crate::boss_event::ServerBossEvent;
use crate::chunk::heightmap::HeightmapType;
use crate::entity::damage::DamageSource;
use crate::entity::entities::{DragonPhase, EndCrystalEntity, EnderDragonEntity};
use crate::entity::{Entity, LivingEntity, RemovalReason, SharedEntity};
use crate::world::World;
use crate::worldgen::feature::FeatureDecorationRunner;

const MAX_TICKS_BEFORE_DRAGON_RESPAWN: i32 = 1200;
const TIME_BETWEEN_CRYSTAL_SCANS: i32 = 100;
const TIME_BETWEEN_PLAYER_SCANS: i32 = 20;
const ARENA_SIZE_CHUNKS: i32 = 8;
const GATEWAY_COUNT: i32 = 20;
/// The height above the fight origin the dragon spawns at.
pub const DRAGON_SPAWN_Y: i32 = 128;
/// Players within this distance of the spawn point see the boss bar.
const VALID_PLAYER_DISTANCE: f64 = 192.0;
/// Vanilla `DimensionType.MIN_Y` and `MAX_Y`, the height of a spike's
/// crystal search box.
const SPIKE_BOX_MIN_Y: f64 = -2032.0;
const SPIKE_BOX_MAX_Y: f64 = 2031.0;

/// Vanilla `EnderDragonFight.CODEC`, persisted as `data/ender_dragon_fight.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SavedDragonFight {
    #[serde(default = "default_true")]
    needs_state_scanning: bool,
    #[serde(default)]
    dragon_killed: bool,
    #[serde(default)]
    previously_killed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    respawn_stage: Option<DragonRespawnStage>,
    #[serde(default)]
    respawn_time: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dragon_uuid: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exit_portal_location: Option<[i32; 3]>,
    #[serde(default)]
    gateways: Vec<i32>,
    #[serde(default)]
    respawn_crystals: Vec<Uuid>,
}

const fn default_true() -> bool {
    true
}

impl Default for SavedDragonFight {
    /// Vanilla `createDefault`.
    fn default() -> Self {
        Self {
            needs_state_scanning: true,
            dragon_killed: false,
            previously_killed: false,
            respawn_stage: None,
            respawn_time: 0,
            dragon_uuid: None,
            exit_portal_location: None,
            gateways: Vec::new(),
            respawn_crystals: Vec::new(),
        }
    }
}

/// The fight's mutable fields.
#[derive(Debug)]
struct FightState {
    ticks_since_dragon_seen: i32,
    alive_crystals: i32,
    ticks_since_crystals_scanned: i32,
    ticks_since_last_player_scan: i32,
    dragon_killed: bool,
    previously_killed: bool,
    dragon_uuid: Option<Uuid>,
    needs_state_scanning: bool,
    exit_portal_location: Option<BlockPos>,
    respawn_stage: Option<DragonRespawnStage>,
    respawn_time: i32,
    respawn_crystals: Vec<Uuid>,
    gateways: Vec<i32>,
    /// Whether the dragon ticket is held, so it is released only once.
    ticket_held: bool,
    /// Vanilla `skipArenaLoadedCheck`, for tests.
    skip_arena_loaded_check: bool,
}

/// Vanilla `EnderDragonFight`.
pub struct EnderDragonFight {
    origin: BlockPos,
    dragon_event: ServerBossEvent,
    exit_portal_pattern: BlockPattern,
    state: SyncMutex<FightState>,
}

impl EnderDragonFight {
    /// Vanilla `getPodiumLocation`: the podium sits at the fight origin.
    #[must_use]
    pub const fn podium_location(origin: BlockPos) -> BlockPos {
        origin
    }

    /// Loads `data/ender_dragon_fight.toml`, starting fresh when it is
    /// absent or unreadable, and runs vanilla `init`.
    pub(crate) async fn load(
        saved_data: &SavedDataManager,
        world: &Identifier,
        seed: i64,
        origin: BlockPos,
    ) -> Self {
        let saved = saved_data
            .load_or_default::<SavedDragonFight>(saved_data_names::ENDER_DRAGON_FIGHT)
            .await
            .unwrap_or_else(|error| {
                log::warn!(
                    "Could not load the dragon fight of world {world}; starting a new one: {error}"
                );
                SavedDragonFight::default()
            });
        Self::from_saved(&saved, seed, origin)
    }

    /// Vanilla constructor plus `init(level, seed, origin)`.
    pub(crate) fn from_saved(saved: &SavedDragonFight, seed: i64, origin: BlockPos) -> Self {
        let dragon_event = ServerBossEvent::new(
            Uuid::new_v4(),
            TextComponent::translated(translations::ENTITY_MINECRAFT_ENDER_DRAGON.msg()),
            BossBarColor::Pink,
            BossBarOverlay::Progress,
        );
        dragon_event.set_flags(BossBarFlags {
            darken_screen: false,
            play_boss_music: true,
            create_world_fog: true,
        });
        let mut gateways = saved.gateways.clone();
        if gateways.is_empty() {
            gateways = (0..GATEWAY_COUNT).collect();
            shuffle(&mut gateways, seed);
        }
        Self {
            origin,
            dragon_event,
            exit_portal_pattern: exit_portal_pattern(),
            state: SyncMutex::new(FightState {
                ticks_since_dragon_seen: 0,
                alive_crystals: 0,
                ticks_since_crystals_scanned: 0,
                ticks_since_last_player_scan: 21,
                dragon_killed: saved.dragon_killed,
                previously_killed: saved.previously_killed,
                dragon_uuid: saved.dragon_uuid,
                needs_state_scanning: saved.needs_state_scanning,
                exit_portal_location: saved
                    .exit_portal_location
                    .map(|[x, y, z]| BlockPos::new(x, y, z)),
                respawn_stage: saved.respawn_stage,
                respawn_time: saved.respawn_time,
                respawn_crystals: saved.respawn_crystals.clone(),
                gateways,
                ticket_held: false,
                skip_arena_loaded_check: false,
            }),
        }
    }

    /// The saved form of this fight.
    pub(crate) fn to_saved(&self) -> SavedDragonFight {
        let state = self.state.lock();
        SavedDragonFight {
            needs_state_scanning: state.needs_state_scanning,
            dragon_killed: state.dragon_killed,
            previously_killed: state.previously_killed,
            respawn_stage: state.respawn_stage,
            respawn_time: state.respawn_time,
            dragon_uuid: state.dragon_uuid,
            exit_portal_location: state
                .exit_portal_location
                .map(|pos| [pos.x(), pos.y(), pos.z()]),
            gateways: state.gateways.clone(),
            respawn_crystals: state.respawn_crystals.clone(),
        }
    }

    /// Vanilla `skipArenaLoadedCheck`, for tests.
    #[cfg(test)]
    pub(crate) fn skip_arena_loaded_check(&self) {
        self.state.lock().skip_arena_loaded_check = true;
    }

    /// The fight origin.
    #[must_use]
    pub const fn origin(&self) -> BlockPos {
        self.origin
    }

    /// The boss bar.
    #[must_use]
    pub const fn dragon_event(&self) -> &ServerBossEvent {
        &self.dragon_event
    }

    /// Vanilla `dragonUUID`.
    #[must_use]
    pub fn dragon_uuid(&self) -> Option<Uuid> {
        self.state.lock().dragon_uuid
    }

    /// Vanilla `aliveCrystals`.
    #[must_use]
    pub fn alive_crystals(&self) -> i32 {
        self.state.lock().alive_crystals
    }

    /// Vanilla `hasPreviouslyKilledDragon`.
    #[must_use]
    pub fn has_previously_killed_dragon(&self) -> bool {
        self.state.lock().previously_killed
    }

    /// Whether the fight's dragon is dead.
    #[must_use]
    pub fn is_dragon_killed(&self) -> bool {
        self.state.lock().dragon_killed
    }

    /// The respawn ritual's stage, if one is running.
    #[must_use]
    pub fn respawn_stage(&self) -> Option<DragonRespawnStage> {
        self.state.lock().respawn_stage
    }

    /// Vanilla `exitPortalLocation`.
    #[must_use]
    pub fn exit_portal_location(&self) -> Option<BlockPos> {
        self.state.lock().exit_portal_location
    }

    /// Vanilla `tick`.
    pub fn tick(&self, world: &Arc<World>) {
        self.dragon_event
            .set_visible(!self.state.lock().dragon_killed);
        let scan_players = {
            let mut state = self.state.lock();
            state.ticks_since_last_player_scan += 1;
            if state.ticks_since_last_player_scan >= TIME_BETWEEN_PLAYER_SCANS {
                state.ticks_since_last_player_scan = 0;
                true
            } else {
                false
            }
        };
        if scan_players {
            self.update_players(world);
        }

        if self.dragon_event.players().is_empty() {
            self.set_ticket(world, false);
            return;
        }
        self.set_ticket(world, true);
        if !self.is_arena_loaded(world) {
            return;
        }

        if self.state.lock().needs_state_scanning {
            self.scan_state(world);
            self.state.lock().needs_state_scanning = false;
        }

        if self.state.lock().respawn_stage.is_some() && !self.tick_respawn(world) {
            return;
        }

        if !self.state.lock().dragon_killed {
            let find_dragon = {
                let mut state = self.state.lock();
                // Vanilla counts only while it knows a dragon.
                let find = state.dragon_uuid.is_none() || {
                    state.ticks_since_dragon_seen += 1;
                    state.ticks_since_dragon_seen >= MAX_TICKS_BEFORE_DRAGON_RESPAWN
                };
                if find {
                    state.ticks_since_dragon_seen = 0;
                }
                find
            };
            if find_dragon {
                self.find_or_create_dragon(world);
            }
            let scan_crystals = {
                let mut state = self.state.lock();
                state.ticks_since_crystals_scanned += 1;
                state.ticks_since_crystals_scanned >= TIME_BETWEEN_CRYSTAL_SCANS
            };
            if scan_crystals {
                self.update_crystal_count(world);
            }
        }
    }

    /// Adds or removes the arena's dragon chunk ticket.
    fn set_ticket(&self, world: &World, held: bool) {
        let changed = {
            let mut state = self.state.lock();
            let changed = state.ticket_held != held;
            state.ticket_held = held;
            changed
        };
        // Vanilla adds the ticket every tick; adding an existing ticket only
        // refreshes it, so Steel adds it once.
        if changed {
            world
                .chunk_map
                .update_dragon_ticket(ChunkPos::new(0, 0), held);
        }
    }

    /// Vanilla `scanState`: works out from the world whether the dragon was
    /// killed before, for worlds older than the fight data.
    fn scan_state(&self, world: &Arc<World>) {
        log::info!("Scanning for legacy world dragon fight...");
        let active_portal_exists = Self::has_active_exit_portal(world);
        if active_portal_exists {
            log::info!("Found that the dragon has been killed in this world already.");
            self.state.lock().previously_killed = true;
        } else {
            log::info!("Found that the dragon has not yet been killed in this world.");
            self.state.lock().previously_killed = false;
            if self.find_exit_portal(world).is_none() {
                self.spawn_exit_portal(world, false);
            }
        }

        let dragons = dragons(world);
        if let Some(dragon) = dragons.first() {
            log::info!(
                "Found that there's a dragon still alive ({})",
                dragon.uuid()
            );
            let mut state = self.state.lock();
            state.dragon_uuid = Some(dragon.uuid());
            state.dragon_killed = false;
            if !active_portal_exists {
                log::info!("But we didn't have a portal, let's remove it.");
                dragon.set_removed(RemovalReason::Discarded);
                state.dragon_uuid = None;
            }
        } else {
            self.state.lock().dragon_killed = true;
        }

        let mut state = self.state.lock();
        if !state.previously_killed && state.dragon_killed {
            state.dragon_killed = false;
        }
    }

    /// Vanilla `findOrCreateDragon`.
    fn find_or_create_dragon(&self, world: &Arc<World>) {
        if let Some(dragon) = dragons(world).first() {
            log::debug!("Haven't seen our dragon, but found another one to use.");
            self.state.lock().dragon_uuid = Some(dragon.uuid());
        } else {
            log::debug!("Haven't seen the dragon, respawning it");
            self.create_new_dragon(world);
        }
    }

    /// Vanilla `isArenaLoaded`, including its scan of only the arena's
    /// southernmost chunk row.
    fn is_arena_loaded(&self, world: &World) -> bool {
        if self.state.lock().skip_arena_loaded_check {
            return true;
        }
        let origin = ChunkPos::from_block_pos(self.origin);
        let (cx, cz) = (origin.0.x, origin.0.y);
        for x in cx - ARENA_SIZE_CHUNKS..=cx + ARENA_SIZE_CHUNKS {
            // Vanilla's inner loop starts at +8, so it checks a single row.
            let z = cz + ARENA_SIZE_CHUNKS;
            if !world
                .chunk_map
                .is_block_ticking_full_chunk_loaded(ChunkPos::new(x, z))
            {
                return false;
            }
        }
        (cx - 1..=cx + 1).all(|x| {
            (cz - 1..=cz + 1).all(|z| world.entity_manager.is_chunk_loaded(ChunkPos::new(x, z)))
        })
    }

    /// Vanilla `updatePlayers`: the boss bar follows living players within
    /// 192 blocks of the dragon's spawn point.
    fn update_players(&self, world: &World) {
        let center = DVec3::new(
            f64::from(self.origin.x()),
            f64::from(DRAGON_SPAWN_Y + self.origin.y()),
            f64::from(self.origin.z()),
        );
        let current = self.dragon_event.players();
        let mut valid = Vec::new();
        world.players.iter_players(|_, player| {
            if LivingEntity::is_alive(player.as_ref())
                && player.position().distance_squared(center)
                    <= VALID_PLAYER_DISTANCE * VALID_PLAYER_DISTANCE
            {
                valid.push(Arc::clone(player));
            }
            true
        });
        for player in &valid {
            self.dragon_event.add_player(player);
        }
        for player in &current {
            if !valid.iter().any(|p| Arc::ptr_eq(p, player)) {
                self.dragon_event.remove_player(player);
            }
        }
    }

    /// Vanilla `updateCrystalCount`: the crystals still on the towers.
    fn update_crystal_count(&self, world: &World) {
        let alive: usize = spikes(world)
            .iter()
            .map(|spike| crystals_in(world, &spike_top_box(spike)).len())
            .sum();
        let mut state = self.state.lock();
        state.ticks_since_crystals_scanned = 0;
        state.alive_crystals = i32::try_from(alive).unwrap_or(i32::MAX);
        log::debug!("Found {} end crystals still alive", state.alive_crystals);
    }

    /// Vanilla `setDragonKilled`: opens the exit portal and the next gateway,
    /// and places the egg after the first kill.
    pub fn set_dragon_killed(&self, world: &Arc<World>, dragon: &EnderDragonEntity) {
        if self.state.lock().dragon_uuid != Some(dragon.uuid()) {
            return;
        }
        self.dragon_event.set_progress(0.0);
        self.dragon_event.set_visible(false);
        self.spawn_exit_portal(world, true);
        self.spawn_new_gateway(world);
        if !self.state.lock().previously_killed {
            let egg = world.heightmap_pos(
                HeightmapType::MotionBlocking,
                Self::podium_location(self.origin),
            );
            world.set_block(
                egg,
                vanilla_blocks::DRAGON_EGG.default_state(),
                UpdateFlags::UPDATE_ALL,
            );
        }
        let mut state = self.state.lock();
        state.previously_killed = true;
        state.dragon_killed = true;
    }

    /// Vanilla `createNewDragon`.
    fn create_new_dragon(&self, world: &Arc<World>) -> Option<Arc<EnderDragonEntity>> {
        let position = DVec3::new(
            f64::from(self.origin.x()),
            f64::from(DRAGON_SPAWN_Y + self.origin.y()),
            f64::from(self.origin.z()),
        );
        let dragon = Arc::new(EnderDragonEntity::new(
            &vanilla_entities::ENDER_DRAGON,
            0,
            position,
            Arc::downgrade(world),
        ));
        dragon.join_fight();
        dragon.set_fight_origin(self.origin);
        dragon.set_phase(DragonPhase::HoldingPattern);
        dragon.snap_to(position, rand::random::<f32>() * 360.0, 0.0);
        let shared: SharedEntity = Arc::<EnderDragonEntity>::clone(&dragon);
        if let Err(error) = world.try_add_entity(shared) {
            log::warn!("Could not spawn the Ender Dragon: {error}");
            return None;
        }
        self.state.lock().dragon_uuid = Some(dragon.uuid());
        Some(dragon)
    }

    /// Vanilla `updateDragon`: the boss bar follows the dragon's health.
    pub fn update_dragon(&self, dragon: &EnderDragonEntity) {
        {
            let mut state = self.state.lock();
            if state.dragon_uuid != Some(dragon.uuid()) {
                return;
            }
            state.ticks_since_dragon_seen = 0;
        }
        self.dragon_event
            .set_progress(dragon.get_health() / dragon.get_max_health());
        if dragon.custom_name().is_some() {
            self.dragon_event.set_name(dragon.display_name());
        }
    }

    /// Vanilla `onCrystalDestroyed`: a ritual crystal breaks the ritual, a
    /// tower crystal hurts the dragon.
    pub fn on_crystal_destroyed(
        &self,
        world: &Arc<World>,
        crystal: &SharedEntity,
        source: &DamageSource,
    ) {
        let breaks_ritual = {
            let state = self.state.lock();
            state.respawn_stage.is_some() && state.respawn_crystals.contains(&crystal.uuid())
        };
        if breaks_ritual {
            self.abort_respawn_sequence(world);
            return;
        }
        self.update_crystal_count(world);
        let dragon_uuid = self.state.lock().dragon_uuid;
        if let Some(dragon) = dragon_uuid.and_then(|uuid| world.get_entity_by_uuid(&uuid))
            && let Some(dragon) = dragon.downcast_ref::<EnderDragonEntity>()
        {
            dragon.on_crystal_destroyed(world, crystal, crystal.block_position(), source);
        }
    }
}

/// Vanilla `ServerLevel.getDragons`: the living Ender Dragons in the world.
fn dragons(world: &World) -> Vec<SharedEntity> {
    world
        .entity_manager
        .get_accessible_entities()
        .into_iter()
        .filter(|entity| {
            entity.downcast_ref::<EnderDragonEntity>().is_some()
                && entity
                    .as_living_entity()
                    .is_some_and(LivingEntity::is_alive)
        })
        .collect()
}

/// Vanilla `EndSpikeFeature.getSpikesForLevel`.
fn spikes(world: &World) -> Vec<EndSpike> {
    FeatureDecorationRunner::end_spikes_for_level(world.seed())
}

/// Vanilla `EndSpike.getTopBoundingBox`.
fn spike_top_box(spike: &EndSpike) -> WorldAabb {
    WorldAabb::new(
        f64::from(spike.center_x - spike.radius),
        SPIKE_BOX_MIN_Y,
        f64::from(spike.center_z - spike.radius),
        f64::from(spike.center_x + spike.radius),
        SPIKE_BOX_MAX_Y,
        f64::from(spike.center_z + spike.radius),
    )
}

/// Vanilla `getEntitiesOfClass(EndCrystal.class, box)`.
fn crystals_in(world: &World, aabb: &WorldAabb) -> Vec<SharedEntity> {
    world.get_entities_in_aabb_matching(aabb, |entity| {
        entity.downcast_ref::<EndCrystalEntity>().is_some()
    })
}

/// Vanilla `Util.shuffle(list, RandomSource.createThreadLocalInstance(seed))`.
fn shuffle(values: &mut [i32], seed: i64) {
    let mut random = LegacyRandom::from_seed(seed.cast_unsigned());
    for index in (1..values.len()).rev() {
        let bound = i32::try_from(index + 1).unwrap_or(i32::MAX);
        let swap = usize::try_from(random.next_i32_bounded(bound)).unwrap_or(0);
        values.swap(index, swap);
    }
}
