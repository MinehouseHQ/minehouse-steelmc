//! Vanilla `SculkCatalystBlockEntity`: hears mobs die nearby and spreads
//! sculk with the experience they would have dropped.

use std::mem;
use std::sync::{Arc, Weak};

use glam::DVec3;
use rand::RngExt as _;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::game_events::GameEventRef;
use steel_registry::particle_type::ParticleData;
use steel_registry::{
    sound_events, vanilla_block_entity_types, vanilla_game_events, vanilla_particle_types,
};
use steel_utils::locks::SyncMutex;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::behavior::blocks::PULSE;
use crate::behavior::blocks::spreader::SculkSpreader;
use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;
use crate::world::game_event::{
    GameEventContext, GameEventDeliveryMode, GameEventListener, SharedGameEventListener,
};

/// Vanilla `CatalystListener.PULSE_TICKS`.
const PULSE_TICKS: i32 = 8;
/// Vanilla `CatalystListener.getListenerRadius`.
const LISTENER_RADIUS: i32 = 8;

/// Vanilla `SculkCatalystBlockEntity.CatalystListener`.
struct CatalystListener {
    pos: BlockPos,
    /// The state the block entity was made with, which vanilla blooms from.
    block_state: BlockStateId,
    spreader: SyncMutex<SculkSpreader>,
    /// Vanilla spreads with the level's random; each catalyst keeps one here.
    random: SyncMutex<LegacyRandom>,
}

impl CatalystListener {
    /// Vanilla `bloom`.
    fn bloom(&self, world: &Arc<World>) {
        let pos = self.pos;
        world.set_block(
            pos,
            self.block_state.set_value(PULSE, true),
            UpdateFlags::UPDATE_ALL,
        );
        world.schedule_block_tick_default(pos, self.block_state.get_block(), PULSE_TICKS);
        world.send_particles(
            ParticleData::simple(&vanilla_particle_types::SCULK_SOUL),
            DVec3::new(
                f64::from(pos.x()) + 0.5,
                f64::from(pos.y()) + 1.15,
                f64::from(pos.z()) + 0.5,
            ),
            2,
            DVec3::new(0.2, 0.0, 0.2),
            0.0,
        );
        world.play_block_sound(
            &sound_events::BLOCK_SCULK_CATALYST_BLOOM,
            pos,
            2.0,
            0.6 + rand::rng().random::<f32>() * 0.4,
            None,
        );
    }
}

impl GameEventListener for CatalystListener {
    fn listener_pos(&self) -> Option<DVec3> {
        let (x, y, z) = self.pos.get_center();
        Some(DVec3::new(x, y, z))
    }

    fn listener_radius(&self) -> i32 {
        LISTENER_RADIUS
    }

    fn delivery_mode(&self) -> GameEventDeliveryMode {
        GameEventDeliveryMode::ByDistance
    }

    /// Vanilla `handleGameEvent`: a dying mob's experience becomes sculk
    /// charge instead of orbs.
    fn handle_game_event(
        &self,
        world: &Arc<World>,
        event: GameEventRef,
        context: &GameEventContext<'_>,
        source_pos: DVec3,
    ) -> bool {
        if event.key != vanilla_game_events::ENTITY_DIE.key {
            return false;
        }
        let Some(mob) = context
            .source_entity()
            .and_then(|entity| entity.as_living_entity())
        else {
            return false;
        };
        if !mob.was_experience_consumed() {
            let killer = mob
                .last_damage_source()
                .and_then(|source| source.causing_entity().cloned());
            let experience = mob.experience_reward(world, killer.as_deref());
            if mob.should_drop_experience() && experience > 0 {
                self.spreader.lock().add_cursors(
                    BlockPos::containing(source_pos.x, source_pos.y + 0.5, source_pos.z),
                    experience,
                );
                // TODO(advancements): vanilla triggers KILL_MOB_NEAR_SCULK_CATALYST
                // for the player who last hurt the mob.
            }
            mob.skip_drop_experience();
            self.bloom(world);
        }
        true
    }
}

