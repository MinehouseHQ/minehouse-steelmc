//! The exit portal, the end gateways and placing their features.

use std::f64::consts::PI;
use std::sync::Arc;

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::feature::ConfiguredFeatureRef;
use steel_registry::{
    level_events, vanilla_block_entity_types, vanilla_blocks, vanilla_configured_features,
};
use steel_utils::random::RandomSource;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::random::worldgen_random::WorldgenRandom;
use steel_utils::{BlockPos, BlockStateId, ChunkPos};

use super::{ARENA_SIZE_CHUNKS, EnderDragonFight};
use crate::behavior::block_pattern::{BlockPattern, BlockPatternMatch};
use crate::chunk::heightmap::HeightmapType;
use crate::world::{LevelReader as _, World};
use crate::worldgen::feature::FeatureDecorationRunner;
use crate::worldgen::region::WorldGenRegion;

/// The lowest y the exit portal search walks down through bedrock to.
const EXIT_PORTAL_MIN_SEARCH_Y: i32 = 63;
/// The radius of the ring of end gateways.
const GATEWAY_RING_RADIUS: f64 = 96.0;
/// The height of the end gateways.
const GATEWAY_Y: i32 = 75;

fn is_bedrock(state: BlockStateId) -> bool {
    state.get_block() == &vanilla_blocks::BEDROCK
}

/// Vanilla `EnderDragonFight.exitPortalPattern`: the bedrock frame and pillar
/// of the exit portal, with the pillar on top.
pub(super) fn exit_portal_pattern() -> BlockPattern {
    BlockPattern::new(
        &[
            &[
                "       ", "       ", "       ", "   #   ", "       ", "       ", "       ",
            ],
            &[
                "       ", "       ", "       ", "   #   ", "       ", "       ", "       ",
            ],
            &[
                "       ", "       ", "       ", "   #   ", "       ", "       ", "       ",
            ],
            &[
                "  ###  ", " #   # ", "#     #", "#  #  #", "#     #", " #   # ", "  ###  ",
            ],
            &[
                "       ", "  ###  ", " ##### ", " ##### ", " ##### ", "  ###  ", "       ",
            ],
        ],
        &[('#', is_bedrock)],
    )
}

/// Places a configured feature in the loaded world, as vanilla
/// `Feature.place(level, generator, RandomSource.create(), pos)` does.
pub(super) fn place_feature(
    world: &Arc<World>,
    feature: &ConfiguredFeatureRef,
    pos: BlockPos,
) -> bool {
    let mut region = WorldGenRegion::new_live(
        &world.chunk_map.world_gen_context,
        Arc::clone(world),
        ChunkPos::from_block_pos(pos),
        RandomSource::Legacy(LegacyRandom::from_seed(rand::random())),
    );
    let mut random = WorldgenRandom::from_seed(rand::random());
    FeatureDecorationRunner::place_feature_in_world(&mut region, feature, &mut random, pos)
}

/// Vanilla's gateway position for gateway index `gateway`.
pub(super) fn gateway_pos(gateway: i32) -> BlockPos {
    let angle = 2.0 * (-PI + (PI / 20.0) * f64::from(gateway));
    #[expect(
        clippy::cast_possible_truncation,
        reason = "vanilla Mth.floor of a value within ±96"
    )]
    let (x, z) = (
        (GATEWAY_RING_RADIUS * angle.cos()).floor() as i32,
        (GATEWAY_RING_RADIUS * angle.sin()).floor() as i32,
    );
    BlockPos::new(x, GATEWAY_Y, z)
}

impl EnderDragonFight {
    /// The chunks around `center` (in chunks) within the arena, with any
    /// End portal block entities of loaded chunks.
    ///
    /// Vanilla `level.getChunk` loads each chunk; Steel never blocks the
    /// tick on chunk loading, so a chunk that is not loaded yet counts as
    /// holding no portal. The dragon ticket keeps the arena loaded while
    /// players are near.
    fn end_portal_positions(world: &World, center: ChunkPos) -> Vec<BlockPos> {
        let mut positions = Vec::new();
        for x in center.0.x - ARENA_SIZE_CHUNKS..=center.0.x + ARENA_SIZE_CHUNKS {
            for z in center.0.y - ARENA_SIZE_CHUNKS..=center.0.y + ARENA_SIZE_CHUNKS {
                let found = world
                    .chunk_map
                    .with_full_chunk(ChunkPos::new(x, z), |chunk| {
                        chunk
                            .get_block_entities()
                            .iter()
                            .filter(|entity| {
                                entity.get_type() == &vanilla_block_entity_types::END_PORTAL
                            })
                            .map(|entity| entity.get_block_pos())
                            .collect::<Vec<_>>()
                    });
                positions.extend(found.unwrap_or_default());
            }
        }
        positions
    }

