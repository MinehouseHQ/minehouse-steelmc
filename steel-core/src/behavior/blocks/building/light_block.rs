//! Vanilla `LightBlock`: the invisible light source game masters place.

use std::sync::Arc;

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction, IntProperty};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::block::schedule_water_tick_if_waterlogged;
use crate::behavior::{
    BlockBehavior, BlockHitResult, BlockPlaceContext, InteractionResult, InventoryAccess,
};
use crate::player::Player;
use crate::world::{ScheduledTickAccess, World};

const LEVEL: &IntProperty = &BlockStateProperties::LEVEL;

/// Vanilla `LightBlock`.
#[block_behavior]
pub struct LightBlock {
    block: BlockRef,
}

impl LightBlock {
    /// Creates the behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `BlockState.cycle(LEVEL)`: the next level, wrapping 15 to 0.
    fn cycle_level(state: BlockStateId) -> BlockStateId {
        let level = state.get_value(LEVEL);
        let next = if level >= LEVEL.max {
            LEVEL.min
        } else {
            level + 1
        };
        state.set_value(LEVEL, next)
    }
}

impl BlockBehavior for LightBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state())
    }

    /// Vanilla `LightBlock.useWithoutItem`: game masters cycle the light level.
    fn use_without_item(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        if player.can_use_game_master_blocks() {
            world.set_block(pos, Self::cycle_level(state), UpdateFlags::UPDATE_CLIENTS);
            InteractionResult::SuccessServer
        } else {
            InteractionResult::Consume
        }
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        _direction: Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        schedule_water_tick_if_waterlogged(state, world, pos);
        state
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_blocks};

    use super::*;

    #[test]
    fn the_level_cycles_up_and_wraps_to_zero() {
        init_vanilla_registry();
        let light = vanilla_blocks::LIGHT.default_state();
        assert_eq!(light.get_value(LEVEL), 15);
        let wrapped = LightBlock::cycle_level(light);
        assert_eq!(wrapped.get_value(LEVEL), 0);
        assert_eq!(LightBlock::cycle_level(wrapped).get_value(LEVEL), 1);
    }
}
