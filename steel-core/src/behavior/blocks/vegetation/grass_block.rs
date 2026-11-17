use std::sync::Arc;

use rand::{Rng, RngExt};
use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::{vanilla_blocks, vanilla_placed_features};
use steel_utils::{BlockPos, BlockStateId, Direction};

use super::bonemealable::{
    BonemealAction, Bonemealable, bone_meal_features, place_bonemeal_feature,
    place_bonemeal_placed_feature,
};
use super::snowy_block::{snowy_placement_state, update_snowy_shape};
use super::spreading_grass_block::SpreadingGrassBlock;
use crate::behavior::BLOCK_BEHAVIORS;
use crate::behavior::block::BlockBehavior;
use crate::behavior::context::BlockPlaceContext;
use crate::world::{LevelReader, ScheduledTickAccess, World};

/// Vanilla's number of spots one bonemeal use tries.
const BONEMEAL_ATTEMPTS: i32 = 128;
/// Vanilla's attempts per extra random step away from the grass block.
const ATTEMPTS_PER_STEP: i32 = 16;
/// Vanilla's chance short grass on a tried spot grows tall.
const GROW_SHORT_GRASS_CHANCE: f32 = 0.1;
/// Vanilla's chance an empty spot gets one of the biome's flowers.
const FLOWER_CHANCE: f32 = 0.125;

/// Behavior for grass blocks.
#[block_behavior]
pub struct GrassBlock {
    block: BlockRef,
}

impl GrassBlock {
    /// Creates a new grass block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

impl GrassBlock {
    /// Vanilla `stopBonemealSpread`: the walk stops off this block or inside
    /// a full block.
    fn stop_bonemeal_spread(&self, world: &Arc<World>, pos: BlockPos) -> bool {
        if world.get_block_state(pos.below()).get_block() != self.block {
            return true;
        }
        let state = world.get_block_state(pos);
        world.is_collision_shape_full_block_at(pos, state)
    }

    /// Vanilla `placeBonemealEffect`: grows short grass now and then, and
    /// fills an empty spot with one of the biome's flowers or grass.
    fn place_bonemeal_effect(world: &Arc<World>, rng: &mut dyn Rng, pos: BlockPos) {
        let state = world.get_block_state(pos);
        if state.get_block() == &vanilla_blocks::SHORT_GRASS
            && rng.random::<f32>() < GROW_SHORT_GRASS_CHANCE
            && let Some(grass) = BLOCK_BEHAVIORS
                .get_behavior(&vanilla_blocks::SHORT_GRASS)
                .as_bonemealable()
            && grass.is_valid_bonemeal_target(state, world.as_ref(), pos)
        {
            grass.perform_bonemeal(state, world, rng, pos);
        }

        if !state.is_air() || world.is_outside_build_height(pos.y()) {
            return;
        }
        if rng.random::<f32>() < FLOWER_CHANCE {
            let Some(biome) = world.biome_at(pos) else {
                return;
            };
            let features = bone_meal_features(biome);
            if features.is_empty() {
                return;
            }
            let feature = features[rng.random_range(0..features.len())];
            place_bonemeal_feature(world, feature, rng, pos);
        } else {
            place_bonemeal_placed_feature(
                world,
                &vanilla_placed_features::GRASS_BONEMEAL,
                rng,
                pos,
            );
        }
    }
}

impl BlockBehavior for GrassBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(snowy_placement_state(self.block, context))
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        _world: &dyn ScheduledTickAccess,
        _pos: BlockPos,
        direction: Direction,
        _neighbor_pos: BlockPos,
        neighbor_state: BlockStateId,
    ) -> BlockStateId {
        update_snowy_shape(state, direction, neighbor_state)
    }

    fn random_tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        SpreadingGrassBlock::random_tick(
            &vanilla_blocks::GRASS_BLOCK,
            &vanilla_blocks::DIRT,
            state,
            world,
            pos,
        );
    }

    fn as_bonemealable(&self) -> Option<&dyn Bonemealable> {
        Some(self)
    }
}

impl Bonemealable for GrassBlock {
    fn is_valid_bonemeal_target(
        &self,
        _state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
    ) -> bool {
        let above = pos.above();
        world.get_block_state(above).is_air() && !world.is_outside_build_height(above.y())
    }

