//! Vanilla `JigsawBlock`: the connector game masters put in structure templates.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_protocol::packets::game::CBlockEntityData;
use steel_registry::RegistryEntry as _;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction, FrontAndTop};
use steel_utils::serial::OptionalNbt;
use steel_utils::{BlockPos, BlockStateId, Downcast as _};

use crate::behavior::block::{BlockBehavior, BlockEntityCreation};
use crate::behavior::{BlockHitResult, BlockPlaceContext, InteractionResult, InventoryAccess};
use crate::block_entity::entities::JigsawBlockEntity;
use crate::player::Player;
use crate::world::World;

/// Vanilla `JigsawBlock`.
#[block_behavior]
pub struct JigsawBlock {
    block: BlockRef,
}

impl JigsawBlock {
    /// Creates the behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

impl BlockBehavior for JigsawBlock {
    /// Vanilla `getStateForPlacement`: the front faces out of the clicked
    /// face; on a floor or ceiling the top points away from the player.
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        let front = context.clicked_face();
        let top = if matches!(front, Direction::Up | Direction::Down) {
            context.horizontal_direction().opposite()
        } else {
            Direction::Up
        };
        let orientation = FrontAndTop::from_front_and_top(front, top)?;
        Some(
            self.block
                .default_state()
                .set_value(&BlockStateProperties::ORIENTATION, orientation),
        )
    }

    /// Vanilla `useWithoutItem`: game masters open the jigsaw screen.
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
        if block_entity.downcast_ref::<JigsawBlockEntity>().is_none()
            || !player.can_use_game_master_blocks()
        {
            return InteractionResult::Pass;
        }
        // Vanilla `ServerPlayer.openJigsawBlock`.
        player.send_packet(CBlockEntityData {
            pos,
            block_entity_type: block_entity.get_type().id() as i32,
            nbt: OptionalNbt(Some(block_entity.save_custom_only())),
        });
        InteractionResult::Success
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(Some(Arc::new(JigsawBlockEntity::new(
            level, pos, state,
        ))))
    }

    fn is_game_master_block(&self) -> bool {
        true
    }
}
