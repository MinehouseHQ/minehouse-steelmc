//! Vanilla `SculkShriekerBlockEntity`: the shrieker's vibration listener, its
//! warnings and the warden it calls.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use glam::DVec3;
use rand::RngExt as _;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, BoolProperty};
use steel_registry::game_events::GameEventRef;
use steel_registry::position_source::{BlockPositionSource, PositionSource};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_game_event_tags::GameEventTag;
use steel_registry::{
    level_events, sound_events, vanilla_block_entity_types, vanilla_entities, vanilla_game_events,
    vanilla_game_rules, vanilla_position_source_types,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::{Difficulty, UpdateFlags};
use steel_utils::{
    BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey, Identifier,
};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::entity::entities::ItemEntity;
use crate::entity::entities::mobs::hostile::WardenEntity;
use crate::entity::spawn_util::{on_top_of_collider, try_spawn_mob};
use crate::entity::{Entity, EntitySpawnReason, SharedEntity};
use crate::player::Player;
use crate::player::warden_spawn_tracker::{MAX_WARNING_LEVEL, WardenSpawnTracker};
use crate::world::World;
use crate::world::game_event::vibrations::{
    VIBRATION_LISTENER_TAG, VibrationData, VibrationListener, VibrationSystem, VibrationUser,
    tick_vibrations,
};
use crate::world::game_event::{GameEventContext, SharedGameEventListener};

const SHRIEKING: &BoolProperty = &BlockStateProperties::SHRIEKING;
const CAN_SUMMON: &BoolProperty = &BlockStateProperties::CAN_SUMMON;

const WARNING_LEVEL_TAG: &str = "warning_level";

/// Vanilla `WARNING_SOUND_RADIUS`.
const WARNING_SOUND_RADIUS: i32 = 10;
/// Vanilla `WARDEN_SPAWN_ATTEMPTS`.
const WARDEN_SPAWN_ATTEMPTS: i32 = 20;
/// Vanilla `WARDEN_SPAWN_RANGE_XZ`.
const WARDEN_SPAWN_RANGE_XZ: i32 = 5;
/// Vanilla `WARDEN_SPAWN_RANGE_Y`.
const WARDEN_SPAWN_RANGE_Y: i32 = 6;
/// Vanilla `DARKNESS_RADIUS`.
const DARKNESS_RADIUS: i32 = 40;
/// Vanilla `SHRIEKING_TICKS`.
pub(crate) const SHRIEKING_TICKS: i32 = 90;
/// Vanilla `VibrationUser.LISTENER_RADIUS`.
const LISTENER_RADIUS: i32 = 8;

/// Vanilla `SOUND_BY_LEVEL`: the warden's reply to each warning level.
fn sound_for_warning_level(warning_level: i32) -> Option<SoundEventRef> {
    match warning_level {
        1 => Some(&sound_events::ENTITY_WARDEN_NEARBY_CLOSE),
        2 => Some(&sound_events::ENTITY_WARDEN_NEARBY_CLOSER),
        3 => Some(&sound_events::ENTITY_WARDEN_NEARBY_CLOSEST),
        4 => Some(&sound_events::ENTITY_WARDEN_LISTENING_ANGRY),
        _ => None,
    }
}

/// Vanilla `tryGetPlayer`: the player behind `source`, whether it is the
/// player, a ride they steer, their projectile or an item they threw.
pub(crate) fn try_get_player(world: &World, source: Option<&dyn Entity>) -> Option<Arc<Player>> {
    let source = source?;
    let player_of = |entity: &dyn Entity| {
        entity
            .as_player()
            .and_then(|_| world.players.get_by_uuid(&entity.uuid()))
    };
    if let Some(player) = player_of(source) {
        return Some(player);
    }
    if let Some(player) = source
        .controlling_passenger()
        .and_then(|passenger| player_of(passenger.as_ref()))
    {
        return Some(player);
    }
    if let Some(player) = source
        .projectile_owner()
        .and_then(|owner| player_of(owner.as_ref()))
    {
        return Some(player);
    }
    let item = source.downcast_ref::<ItemEntity>()?;
    let thrower = world.get_entity_by_uuid(&item.get_thrower()?)?;
    player_of(thrower.as_ref())
}

/// The shrieker's state that its vibration listener reaches, vanilla
/// `SculkShriekerBlockEntity` minus the block entity plumbing.
struct Shrieker {
    pos: BlockPos,
    warning_level: AtomicI32,
    data: SyncMutex<VibrationData>,
    /// Vanilla `onDataChanged` → `setChanged`, applied by the block entity tick.
    data_changed: AtomicBool,
}

impl Shrieker {
    /// Vanilla `tryShriek`.
    fn try_shriek(&self, world: &Arc<World>, player: Option<&Arc<Player>>) {
        let Some(player) = player else {
            return;
        };
        let state = world.get_block_state(self.pos);
        if state.get_value(SHRIEKING) {
            return;
        }
        self.warning_level.store(0, Ordering::Relaxed);
        if !can_respond(world, state) || self.try_to_warn(world, player) {
            self.shriek(world, state, player);
        }
    }

    /// Vanilla `tryToWarn`.
    fn try_to_warn(&self, world: &World, player: &Arc<Player>) -> bool {
        let Some(warning_level) = WardenSpawnTracker::try_warn(world, self.pos, player) else {
            return false;
        };
        self.warning_level.store(warning_level, Ordering::Relaxed);
        true
    }

    /// Vanilla `shriek`.
    fn shriek(&self, world: &Arc<World>, state: BlockStateId, source: &Arc<Player>) {
        world.set_block(
            self.pos,
            state.set_value(SHRIEKING, true),
            UpdateFlags::UPDATE_CLIENTS,
        );
        world.schedule_block_tick_default(self.pos, state.get_block(), SHRIEKING_TICKS);
        world.level_event(level_events::PARTICLES_SCULK_SHRIEK, self.pos, 0, None);
        world.game_event(
            &vanilla_game_events::SHRIEK,
            self.pos,
            &GameEventContext::new(Some(source.as_ref() as &dyn Entity), None),
        );
    }

    /// Vanilla `tryRespond`: after a warning, a warden answers, or at least
    /// darkness and a sound do.
    fn try_respond(&self, world: &Arc<World>, state: BlockStateId) {
        if !can_respond(world, state) || self.warning_level.load(Ordering::Relaxed) <= 0 {
            return;
        }
        if !self.try_summon_warden(world) {
            self.play_warden_reply_sound(world);
        }
        let (x, y, z) = self.pos.get_center();
        WardenEntity::apply_darkness_around(world, DVec3::new(x, y, z), None, DARKNESS_RADIUS);
    }

    /// Vanilla `playWardenReplySound`: somewhere within ten blocks.
    fn play_warden_reply_sound(&self, world: &World) {
        let Some(sound) = sound_for_warning_level(self.warning_level.load(Ordering::Relaxed))
        else {
            return;
        };
        let mut random = rand::rng();
        let mut offset = || random.random_range(-WARNING_SOUND_RADIUS..=WARNING_SOUND_RADIUS);
        let x = self.pos.x() + offset();
        let y = self.pos.y() + offset();
        let z = self.pos.z() + offset();
        world.play_sound_at(
            sound,
            SoundSource::Hostile,
            DVec3::new(f64::from(x), f64::from(y), f64::from(z)),
            5.0,
            1.0,
            None,
        );
    }

    /// Vanilla `trySummonWarden`: only at the last warning level.
    fn try_summon_warden(&self, world: &Arc<World>) -> bool {
        if self.warning_level.load(Ordering::Relaxed) < MAX_WARNING_LEVEL {
            return false;
        }
        try_spawn_mob(
            world,
            &vanilla_entities::WARDEN,
            EntitySpawnReason::Triggered,
            self.pos,
            WARDEN_SPAWN_ATTEMPTS,
            WARDEN_SPAWN_RANGE_XZ,
            WARDEN_SPAWN_RANGE_Y,
            on_top_of_collider,
            false,
        )
        .is_some()
    }
}

/// Vanilla `canRespond`.
fn can_respond(world: &World, state: BlockStateId) -> bool {
    state.get_value(CAN_SUMMON)
        && world.difficulty() != Difficulty::Peaceful
        && world.get_game_rule(&vanilla_game_rules::SPAWN_WARDENS)
}

impl VibrationUser for Shrieker {
    fn vibration_listener_radius(&self) -> i32 {
        LISTENER_RADIUS
    }

    fn vibration_position_source(&self) -> PositionSource {
        PositionSource::new(
            &vanilla_position_source_types::BLOCK,
            BlockPositionSource::new(self.pos),
        )
    }

    fn vibration_listener_pos(&self) -> Option<DVec3> {
        let (x, y, z) = self.pos.get_center();
        Some(DVec3::new(x, y, z))
    }

    fn listenable_events(&self) -> Identifier {
        GameEventTag::SHRIEKER_CAN_LISTEN
    }

    fn requires_adjacent_chunks_to_be_ticking(&self) -> bool {
        true
    }

    fn can_receive_vibration(
        &self,
        world: &Arc<World>,
        _pos: BlockPos,
        _event: GameEventRef,
        context: &GameEventContext<'_>,
    ) -> bool {
        !world.get_block_state(self.pos).get_value(SHRIEKING)
            && try_get_player(world, context.source_entity()).is_some()
    }

    fn on_receive_vibration(
        &self,
        world: &Arc<World>,
        _pos: BlockPos,
        _event: GameEventRef,
        source_entity: Option<&SharedEntity>,
        projectile_owner: Option<&SharedEntity>,
        _receiving_distance: f32,
    ) {
        let source = projectile_owner.or(source_entity);
        let player = try_get_player(world, source.map(|entity| entity.as_ref() as &dyn Entity));
        self.try_shriek(world, player.as_ref());
    }

    fn on_vibration_data_changed(&self) {
        self.data_changed.store(true, Ordering::Relaxed);
    }
}

impl VibrationSystem for Shrieker {
    fn vibration_data(&self) -> &SyncMutex<VibrationData> {
        &self.data
    }
}

/// Vanilla `SculkShriekerBlockEntity`.
pub struct SculkShriekerBlockEntity {
    base: BlockEntityBase,
    shrieker: Arc<Shrieker>,
    listener: Arc<VibrationListener>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `SculkShriekerBlockEntity`.
unsafe impl DowncastType for SculkShriekerBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/sculk_shrieker");
}

impl SculkShriekerBlockEntity {
    /// Creates a sculk shrieker.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        let shrieker = Arc::new(Shrieker {
            pos,
            warning_level: AtomicI32::new(0),
            data: SyncMutex::new(VibrationData::default()),
            data_changed: AtomicBool::new(false),
        });
        let system: Arc<dyn VibrationSystem> = Arc::clone(&shrieker) as _;
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::SCULK_SHRIEKER,
                level,
                pos,
                state,
            ),
            listener: Arc::new(VibrationListener::for_system(&system)),
            shrieker,
        }
    }

    /// The warning level this shrieker last reached.
    #[must_use]
    pub fn warning_level(&self) -> i32 {
        self.shrieker.warning_level.load(Ordering::Relaxed)
    }

    /// Vanilla `tryShriek`.
    pub fn try_shriek(&self, world: &Arc<World>, player: Option<&Arc<Player>>) {
        self.shrieker.try_shriek(world, player);
    }

    /// Vanilla `tryRespond`.
    pub fn try_respond(&self, world: &Arc<World>) {
        self.shrieker.try_respond(world, self.get_block_state());
    }
}