    /// Vanilla `hasActiveExitPortal`: any End portal block within the
    /// chunks -8..=8 around chunk (0, 0).
    pub(super) fn has_active_exit_portal(world: &World) -> bool {
        !Self::end_portal_positions(world, ChunkPos::new(0, 0)).is_empty()
    }

    /// Vanilla `findExitPortal`: the portal frame next to an End portal
    /// block, or else the first frame below the podium.
    pub(super) fn find_exit_portal(&self, world: &World) -> Option<BlockPatternMatch> {
        let origin = ChunkPos::from_block_pos(self.origin);
        for pos in Self::end_portal_positions(world, origin) {
            if let Some(found) = self.exit_portal_pattern.find(world, pos) {
                self.remember_exit_portal(&found);
                return Some(found);
            }
        }
        let podium = Self::podium_location(self.origin);
        let max_y = world
            .heightmap_pos(HeightmapType::MotionBlocking, podium)
            .y();
        for y in (world.min_y()..=max_y).rev() {
            let at = BlockPos::new(podium.x(), y, podium.z());
            if let Some(found) = self.exit_portal_pattern.find(world, at) {
                self.remember_exit_portal(&found);
                return Some(found);
            }
        }
        None
    }

    fn remember_exit_portal(&self, found: &BlockPatternMatch) {
        let mut state = self.state.lock();
        if state.exit_portal_location.is_none() {
            state.exit_portal_location = Some(found.block(3, 3, 3));
        }
    }

    /// Vanilla `spawnExitPortal`: places the podium, with an open portal
    /// when `activated`.
    pub(super) fn spawn_exit_portal(&self, world: &Arc<World>, activated: bool) {
        let location = self.exit_portal_location_or_find(world);
        let feature = if activated {
            &*vanilla_configured_features::END_PODIUM_ACTIVE
        } else {
            &*vanilla_configured_features::END_PODIUM_INACTIVE
        };
        // Vanilla then waits for light before sending the chunks; Steel sends
        // chunks only once their light is ready.
        place_feature(world, &ConfiguredFeatureRef::Reference(feature), location);
    }

    /// The saved exit portal location, or the first non-bedrock block below
    /// the podium's surface, which is then saved.
    fn exit_portal_location_or_find(&self, world: &World) -> BlockPos {
        let saved = self.state.lock().exit_portal_location;
        if let Some(location) = saved {
            return location;
        }
        let podium = Self::podium_location(self.origin);
        let mut location = world
            .heightmap_pos(HeightmapType::MotionBlockingNoLeaves, podium)
            .below();
        while world.get_block_state(location).get_block() == &vanilla_blocks::BEDROCK
            && location.y() > EXIT_PORTAL_MIN_SEARCH_Y
        {
            location = location.below();
        }
        let location = BlockPos::new(
            location.x(),
            location.y().max(world.min_y() + 1),
            location.z(),
        );
        self.state.lock().exit_portal_location = Some(location);
        location
    }

    /// Vanilla `spawnNewGateway()`: opens the next gateway of the shuffled
    /// ring.
    pub(super) fn spawn_new_gateway(&self, world: &Arc<World>) {
        let Some(gateway) = self.state.lock().gateways.pop() else {
            return;
        };
        let pos = gateway_pos(gateway);
        world.level_event(level_events::ANIMATION_END_GATEWAY_SPAWN, pos, 0, None);
        place_feature(
            world,
            &ConfiguredFeatureRef::Reference(&vanilla_configured_features::END_GATEWAY_DELAYED),
            pos,
        );
    }
}
