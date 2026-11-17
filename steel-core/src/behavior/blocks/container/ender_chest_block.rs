//! Ender chest block behavior implementation.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::{BlockStateProperties, Direction};
use steel_registry::{vanilla_block_entity_types, vanilla_custom_stats};
use steel_utils::{BlockPos, BlockStateId, Downcast as _, translations};
use text_components::TextComponent;

use super::anger_nearby_piglins;
use crate::behavior::InventoryAccess;
use crate::behavior::block::{
    BlockBehavior, BlockEntityCreation, schedule_water_tick_if_waterlogged,
};
use crate::behavior::context::{BlockHitResult, BlockPlaceContext, InteractionResult};
use crate::block_entity::BLOCK_ENTITIES;
use crate::block_entity::entities::EnderChestBlockEntity;
use crate::entity::ai::path::PathComputationType;
use crate::inventory::menu::kinds::ender_chest;
use crate::player::Player;
use crate::world::{ScheduledTickAccess, World, is_redstone_conductor};

/// The ender chest block behavior.
#[block_behavior]
pub struct EnderChestBlock {
    block: BlockRef,
}

impl EnderChestBlock {
    /// Creates a new ender chest block.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

impl BlockBehavior for EnderChestBlock {
    fn is_pathfindable(
        &self,
        _state: BlockStateId,
        _computation_type: PathComputationType,
    ) -> bool {
        false
    }

    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        let facing = context.horizontal_direction().opposite();

        Some(
            self.block
                .default_state()
                .set_value(&BlockStateProperties::FACING, facing)
                .set_value(
                    &BlockStateProperties::WATERLOGGED,
                    context.is_water_source(),
                ),
        )
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
            return InteractionResult::Success;
        };
        if block_entity.get_type() != &vanilla_block_entity_types::ENDER_CHEST {
            return InteractionResult::Success;
        }

        let above_pos = pos.above();
        if is_redstone_conductor(world.as_ref(), world.get_block_state(above_pos), above_pos) {
            return InteractionResult::Success;
        }

        let inventory = Arc::clone(&player.inventory);
        let container = Arc::clone(&player.ender_chest_inventory);
        let chest = Arc::downgrade(&block_entity);
        player.open_menu(
            TextComponent::translated(translations::CONTAINER_ENDERCHEST.msg()),
            move |context| {
                // open only after active chest has closed, since
                // closing an ender chest menu clears the active chest.
                container.lock().set_active_chest(chest);
                ender_chest(inventory, context.container_id, container)
            },
        );

        player.award_custom_stat(&vanilla_custom_stats::OPEN_ENDERCHEST);
        anger_nearby_piglins(world, player, true);

        InteractionResult::Success
    }

    /// Vanilla `EnderChestBlock.tick`: the viewer recheck scheduled while the
    /// chest is open.
    fn tick(&self, _state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if let Some(chest) = world.get_block_entity(pos)
            && chest.downcast_ref::<EnderChestBlockEntity>().is_some()
        {
            chest.recheck_open();
        }
    }

    /// Vanilla `BaseEntityBlock.triggerEvent`: the block entity decides.
    fn trigger_event(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        param_a: i32,
        param_b: i32,
    ) -> bool {
        world
            .get_block_entity(pos)
            .is_some_and(|chest| chest.trigger_event(param_a, param_b))
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::ENDER_CHEST,
            level,
            pos,
            state,
        ))
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_fluids};

    use super::*;
    use crate::test_support::TestLevel;

    #[test]
    fn waterlogged_ender_chests_schedule_water_on_neighbor_updates() {
        init_vanilla_registry();
        let behavior = EnderChestBlock::new(&vanilla_blocks::ENDER_CHEST);
        let pos = BlockPos::new(0, 64, 0);
        let level = TestLevel::default();
        for (waterlogged, expected) in [(false, 0), (true, 1)] {
            let state = vanilla_blocks::ENDER_CHEST
                .default_state()
                .set_value(&BlockStateProperties::WATERLOGGED, waterlogged);
            let updated = behavior.update_shape(
                state,
                &level,
                pos,
                Direction::East,
                pos.east(),
                vanilla_blocks::AIR.default_state(),
            );
            assert_eq!(updated, state);
            assert_eq!(level.scheduled_fluid_ticks.borrow().len(), expected);
        }
        assert_eq!(
            level.scheduled_fluid_ticks.borrow()[0].fluid,
            &vanilla_fluids::WATER
        );
    }
}
