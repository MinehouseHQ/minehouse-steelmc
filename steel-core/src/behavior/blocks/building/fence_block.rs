//! Fence block behavior implementation.
//!
//! Fences connect to adjacent fences, fence gates, and solid blocks.

use std::sync::Arc;

use crate::behavior::block::{BlockBehavior, schedule_water_tick_if_waterlogged};
use crate::behavior::blocks::utils::is_excluded_for_connection;
use crate::behavior::context::BlockPlaceContext;
use crate::behavior::items::LeadItem;
use crate::behavior::{InteractionResult, InventoryAccess};
use crate::entity::ai::path::PathComputationType;
use crate::player::Player;
use crate::world::{LevelReader, ScheduledTickAccess, World};
use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty,
};
use steel_registry::items::item::BlockHitResult;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_utils::{BlockPos, BlockStateId};

/// Behavior for fence blocks.
///
/// Fences have 4 boolean properties (north, east, south, west) that indicate
/// whether the fence connects in that direction. A fence connects to:
/// - Other fences of the same kind (wooden or not)
/// - Fence gates facing the appropriate direction
/// - Blocks with a sturdy face on the connecting side, except leaves, barriers,
///   pumpkins, melons and shulker boxes
#[block_behavior]
pub struct FenceBlock {
    block: BlockRef,
}

const HORIZONTAL_FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;
/// North connection property.
pub(crate) const NORTH: &BoolProperty = &BlockStateProperties::NORTH;
/// East connection property.
pub(crate) const EAST: &BoolProperty = &BlockStateProperties::EAST;
/// South connection property.
pub(crate) const SOUTH: &BoolProperty = &BlockStateProperties::SOUTH;
/// West connection property.
pub(crate) const WEST: &BoolProperty = &BlockStateProperties::WEST;
/// Waterlogged property.
pub(crate) const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;

impl FenceBlock {
    /// Creates a new fence block behavior for the given block.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `FenceBlock.connectsTo` for the neighbor at `neighbor_pos`, which lies in
    /// `direction` from the fence.
    fn connects_to(
        &self,
        world: &dyn LevelReader,
        neighbor_state: BlockStateId,
        neighbor_pos: BlockPos,
        direction: Direction,
    ) -> bool {
        let towards_fence = direction.opposite();
        let face_solid = world.is_face_sturdy(neighbor_state, neighbor_pos, towards_fence);
        let neighbor_block = neighbor_state.get_block();
        let gate = neighbor_block.has_tag(&BlockTag::FENCE_GATES)
            && connects_to_gate(neighbor_state, towards_fence);
        (!is_excluded_for_connection(neighbor_block) && face_solid)
            || self.is_same_fence(neighbor_state)
            || gate
    }

    /// Vanilla `FenceBlock.isSameFence`: wooden fences and nether brick fences never join.
    fn is_same_fence(&self, neighbor_state: BlockStateId) -> bool {
        let neighbor = neighbor_state.get_block();
        neighbor.has_tag(&BlockTag::FENCES)
            && neighbor.has_tag(&BlockTag::WOODEN_FENCES)
                == self.block.has_tag(&BlockTag::WOODEN_FENCES)
    }

    /// Gets the connection state for a position by checking all 4 horizontal neighbors.
    fn get_connection_state(&self, world: &Arc<World>, pos: BlockPos) -> BlockStateId {
        let mut state = self.block.default_state();

        // Check north
        let north_pos = Direction::North.relative(pos);
        let north_state = world.get_block_state(north_pos);
        let connects_north = self.connects_to(world, north_state, north_pos, Direction::North);
        state = state.set_value(NORTH, connects_north);

        // Check east
        let east_pos = Direction::East.relative(pos);
        let east_state = world.get_block_state(east_pos);
        let connects_east = self.connects_to(world, east_state, east_pos, Direction::East);
        state = state.set_value(EAST, connects_east);

        // Check south
        let south_pos = Direction::South.relative(pos);
        let south_state = world.get_block_state(south_pos);
        let connects_south = self.connects_to(world, south_state, south_pos, Direction::South);
        state = state.set_value(SOUTH, connects_south);

        // Check west
        let west_pos = Direction::West.relative(pos);
        let west_state = world.get_block_state(west_pos);
        let connects_west = self.connects_to(world, west_state, west_pos, Direction::West);
        state = state.set_value(WEST, connects_west);

        state
    }
}

/// Vanilla `FenceGateBlock.connectsToDirection`.
fn connects_to_gate(gate_state: BlockStateId, direction: Direction) -> bool {
    gate_state
        .try_get_value(HORIZONTAL_FACING)
        .is_some_and(|facing| facing.axis() == direction.rotate_y_clockwise().axis())
}