impl BlockEntity for SculkShriekerBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt: NbtCompoundView<'_, '_> = nbt.into();
        self.shrieker
            .warning_level
            .store(nbt.int(WARNING_LEVEL_TAG).unwrap_or(0), Ordering::Relaxed);
        *self.shrieker.data.lock() = nbt
            .compound(VIBRATION_LISTENER_TAG)
            .and_then(VibrationData::load)
            .unwrap_or_default();
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        nbt.insert(WARNING_LEVEL_TAG, self.warning_level());
        nbt.insert(VIBRATION_LISTENER_TAG, self.shrieker.data.lock().save());
    }

    /// Vanilla `preRemoveSideEffects`: a shrieker broken mid-shriek still
    /// gets its answer.
    fn pre_remove_side_effects(&self, _pos: BlockPos, state: BlockStateId) {
        if state.get_value(SHRIEKING)
            && let Some(world) = self.get_level()
        {
            self.shrieker.try_respond(&world, state);
        }
    }

    fn game_event_listener(&self) -> Option<SharedGameEventListener> {
        Some(Arc::clone(&self.listener) as SharedGameEventListener)
    }

    /// Vanilla `VibrationSystem.Ticker.tick`.
    fn tick(&self, world: &Arc<World>) {
        tick_vibrations(world, self.shrieker.as_ref());
        if self.shrieker.data_changed.swap(false, Ordering::Relaxed) {
            self.set_changed();
        }
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_blocks};
    use steel_utils::ChunkPos;

    use super::*;
    use crate::behavior::{BLOCK_BEHAVIORS, init_behaviors};
    use crate::block_entity::{SharedBlockEntity, init_block_entities};
    use crate::entity::{LivingEntity as _, init_entities};
    use crate::player::ResetReason;
    use crate::test_support::{
        TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk,
    };
    use steel_registry::vanilla_mob_effects;

    const SHRIEKER: BlockPos = BlockPos::new(8, 64, 8);

    fn arena(name: &'static str, can_summon: bool) -> TestWorld {
        init_vanilla_registry();
        init_behaviors();
        init_block_entities();
        init_entities();
        let fixture = fresh_test_world(name);
        fixture.world.set_difficulty(Difficulty::Normal);
        let center = ChunkPos::from_block_pos(SHRIEKER);
        for dx in -1..=1 {
            for dz in -1..=1 {
                insert_ready_full_chunk(
                    &fixture.world,
                    ChunkPos::new(center.0.x + dx, center.0.y + dz),
                );
            }
        }
        assert!(
            fixture.world.set_block(
                SHRIEKER,
                vanilla_blocks::SCULK_SHRIEKER
                    .default_state()
                    .set_value(CAN_SUMMON, can_summon),
                UpdateFlags::UPDATE_ALL,
            )
        );
        fixture
    }

    fn block_entity(world: &World) -> SharedBlockEntity {
        world.get_block_entity(SHRIEKER).expect("a sculk shrieker")
    }

    fn shrieker(block_entity: &SharedBlockEntity) -> &SculkShriekerBlockEntity {
        block_entity
            .downcast_ref::<SculkShriekerBlockEntity>()
            .expect("a sculk shrieker block entity")
    }

    fn shrieking(world: &World) -> bool {
        world.get_block_state(SHRIEKER).get_value(SHRIEKING)
    }

    fn player_at(world: &Arc<World>, name: &'static str, id: i32, x: f64) -> Arc<Player> {
        let player = TestPlayerBuilder::new(Arc::clone(world), name, id).build();
        player.snap_to(DVec3::new(x, 65.0, 8.5), 0.0, 0.0);
        assert!(world.players.insert(Arc::clone(&player)));
        player
    }

    fn step_on(world: &Arc<World>, player: &Arc<Player>) {
        let state = world.get_block_state(SHRIEKER);
        BLOCK_BEHAVIORS.get_behavior(state.get_block()).step_on(
            state,
            world,
            SHRIEKER,
            player.as_ref(),
        );
    }

    /// Runs the shrieker's scheduled block tick that ends the shriek.
    fn end_shriek(world: &Arc<World>) {
        let state = world.get_block_state(SHRIEKER);
        BLOCK_BEHAVIORS
            .get_behavior(state.get_block())
            .tick(state, world, SHRIEKER);
    }

    #[test]
    fn a_shrieker_that_cannot_summon_shrieks_without_warning() {
        let fixture = arena("sculk_shrieker_no_summon", false);
        let world = &fixture.world;
        let player = player_at(world, "Walker", 1, 8.5);

        step_on(world, &player);
        assert!(shrieking(world));
        assert_eq!(shrieker(&block_entity(world)).warning_level(), 0);
        assert_eq!(player.warden_spawn_tracker(), WardenSpawnTracker::default());

        end_shriek(world);
        assert!(!shrieking(world));
        assert!(player.mob_effect(vanilla_mob_effects::DARKNESS).is_none());
    }

    #[test]
    fn a_summoning_shrieker_warns_every_player_nearby_once_per_cooldown() {
        let fixture = arena("sculk_shrieker_warning", true);
        let world = &fixture.world;
        let walker = player_at(world, "Walker", 1, 8.5);
        let friend = player_at(world, "Friend", 2, 12.5);
        let far = player_at(world, "Faraway", 3, 40.5);
        friend.set_warden_spawn_tracker(WardenSpawnTracker::new(5, 2, 0));

        step_on(world, &walker);
        assert!(shrieking(world));
        // The highest level nearby rises by one and is shared.
        assert_eq!(shrieker(&block_entity(world)).warning_level(), 3);
        assert_eq!(
            walker.warden_spawn_tracker(),
            WardenSpawnTracker::new(0, 3, 200)
        );
        assert_eq!(
            friend.warden_spawn_tracker(),
            WardenSpawnTracker::new(0, 3, 200)
        );
        assert_eq!(far.warden_spawn_tracker(), WardenSpawnTracker::default());

        // The warden is not here yet: a reply sound and darkness.
        end_shriek(world);
        assert!(!shrieking(world));
        assert!(walker.mob_effect(vanilla_mob_effects::DARKNESS).is_some());
        assert!(far.mob_effect(vanilla_mob_effects::DARKNESS).is_some());

        // On cooldown the shrieker stays quiet.
        step_on(world, &walker);
        assert!(!shrieking(world));
        assert_eq!(walker.warden_spawn_tracker().warning_level(), 3);
    }

    #[test]
    fn peaceful_worlds_only_get_the_shriek() {
        let fixture = arena("sculk_shrieker_peaceful", true);
        let world = &fixture.world;
        world.set_difficulty(Difficulty::Peaceful);
        let player = player_at(world, "Walker", 1, 8.5);

        step_on(world, &player);
        assert!(shrieking(world));
        assert_eq!(player.warden_spawn_tracker(), WardenSpawnTracker::default());
    }

    #[test]
    fn a_player_vibration_sets_it_off_from_a_distance() {
        let fixture = arena("sculk_shrieker_vibration", false);
        let world = &fixture.world;
        // The vibration finds its source again by UUID, so the player joins
        // the world's entities.
        let player = TestPlayerBuilder::new(Arc::clone(world), "Walker", 1).build();
        player.snap_to(DVec3::new(12.5, 65.0, 8.5), 0.0, 0.0);
        assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));

        // Shriekers hear only sculk sensors clicking.
        world.game_event(
            &vanilla_game_events::SCULK_SENSOR_TENDRILS_CLICKING,
            BlockPos::new(12, 64, 8),
            &GameEventContext::new(Some(player.as_ref() as &dyn Entity), None),
        );
        let block_entity = block_entity(world);
        for _ in 0..10 {
            world.level_data.write().advance_game_time();
            block_entity.tick(world);
        }
        assert!(shrieking(world));
    }

    #[test]
    fn breaking_it_mid_shriek_still_brings_the_answer() {
        let fixture = arena("sculk_shrieker_broken", true);
        let world = &fixture.world;
        let player = player_at(world, "Walker", 1, 8.5);
        step_on(world, &player);
        assert!(shrieking(world));

        assert!(world.set_block(
            SHRIEKER,
            vanilla_blocks::AIR.default_state(),
            UpdateFlags::UPDATE_ALL,
        ));
        assert!(player.mob_effect(vanilla_mob_effects::DARKNESS).is_some());
    }
}
