//! Vanilla `StructureBlock`: saves and loads structure templates, from its
//! screen or on a rising redstone signal.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::properties::StructureMode;
use steel_utils::{BlockPos, BlockStateId, Downcast as _};

use crate::behavior::block::{BlockBehavior, BlockEntityCreation};
use crate::behavior::context::PlacementSource;
use crate::behavior::{BlockHitResult, BlockPlaceContext, InteractionResult, InventoryAccess};
use crate::block_entity::entities::StructureBlockEntity;
use crate::entity::Entity as _;
use crate::player::Player;
use crate::world::{SignalGetter as _, World};

/// Vanilla `StructureBlock`.
#[block_behavior]
pub struct StructureBlock {
    block: BlockRef,
}

impl StructureBlock {
    /// Creates the behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `trigger`: what a rising signal does in each mode.
    fn trigger(world: &Arc<World>, structure: &StructureBlockEntity) {
        match structure.settings().mode {
            StructureMode::Save => {
                structure.save_structure(world, false);
            }
            StructureMode::Load => structure.place_structure(world),
            StructureMode::Corner => structure.unload_structure(world),
            StructureMode::Data => {}
        }
    }
}

impl BlockBehavior for StructureBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state())
    }

    /// Vanilla `useWithoutItem`: the client opens the screen itself from the
    /// block's data; game masters get a success.
    fn use_without_item(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        let Some(block_entity) = world.get_block_entity(pos) else {
            return InteractionResult::Pass;
        };
        if block_entity
            .downcast_ref::<StructureBlockEntity>()
            .is_some()
            && player.can_use_game_master_blocks()
        {
            InteractionResult::Success
        } else {
            InteractionResult::Pass
        }
    }

    /// Vanilla `setPlacedBy`: the placer becomes the author.
    fn set_placed_by(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        source: &PlacementSource<'_>,
    ) {
        let Some(player) = source.player() else {
            return;
        };
        if let Some(block_entity) = world.get_block_entity(pos)
            && let Some(structure) = block_entity.downcast_ref::<StructureBlockEntity>()
        {
            structure.created_by(player.plain_text_name());
        }
    }

    /// Vanilla `neighborChanged`: a rising signal triggers the block.
    fn handle_neighbor_changed(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source_block: BlockRef,
        _moved_by_piston: bool,
    ) {
        let Some(block_entity) = world.get_block_entity(pos) else {
            return;
        };
        let Some(structure) = block_entity.downcast_ref::<StructureBlockEntity>() else {
            return;
        };
        let should_trigger = world.has_neighbor_signal(pos);
        let is_powered = structure.is_powered();
        if should_trigger && !is_powered {
            structure.set_powered(true);
            Self::trigger(world, structure);
        } else if !should_trigger && is_powered {
            structure.set_powered(false);
        }
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(Some(Arc::new(StructureBlockEntity::new(
            level, pos, state,
        ))))
    }

    fn is_game_master_block(&self) -> bool {
        true
    }
}
