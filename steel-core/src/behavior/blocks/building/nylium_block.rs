//! Vanilla `NyliumBlock`: crimson and warped nylium.

use std::sync::Arc;

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_data::Direction;
use steel_registry::{vanilla_blocks, vanilla_configured_features};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::blocks::vegetation::bonemealable::{
    BonemealAction, Bonemealable, place_bonemeal_feature,
};
use crate::behavior::{BlockBehavior, BlockPlaceContext};
use crate::chunk::light::{MAX_LIGHT_LEVEL, get_light_block_into};
use crate::world::{LevelReader, World};

/// Vanilla `NyliumBlock`.
#[block_behavior]
pub struct NyliumBlock {
    block: BlockRef,
}

impl NyliumBlock {
    /// Creates the behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `NyliumBlock.canBeNylium`: light still reaches the top face.
    fn can_be_nylium(state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> bool {
        let above_state = world.get_block_state(pos.above());
        get_light_block_into(
            state,
            above_state,
            Direction::Up,
            above_state.get_light_dampening(),
        ) < MAX_LIGHT_LEVEL
    }
}

impl BlockBehavior for NyliumBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state())
    }

    /// Vanilla `NyliumBlock.randomTick`: covered nylium turns back into netherrack.
    fn random_tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if !Self::can_be_nylium(state, world.as_ref(), pos) {
            world.set_block(
                pos,
                vanilla_blocks::NETHERRACK.default_state(),
                UpdateFlags::UPDATE_ALL,
            );
        }
    }

    fn as_bonemealable(&self) -> Option<&dyn Bonemealable> {
        Some(self)
    }
}

impl Bonemealable for NyliumBlock {
    fn is_valid_bonemeal_target(
        &self,
        _state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
    ) -> bool {
        world.get_block_state(pos.above()).is_air() && !world.is_outside_build_height(pos.y() + 1)
    }

    fn perform_bonemeal(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        rng: &mut dyn rand::Rng,
        pos: BlockPos,
    ) {
        let above = pos.above();
        if !world.is_outside_build_height(above.y()) {
            place_bonemeal_feature(
                world,
                &vanilla_configured_features::NYLIUM_BONEMEAL,
                rng,
                above,
            );
        }
    }

    fn bonemeal_action_type(&self) -> BonemealAction {
        BonemealAction::NeighborSpreader
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng as _;
    use rand::rngs::StdRng;
    use steel_registry::init_vanilla_registry;
    use steel_utils::ChunkPos;

    use super::*;
    use crate::behavior::{BLOCK_BEHAVIORS, init_behaviors};
    use crate::test_support::{fresh_test_world, insert_ready_full_chunk};

    #[test]
    fn covered_nylium_turns_back_into_netherrack() {
        init_vanilla_registry();
        init_behaviors();
        let fixture = fresh_test_world("nylium_covered");
        let world = &fixture.world;
        let pos = BlockPos::new(8, 70, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        let behavior = BLOCK_BEHAVIORS.get_behavior(&vanilla_blocks::CRIMSON_NYLIUM);
        let nylium = vanilla_blocks::CRIMSON_NYLIUM.default_state();

        assert!(world.set_block(pos, nylium, UpdateFlags::UPDATE_NONE));
        behavior.random_tick(nylium, world, pos);
        assert_eq!(world.get_block_state(pos), nylium, "open sky keeps nylium");

        assert!(world.set_block(
            pos.above(),
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_NONE,
        ));
        behavior.random_tick(nylium, world, pos);
        assert_eq!(
            world.get_block_state(pos).get_block(),
            &vanilla_blocks::NETHERRACK
        );
    }

    #[test]
    fn bonemealed_nylium_grows_nether_vegetation() {
        init_vanilla_registry();
        init_behaviors();
        let fixture = fresh_test_world("nylium_bonemeal");
        let world = &fixture.world;
        let pos = BlockPos::new(8, 70, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        for x in 2..14 {
            for z in 2..14 {
                assert!(world.set_block(
                    BlockPos::new(x, 70, z),
                    vanilla_blocks::WARPED_NYLIUM.default_state(),
                    UpdateFlags::UPDATE_NONE,
                ));
            }
        }
        let behavior = BLOCK_BEHAVIORS.get_behavior(&vanilla_blocks::WARPED_NYLIUM);
        let bonemealable = behavior.as_bonemealable().expect("nylium takes bonemeal");
        let state = vanilla_blocks::WARPED_NYLIUM.default_state();
        assert!(bonemealable.is_valid_bonemeal_target(state, world.as_ref(), pos));

        bonemealable.perform_bonemeal(state, world, &mut StdRng::seed_from_u64(7), pos);

        let grown = (2..14)
            .flat_map(|x| (2..14).map(move |z| BlockPos::new(x, 71, z)))
            .filter(|&plant| !world.get_block_state(plant).is_air())
            .count();
        assert!(grown > 0, "no vegetation grew");
    }
}