    fn perform_bonemeal(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        rng: &mut dyn Rng,
        pos: BlockPos,
    ) {
        let above = pos.above();
        'attempts: for attempt in 0..BONEMEAL_ATTEMPTS {
            let mut test_pos = above;
            for _ in 0..attempt / ATTEMPTS_PER_STEP {
                let dx = rng.random_range(-1..=1);
                let dy = rng.random_range(-1..=1) * rng.random_range(0..3) / 2;
                let dz = rng.random_range(-1..=1);
                test_pos = test_pos.offset(dx, dy, dz);
                if self.stop_bonemeal_spread(world, test_pos) {
                    continue 'attempts;
                }
            }
            Self::place_bonemeal_effect(world, rng, test_pos);
        }
    }

    fn bonemeal_action_type(&self) -> BonemealAction {
        BonemealAction::NeighborSpreader
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use steel_registry::blocks::block_state_ext::BlockStateExt;
    use steel_registry::blocks::properties::BlockStateProperties;
    use steel_registry::vanilla_block_tags::BlockTag;
    use steel_registry::{init_vanilla_registry, vanilla_biomes, vanilla_blocks};
    use steel_utils::types::UpdateFlags;
    use steel_utils::{BlockPos, ChunkPos, Direction};

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::test_support::{TestLevel, fresh_test_world, insert_ready_full_chunk};

    #[test]
    fn plains_bone_meal_grows_plains_flowers() {
        init_vanilla_registry();
        let keys: Vec<String> = bone_meal_features(&vanilla_biomes::PLAINS)
            .iter()
            .map(|feature| feature.key.to_string())
            .collect();
        assert_eq!(keys, ["minecraft:flower_plain"]);
        let keys: Vec<String> = bone_meal_features(&vanilla_biomes::DESERT)
            .iter()
            .map(|feature| feature.key.to_string())
            .collect();
        assert_eq!(keys, ["minecraft:flower_default"]);
    }

    #[test]
    fn bonemeal_on_grass_grows_plants_around() {
        init_vanilla_registry();
        init_behaviors();
        let fixture = fresh_test_world("grass_bonemeal");
        let world = &fixture.world;
        let origin = BlockPos::new(8, 64, 8);
        let center = ChunkPos::from_block_pos(origin);
        for chunk_x in center.0.x - 1..=center.0.x + 1 {
            for chunk_z in center.0.y - 1..=center.0.y + 1 {
                insert_ready_full_chunk(world, ChunkPos::new(chunk_x, chunk_z));
            }
        }
        for x in origin.x() - 10..=origin.x() + 10 {
            for z in origin.z() - 10..=origin.z() + 10 {
                world.set_block(
                    BlockPos::new(x, origin.y(), z),
                    vanilla_blocks::GRASS_BLOCK.default_state(),
                    UpdateFlags::UPDATE_NONE,
                );
            }
        }
        let behavior = GrassBlock::new(&vanilla_blocks::GRASS_BLOCK);
        let state = vanilla_blocks::GRASS_BLOCK.default_state();
        assert!(behavior.is_valid_bonemeal_target(state, world.as_ref(), origin));

        behavior.perform_bonemeal(state, world, &mut StdRng::seed_from_u64(7), origin);

        let mut grass = 0;
        let mut flowers = 0;
        for x in origin.x() - 10..=origin.x() + 10 {
            for z in origin.z() - 10..=origin.z() + 10 {
                let block = world
                    .get_block_state(BlockPos::new(x, origin.y() + 1, z))
                    .get_block();
                if block == &vanilla_blocks::SHORT_GRASS || block == &vanilla_blocks::TALL_GRASS {
                    grass += 1;
                } else if block.has_tag(&BlockTag::FLOWERS) {
                    flowers += 1;
                }
            }
        }
        assert!(grass > 10, "bonemeal spreads grass, got {grass}");
        assert!(flowers > 0, "bonemeal grows some flowers");
    }

    #[test]
    fn bonemeal_needs_air_above_grass() {
        init_vanilla_registry();
        let pos = BlockPos::new(0, 64, 0);
        let level =
            TestLevel::default().with_block(pos.above(), vanilla_blocks::STONE.default_state());
        let behavior = GrassBlock::new(&vanilla_blocks::GRASS_BLOCK);
        assert!(!behavior.is_valid_bonemeal_target(
            vanilla_blocks::GRASS_BLOCK.default_state(),
            &level,
            pos
        ));
    }

    #[test]
    fn grass_block_updates_snowy_state() {
        init_vanilla_registry();
        init_behaviors();

        let level = TestLevel::default();
        let pos = BlockPos::new(0, 64, 0);
        let behavior = GrassBlock::new(&vanilla_blocks::GRASS_BLOCK);

        let non_snowy = vanilla_blocks::GRASS_BLOCK.default_state();
        let snowy = behavior.update_shape(
            non_snowy,
            &level,
            pos,
            Direction::Up,
            pos.above(),
            vanilla_blocks::SNOW.default_state(),
        );
        assert!(snowy.get_value(&BlockStateProperties::SNOWY));

        let cleared = behavior.update_shape(
            snowy,
            &level,
            pos,
            Direction::Up,
            pos.above(),
            vanilla_blocks::AIR.default_state(),
        );
        assert!(!cleared.get_value(&BlockStateProperties::SNOWY));
    }
}
