//! Vanilla `SculkBlock`.

use std::sync::Arc;

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::item_stack::ItemStack;
use steel_utils::value_providers::IntProvider;
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::{BlockBehavior, BlockPlaceContext, try_drop_experience};
use crate::world::World;

/// Vanilla `SculkBlock`: drops experience when mined. How it takes a sculk
/// charge is part of the [spreader](super::spreader).
#[block_behavior]
pub struct SculkBlock {
    block: BlockRef,
    #[json_arg(int_provider, json = "xp_range")]
    experience: IntProvider,
}

impl SculkBlock {
    /// Creates sculk behavior with its extracted experience provider.
    #[must_use]
    pub const fn new(block: BlockRef, experience: IntProvider) -> Self {
        Self { block, experience }
    }
}

impl BlockBehavior for SculkBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state())
    }

    fn spawn_after_break(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        tool: &ItemStack,
        drop_experience: bool,
    ) {
        if drop_experience {
            try_drop_experience(world, pos, tool, &self.experience);
        }
    }
}
