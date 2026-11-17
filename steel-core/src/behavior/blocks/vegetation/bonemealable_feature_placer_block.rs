//! Vanilla `BonemealableFeaturePlacerBlock`: moss blocks that grow a patch when bonemealed.

use std::sync::{Arc, LazyLock};

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::feature::ConfiguredFeature;
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::blocks::vegetation::bonemealable::{
    BonemealAction, Bonemealable, place_bonemeal_feature,
};
use crate::behavior::{BlockBehavior, BlockPlaceContext};
use crate::world::{LevelReader, World};

/// Vanilla `BonemealableFeaturePlacerBlock`.
#[block_behavior]
pub struct BonemealableFeaturePlacerBlock {
    block: BlockRef,
    #[json_arg(vanilla_configured_features, json = "feature")]
    feature: &'static LazyLock<ConfiguredFeature>,
}

impl BonemealableFeaturePlacerBlock {
    /// Creates the behavior.
    #[must_use]
    pub const fn new(block: BlockRef, feature: &'static LazyLock<ConfiguredFeature>) -> Self {
        Self { block, feature }
    }
}

impl BlockBehavior for BonemealableFeaturePlacerBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state())
    }

    fn as_bonemealable(&self) -> Option<&dyn Bonemealable> {
        Some(self)
    }
}

impl Bonemealable for BonemealableFeaturePlacerBlock {
    fn is_valid_bonemeal_target(
        &self,
        _state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
    ) -> bool {
        world.get_block_state(pos.above()).is_air()
    }

    fn perform_bonemeal(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        rng: &mut dyn rand::Rng,
        pos: BlockPos,
    ) {
        place_bonemeal_feature(world, self.feature, rng, pos.above());
    }

    fn bonemeal_action_type(&self) -> BonemealAction {
        BonemealAction::NeighborSpreader
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng as _;
    use rand::rngs::StdRng;
    use steel_registry::{init_vanilla_registry, vanilla_blocks};
    use steel_utils::ChunkPos;
    use steel_utils::types::UpdateFlags;

    use super::*;
    use crate::behavior::{BLOCK_BEHAVIORS, init_behaviors};
    use crate::test_support::{fresh_test_world, insert_ready_full_chunk};

    #[test]
    fn bonemealed_moss_spreads_over_the_stone_around_it() {
        init_vanilla_registry();
        init_behaviors();
        let fixture = fresh_test_world("moss_bonemeal");
        let world = &fixture.world;
        let pos = BlockPos::new(8, 70, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        for x in 1..15 {
            for z in 1..15 {
                assert!(world.set_block(
                    BlockPos::new(x, 70, z),
                    vanilla_blocks::STONE.default_state(),
                    UpdateFlags::UPDATE_NONE,
                ));
            }
        }
        let moss = vanilla_blocks::MOSS_BLOCK.default_state();
        assert!(world.set_block(pos, moss, UpdateFlags::UPDATE_NONE));
        let behavior = BLOCK_BEHAVIORS.get_behavior(&vanilla_blocks::MOSS_BLOCK);
        let bonemealable = behavior.as_bonemealable().expect("moss takes bonemeal");
        assert!(bonemealable.is_valid_bonemeal_target(moss, world.as_ref(), pos));

        bonemealable.perform_bonemeal(moss, world, &mut StdRng::seed_from_u64(3), pos);

        let mossy = (1..15)
            .flat_map(|x| (1..15).map(move |z| BlockPos::new(x, 70, z)))
            .filter(|&floor| {
                world.get_block_state(floor).get_block() == &vanilla_blocks::MOSS_BLOCK
            })
            .count();
        assert!(mossy > 1, "moss did not spread");
    }
}