impl BlockBehavior for FenceBlock {
    fn is_pathfindable(
        &self,
        _state: BlockStateId,
        _computation_type: PathComputationType,
    ) -> bool {
        false
    }

    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        log::debug!(
            "FenceBlock::get_state_for_placement called for {:?} at {:?}",
            self.block.key,
            context.place_pos()
        );
        Some(
            self.get_connection_state(context.world, context.place_pos())
                .set_value(WATERLOGGED, context.is_water_source()),
        )
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        direction: Direction,
        neighbor_pos: BlockPos,
        neighbor_state: BlockStateId,
    ) -> BlockStateId {
        schedule_water_tick_if_waterlogged(state, world, pos);

        // Only update for horizontal directions
        match direction {
            Direction::North => {
                let connects =
                    self.connects_to(world, neighbor_state, neighbor_pos, Direction::North);
                state.set_value(NORTH, connects)
            }
            Direction::East => {
                let connects =
                    self.connects_to(world, neighbor_state, neighbor_pos, Direction::East);
                state.set_value(EAST, connects)
            }
            Direction::South => {
                let connects =
                    self.connects_to(world, neighbor_state, neighbor_pos, Direction::South);
                state.set_value(SOUTH, connects)
            }
            Direction::West => {
                let connects =
                    self.connects_to(world, neighbor_state, neighbor_pos, Direction::West);
                state.set_value(WEST, connects)
            }
            // Vertical directions don't affect fence connections
            Direction::Up | Direction::Down => state,
        }
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
        LeadItem::bind_player_mobs(player, world, pos)
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_fluids};
    use steel_utils::BlockPos;

    use crate::test_support::TestLevel;

    use super::*;

    #[test]
    fn waterlogged_fence_update_shape_schedules_water_tick() {
        init_vanilla_registry();

        let behavior = FenceBlock::new(&vanilla_blocks::OAK_FENCE);
        let state = vanilla_blocks::OAK_FENCE
            .default_state()
            .set_value(WATERLOGGED, true);
        let level = TestLevel::default();

        let updated = behavior.update_shape(
            state,
            &level,
            BlockPos::ZERO,
            Direction::Up,
            Direction::Up.relative(BlockPos::ZERO),
            vanilla_blocks::AIR.default_state(),
        );

        assert_eq!(updated, state);
        assert_eq!(
            level
                .scheduled_fluid_ticks
                .borrow()
                .iter()
                .map(|tick| (tick.fluid, tick.delay))
                .collect::<Vec<_>>(),
            vec![(&vanilla_fluids::WATER, 5)]
        );
    }

    fn east_after_neighbor(fence: BlockRef, neighbor: BlockStateId) -> bool {
        let behavior = FenceBlock::new(fence);
        let level = TestLevel::default();
        let updated = behavior.update_shape(
            fence.default_state(),
            &level,
            BlockPos::ZERO,
            Direction::East,
            Direction::East.relative(BlockPos::ZERO),
            neighbor,
        );
        updated.get_value(EAST)
    }

    #[test]
    fn fences_skip_connection_exceptions_but_join_solid_blocks() {
        init_vanilla_registry();

        assert!(east_after_neighbor(
            &vanilla_blocks::OAK_FENCE,
            vanilla_blocks::STONE.default_state()
        ));
        for exception in [
            &vanilla_blocks::CARVED_PUMPKIN,
            &vanilla_blocks::JACK_O_LANTERN,
            &vanilla_blocks::PUMPKIN,
            &vanilla_blocks::MELON,
            &vanilla_blocks::BARRIER,
            &vanilla_blocks::OAK_LEAVES,
            &vanilla_blocks::SHULKER_BOX,
        ] {
            assert!(
                !east_after_neighbor(&vanilla_blocks::OAK_FENCE, exception.default_state()),
                "{} must not join a fence",
                exception.key
            );
        }
    }

    #[test]
    fn wooden_and_nether_brick_fences_do_not_join() {
        init_vanilla_registry();

        assert!(east_after_neighbor(
            &vanilla_blocks::OAK_FENCE,
            vanilla_blocks::CRIMSON_FENCE.default_state()
        ));
        assert!(!east_after_neighbor(
            &vanilla_blocks::OAK_FENCE,
            vanilla_blocks::NETHER_BRICK_FENCE.default_state()
        ));
        assert!(!east_after_neighbor(
            &vanilla_blocks::NETHER_BRICK_FENCE,
            vanilla_blocks::BAMBOO_FENCE.default_state()
        ));
        assert!(east_after_neighbor(
            &vanilla_blocks::NETHER_BRICK_FENCE,
            vanilla_blocks::NETHER_BRICK_FENCE.default_state()
        ));
    }

    #[test]
    fn fences_join_gates_side_on() {
        init_vanilla_registry();

        let gate = vanilla_blocks::OAK_FENCE_GATE.default_state();
        assert!(east_after_neighbor(
            &vanilla_blocks::NETHER_BRICK_FENCE,
            gate.set_value(HORIZONTAL_FACING, Direction::North)
        ));
        assert!(!east_after_neighbor(
            &vanilla_blocks::NETHER_BRICK_FENCE,
            gate.set_value(HORIZONTAL_FACING, Direction::East)
        ));
    }
}
