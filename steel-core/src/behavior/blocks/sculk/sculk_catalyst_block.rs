//! Vanilla `SculkCatalystBlock`: blooms when a mob dies nearby and spreads
//! sculk with its experience.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, BoolProperty};
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_entity_types;
use steel_utils::types::UpdateFlags;
use steel_utils::value_providers::IntProvider;
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::block::{BlockBehavior, BlockEntityCreation, try_drop_experience};
use crate::behavior::context::BlockPlaceContext;
use crate::block_entity::{BLOCK_ENTITIES, BlockEntityTicker};
use crate::world::World;

/// Vanilla `SculkCatalystBlock.PULSE`.
pub(crate) const PULSE: &BoolProperty = &BlockStateProperties::BLOOM;

/// Vanilla `SculkCatalystBlock`.
#[block_behavior]
pub struct SculkCatalystBlock {
    block: BlockRef,
    #[json_arg(int_provider, json = "xp_range")]
    experience: IntProvider,
}

impl SculkCatalystBlock {
    /// Creates sculk catalyst behavior with its extracted experience provider.
    #[must_use]
    pub const fn new(block: BlockRef, experience: IntProvider) -> Self {
        Self { block, experience }
    }
}

impl BlockBehavior for SculkCatalystBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state())
    }

    /// Vanilla `tick`: the bloom ends.
    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if state.get_value(PULSE) {
            world.set_block(pos, state.set_value(PULSE, false), UpdateFlags::UPDATE_ALL);
        }
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

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::SCULK_CATALYST,
            level,
            pos,
            state,
        ))
    }

    fn get_block_entity_ticker(
        &self,
        _world: &Arc<World>,
        _state: BlockStateId,
        block_entity_type: BlockEntityTypeRef,
    ) -> Option<BlockEntityTicker> {
        BlockEntityTicker::for_matching_entity_tick(
            block_entity_type,
            &vanilla_block_entity_types::SCULK_CATALYST,
        )
    }
}