/// Vanilla `SculkCatalystBlockEntity`.
pub struct SculkCatalystBlockEntity {
    base: BlockEntityBase,
    listener: Arc<CatalystListener>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `SculkCatalystBlockEntity`.
unsafe impl DowncastType for SculkCatalystBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/sculk_catalyst");
}

impl SculkCatalystBlockEntity {
    /// Creates a sculk catalyst.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::SCULK_CATALYST,
                level,
                pos,
                state,
            ),
            listener: Arc::new(CatalystListener {
                pos,
                block_state: state,
                spreader: SyncMutex::new(SculkSpreader::level()),
                random: SyncMutex::new(LegacyRandom::from_seed(rand::random())),
            }),
        }
    }

    /// A copy of the catalyst's spreader.
    #[must_use]
    pub fn spreader(&self) -> SculkSpreader {
        self.listener.spreader.lock().clone()
    }

    /// Adds sculk charge at `pos`, as a mob dying there would.
    pub fn add_cursors(&self, pos: BlockPos, charge: i32) {
        self.listener.spreader.lock().add_cursors(pos, charge);
    }
}

impl BlockEntity for SculkCatalystBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt: NbtCompoundView<'_, '_> = nbt.into();
        self.listener.spreader.lock().load(&nbt);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.listener.spreader.lock().save(nbt);
    }

    fn game_event_listener(&self) -> Option<SharedGameEventListener> {
        Some(Arc::clone(&self.listener) as SharedGameEventListener)
    }

    /// Vanilla `serverTick`.
    fn tick(&self, world: &Arc<World>) {
        // The spreader is taken out while it spreads, so a mob dying from the
        // block changes can still reach the listener; its charge joins after.
        let mut spreader =
            mem::replace(&mut *self.listener.spreader.lock(), SculkSpreader::level());
        let mut random = self.listener.random.lock();
        spreader.update_cursors(world, self.listener.pos, &mut *random, true);
        drop(random);
        let mut current = self.listener.spreader.lock();
        spreader.append(&mut current);
        *current = spreader;
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use simdnbt::borrow::read_compound;
    use simdnbt::owned::{NbtList, NbtTag};
    use steel_registry::blocks::BlockRef;
    use steel_registry::blocks::properties::Direction;
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_entities};
    use steel_utils::{ChunkPos, Downcast as _, WorldAabb};

    use super::*;
    use crate::behavior::blocks::spreader::ChargeCursor;
    use crate::behavior::{BLOCK_BEHAVIORS, init_behaviors};
    use crate::block_entity::{SharedBlockEntity, init_block_entities};
    use crate::entity::create_entity_instance;
    use crate::entity::init_entities;
    use crate::test_support::{TestWorld, fresh_test_world, insert_ready_full_chunk};

    const CATALYST: BlockPos = BlockPos::new(8, 64, 8);

    /// A catalyst on a stone floor, with the chunks around it ticking.
    fn arena(name: &'static str) -> TestWorld {
        init_vanilla_registry();
        init_behaviors();
        init_block_entities();
        init_entities();
        let fixture = fresh_test_world(name);
        let world = &fixture.world;
        let center = ChunkPos::from_block_pos(CATALYST);
        for dx in -1..=1 {
            for dz in -1..=1 {
                insert_ready_full_chunk(world, ChunkPos::new(center.0.x + dx, center.0.y + dz));
            }
        }
        for x in -8..24 {
            for z in -8..24 {
                world.set_block(
                    BlockPos::new(x, 63, z),
                    vanilla_blocks::STONE.default_state(),
                    UpdateFlags::UPDATE_ALL,
                );
            }
        }
        assert!(world.set_block(
            CATALYST,
            vanilla_blocks::SCULK_CATALYST.default_state(),
            UpdateFlags::UPDATE_ALL,
        ));
        fixture
    }

    fn block_entity(world: &World) -> SharedBlockEntity {
        world.get_block_entity(CATALYST).expect("a sculk catalyst")
    }

    fn catalyst(block_entity: &SharedBlockEntity) -> &SculkCatalystBlockEntity {
        block_entity
            .downcast_ref::<SculkCatalystBlockEntity>()
            .expect("a sculk catalyst block entity")
    }

    fn blooming(world: &World) -> bool {
        world.get_block_state(CATALYST).get_value(PULSE)
    }

    fn count_blocks(world: &World, block: BlockRef) -> usize {
        let mut count = 0;
        for x in -8..24 {
            for y in 63..=65 {
                for z in -8..24 {
                    if world.get_block_state(BlockPos::new(x, y, z)).get_block() == block {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    fn experience_orbs(world: &World) -> usize {
        world
            .get_entities_in_aabb(&WorldAabb::new(0.0, 60.0, 0.0, 16.0, 70.0, 16.0))
            .iter()
            .filter(|entity| entity.entity_type() == &vanilla_entities::EXPERIENCE_ORB)
            .count()
    }

    #[test]
    fn a_mob_dying_nearby_turns_its_experience_into_charge_and_the_catalyst_blooms() {
        let fixture = arena("sculk_catalyst_death");
        let world = &fixture.world;
        let zombie = create_entity_instance(
            world,
            &vanilla_entities::ZOMBIE,
            DVec3::new(11.5, 64.0, 8.5),
        )
        .expect("zombie factory");
        world
            .try_add_entity(Arc::clone(&zombie))
            .expect("zombie added");
        zombie.kill(world);

        let block_entity = block_entity(world);
        let cursors = catalyst(&block_entity).spreader().cursors().to_vec();
        assert_eq!(cursors.len(), 1);
        assert_eq!(cursors[0].pos(), BlockPos::new(11, 64, 8));
        assert_eq!(cursors[0].charge(), 5);
        assert!(blooming(world));
        assert_eq!(experience_orbs(world), 0);

        // The bloom fades on the scheduled tick.
        let state = world.get_block_state(CATALYST);
        BLOCK_BEHAVIORS
            .get_behavior(state.get_block())
            .tick(state, world, CATALYST);
        assert!(!blooming(world));
    }

    #[test]
    fn a_mob_dying_out_of_range_drops_its_experience() {
        let fixture = arena("sculk_catalyst_far_death");
        let world = &fixture.world;
        let zombie = create_entity_instance(
            world,
            &vanilla_entities::ZOMBIE,
            DVec3::new(20.5, 64.0, 8.5),
        )
        .expect("zombie factory");
        world
            .try_add_entity(Arc::clone(&zombie))
            .expect("zombie added");
        zombie.kill(world);

        assert_eq!(catalyst(&block_entity(world)).spreader().cursors(), []);
        assert!(!blooming(world));
    }

    #[test]
    fn charge_spreads_sculk_over_the_floor() {
        let fixture = arena("sculk_catalyst_spread");
        let world = &fixture.world;
        let block_entity = block_entity(world);
        let catalyst = catalyst(&block_entity);
        catalyst.add_cursors(BlockPos::new(14, 64, 8), 200);
        for _ in 0..600 {
            block_entity.tick(world);
            world.level_data.write().advance_game_time();
        }

        assert!(count_blocks(world, &vanilla_blocks::SCULK) > 0);
        // Charge is spent spreading; the spreader runs dry.
        assert_eq!(catalyst.spreader().cursors(), []);
    }

    #[test]
    fn cursors_that_meet_merge_and_lone_cursors_in_the_air_fade() {
        let fixture = arena("sculk_catalyst_merge");
        let world = &fixture.world;
        let mut spreader = SculkSpreader::level();
        let mut random = LegacyRandom::from_seed(7);
        let up_high = BlockPos::new(8, 100, 8);
        spreader.add_cursors(up_high, 300);
        spreader.add_cursors(up_high, 200);
        assert_eq!(spreader.cursors().len(), 2);

        spreader.update_cursors(world, CATALYST, &mut random, true);
        assert_eq!(spreader.cursors().len(), 1);
        assert_eq!(spreader.cursors()[0].charge(), 500);

        // With nothing to cling to, the cursor decays away.
        spreader.update_cursors(world, CATALYST, &mut random, true);
        spreader.update_cursors(world, CATALYST, &mut random, true);
        assert_eq!(spreader.cursors(), []);
    }

    #[test]
    fn charge_over_the_cap_splits_into_cursors_of_a_thousand() {
        init_vanilla_registry();
        let mut spreader = SculkSpreader::level();
        spreader.add_cursors(CATALYST, 2500);
        let charges = spreader
            .cursors()
            .iter()
            .map(ChargeCursor::charge)
            .collect::<Vec<_>>();
        assert_eq!(charges, [1000, 1000, 500]);
    }

    fn reload(saved: &NbtCompound) -> SculkSpreader {
        let mut bytes = Vec::new();
        saved.write(&mut bytes);
        let borrowed = read_compound(&mut Cursor::new(bytes.as_slice())).expect("valid nbt");
        let view: NbtCompoundView<'_, '_> = (&borrowed).into();
        let mut spreader = SculkSpreader::level();
        spreader.load(&view);
        spreader
    }

    fn cursor_nbt(charge: i32, facings: Option<NbtTag>) -> NbtCompound {
        let mut cursor = NbtCompound::new();
        cursor.insert("pos", NbtTag::IntArray(vec![1, 2, 3]));
        cursor.insert("charge", charge);
        if let Some(facings) = facings {
            cursor.insert("facings", facings);
        }
        let mut nbt = NbtCompound::new();
        nbt.insert("cursors", NbtList::Compound(vec![cursor]));
        nbt
    }

    #[test]
    fn cursors_survive_a_reload() {
        let fixture = arena("sculk_catalyst_reload");
        let world = &fixture.world;
        let block_entity = block_entity(world);
        let catalyst = catalyst(&block_entity);
        catalyst.add_cursors(BlockPos::new(12, 64, 8), 40);
        for _ in 0..5 {
            block_entity.tick(world);
        }
        let mut saved = NbtCompound::new();
        catalyst.save_additional(&mut saved);
        let reloaded = reload(&saved);
        assert_eq!(reloaded.cursors(), catalyst.spreader().cursors());

        let mut resaved = NbtCompound::new();
        reloaded.save(&mut resaved);
        assert_eq!(resaved, saved);
    }

    #[test]
    fn a_bad_cursor_drops_the_list_and_bad_facings_are_ignored() {
        init_vanilla_registry();
        assert_eq!(reload(&cursor_nbt(2000, None)).cursors(), []);

        let named = NbtTag::List(NbtList::String(vec!["up".into(), "north".into()]));
        let spreader = reload(&cursor_nbt(10, Some(named)));
        let facings = spreader.cursors()[0].facings().expect("facings");
        assert_eq!(
            facings.iter().collect::<Vec<_>>(),
            [Direction::Up, Direction::North]
        );

        let unknown = NbtTag::List(NbtList::String(vec!["sideways".into()]));
        let spreader = reload(&cursor_nbt(10, Some(unknown)));
        assert_eq!(spreader.cursors()[0].charge(), 10);
        assert_eq!(spreader.cursors()[0].facings(), None);

        let mut too_many = NbtCompound::new();
        too_many.insert(
            "cursors",
            NbtList::Compound(
                (0..33)
                    .map(|_| {
                        let mut cursor = NbtCompound::new();
                        cursor.insert("pos", NbtTag::IntArray(vec![0, 0, 0]));
                        cursor
                    })
                    .collect(),
            ),
        );
        assert_eq!(reload(&too_many).cursors(), []);
    }
}
