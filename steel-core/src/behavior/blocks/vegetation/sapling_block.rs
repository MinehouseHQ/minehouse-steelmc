use std::sync::Arc;

use rand::{Rng, RngExt};
use steel_macros::block_behavior;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::{BlockStateProperties, IntProperty};
use steel_registry::vanilla_block_tags::BlockTag;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, Direction};

use crate::behavior::block::BlockBehavior;
use crate::behavior::blocks::vegetation::bonemealable::Bonemealable;
use crate::behavior::blocks::vegetation::vegetation_block::survival_update_shape;
use crate::behavior::context::BlockPlaceContext;
use crate::world::{LevelReader, ScheduledTickAccess, World};

use super::{BlockRef, TreeGrower, default_surviving_state, survives_on_tag};

const STAGE: &IntProperty = &BlockStateProperties::STAGE;
/// Vanilla's minimum light above a sapling for it to grow.
const MIN_GROWTH_LIGHT: u8 = 9;
/// Vanilla's one-in-seven chance a lit sapling advances each random tick.
const RANDOM_GROWTH_BOUND: u32 = 7;
/// Vanilla's chance bonemeal advances a sapling.
const BONEMEAL_SUCCESS_CHANCE: f32 = 0.45;

/// Vanilla `SaplingBlock`: survives on vegetation soil, and grows through
/// two stages into the trees of its [`TreeGrower`].
#[block_behavior]
pub struct SaplingBlock {
    block: BlockRef,
    #[json_arg(r#enum = "TreeGrower", json = "tree_grower_name")]
    tree_grower: TreeGrower,
}

impl SaplingBlock {
    /// Creates a new sapling block behavior.
    #[must_use]
    pub const fn new(block: BlockRef, tree_grower: TreeGrower) -> Self {
        Self { block, tree_grower }
    }

    /// Vanilla `advanceTree`: stage 0 ripens to stage 1, and a ripe sapling
    /// grows its tree.
    fn advance_tree(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        rng: &mut dyn Rng,
    ) {
        if state.get_value(STAGE) == 0 {
            world.set_block(pos, state.set_value(STAGE, 1), UpdateFlags::UPDATE_NONE);
        } else {
            self.tree_grower.grow_tree(world, pos, state, rng);
        }
    }

    fn random_tick_with_rng(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        rng: &mut dyn Rng,
    ) {
        if world.max_local_raw_brightness(pos.above(), world.sky_darkening()) >= MIN_GROWTH_LIGHT
            && rng.random_range(0..RANDOM_GROWTH_BOUND) == 0
        {
            self.advance_tree(world, pos, state, rng);
        }
    }
}

impl BlockBehavior for SaplingBlock {
    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        _direction: Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        survival_update_shape(self, state, world, pos)
    }

    fn can_survive(&self, _state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> bool {
        survives_on_tag(world, pos, &BlockTag::SUPPORTS_VEGETATION)
    }

    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        default_surviving_state(self.block, self, context)
    }

    fn random_tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        self.random_tick_with_rng(state, world, pos, &mut rand::rng());
    }

    fn as_bonemealable(&self) -> Option<&dyn Bonemealable> {
        Some(self)
    }
}

impl Bonemealable for SaplingBlock {
    fn is_valid_bonemeal_target(
        &self,
        state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
    ) -> bool {
        if !self.tree_grower.can_grow(world, pos, state) {
            return false;
        }
        let height = self.tree_grower.minimum_height().unwrap_or(0);
        !world.is_outside_build_height(pos.above_n(height).y())
    }

    fn is_bonemeal_success(
        &self,
        _state: BlockStateId,
        _world: &Arc<World>,
        rng: &mut dyn Rng,
        _pos: BlockPos,
    ) -> bool {
        rng.random::<f32>() < BONEMEAL_SUCCESS_CHANCE
    }

    fn perform_bonemeal(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        rng: &mut dyn Rng,
        pos: BlockPos,
    ) {
        self.advance_tree(world, pos, state, rng);
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_blocks};
    use steel_utils::ChunkPos;

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::test_support::{fresh_test_world, insert_ready_full_chunk};

    #[test]
    fn sapling_pops_off_when_its_soil_is_removed() {
        init_vanilla_registry();
        init_behaviors();
        let fixture = fresh_test_world("sapling_support_loss");
        let world = &fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let soil = BlockPos::new(4, 64, 4);
        world.set_block(
            soil,
            vanilla_blocks::DIRT.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        world.set_block(
            soil.above(),
            vanilla_blocks::OAK_SAPLING.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        assert_eq!(
            world.get_block_state(soil.above()).get_block(),
            &vanilla_blocks::OAK_SAPLING
        );
        world.set_block(
            soil,
            vanilla_blocks::AIR.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        assert!(world.get_block_state(soil.above()).is_air());
    }
}
