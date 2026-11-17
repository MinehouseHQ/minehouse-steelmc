//! Slab block behavior implementation.
//!
//! Slabs choose top/bottom/double shape during placement and only single slabs
//! implement vanilla waterlogging.

use std::sync::Arc;

use steel_macros::block_behavior;
use steel_registry::{
    REGISTRY,
    blocks::{
        BlockRef,
        block_state_ext::BlockStateExt as _,
        properties::{BlockStateProperties, BoolProperty, Direction, EnumProperty, SlabType},
    },
    fluid::{FluidRef, FluidState},
    vanilla_fluids,
};
use steel_utils::{BlockPos, BlockStateId};

use super::weathering_block::{WeatherState, WeatheringCopper};
use crate::entity::ai::path::PathComputationType;
use crate::{
    behavior::{
        BlockBehavior, BlockPlaceContext,
        block::{place_simple_waterlogged_liquid, schedule_water_tick_if_waterlogged},
    },
    world::{LevelAccessor, ScheduledTickAccess, World},
};

/// Behavior for slab blocks.
#[block_behavior]
pub struct SlabBlock {
    block: BlockRef,
}

const SLAB_TYPE: &EnumProperty<SlabType> = &BlockStateProperties::SLAB_TYPE;
const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;

impl SlabBlock {
    /// Creates a new slab block behavior for the given block.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    fn single_slab_type_for_placement(context: &BlockPlaceContext<'_>) -> SlabType {
        if context.clicked_face() != Direction::Down
            && (context.clicked_face() == Direction::Up
                || context.click_location().y - f64::from(context.place_pos().y()) <= 0.5)
        {
            SlabType::Bottom
        } else {
            SlabType::Top
        }
    }
}

impl BlockBehavior for SlabBlock {
    fn is_pathfindable(&self, state: BlockStateId, computation_type: PathComputationType) -> bool {
        // Vanilla `SlabBlock.isPathfindable`: only water paths, through a
        // waterlogged slab.
        computation_type == PathComputationType::Water && state.get_value(WATERLOGGED)
    }

    /// Vanilla `SlabBlock.canBeReplaced`: a single slab takes a matching slab
    /// clicked into its free half, making a double slab.
    fn can_be_replaced(&self, state: BlockStateId, context: &BlockPlaceContext<'_>) -> bool {
        let slab_type = state.get_value(SLAB_TYPE);
        if slab_type == SlabType::Double
            || !context.with_item(|item| item.item() == REGISTRY.items.by_block(self.block))
        {
            return false;
        }
        if !context.replaces_clicked_block() {
            return true;
        }
        let above = context.click_location().y - f64::from(context.place_pos().y()) > 0.5;
        let clicked_face = context.clicked_face();
        let horizontal = clicked_face.get_axis().is_horizontal();
        if slab_type == SlabType::Bottom {
            clicked_face == Direction::Up || above && horizontal
        } else {
            clicked_face == Direction::Down || !above && horizontal
        }
    }

    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        let existing_state = context.world.get_block_state(context.place_pos());
        if existing_state.get_block() == self.block {
            return Some(
                existing_state
                    .set_value(SLAB_TYPE, SlabType::Double)
                    .set_value(WATERLOGGED, false),
            );
        }

        Some(
            self.block
                .default_state()
                .set_value(SLAB_TYPE, Self::single_slab_type_for_placement(context))
                .set_value(WATERLOGGED, context.is_water_source()),
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

    fn can_place_liquid(&self, state: BlockStateId, fluid: FluidRef) -> bool {
        state.try_get_value(SLAB_TYPE) != Some(SlabType::Double) && fluid == &vanilla_fluids::WATER
    }

    fn place_liquid(
        &self,
        level: &dyn LevelAccessor,
        pos: BlockPos,
        state: BlockStateId,
        fluid_state: FluidState,
    ) -> bool {
        state.try_get_value(SLAB_TYPE) != Some(SlabType::Double)
            && place_simple_waterlogged_liquid(level, pos, state, fluid_state)
    }
}

/// Weathering copper slabs share slab shape/fluid behavior and add copper aging.
#[block_behavior]
pub struct WeatheringCopperSlabBlock {
    slab: SlabBlock,
    #[json_arg(r#enum = "WeatherState", json = "weather_state")]
    weathering: WeatheringCopper,
}

impl WeatheringCopperSlabBlock {
    /// Creates a new weathering copper slab behavior.
    #[must_use]
    pub const fn new(block: BlockRef, weather_state: WeatherState) -> Self {
        Self {
            slab: SlabBlock::new(block),
            weathering: WeatheringCopper::new(weather_state),
        }
    }
}

impl BlockBehavior for WeatheringCopperSlabBlock {
    fn is_pathfindable(&self, state: BlockStateId, computation_type: PathComputationType) -> bool {
        self.slab.is_pathfindable(state, computation_type)
    }

    fn can_be_replaced(&self, state: BlockStateId, context: &BlockPlaceContext<'_>) -> bool {
        self.slab.can_be_replaced(state, context)
    }

    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        self.slab.get_state_for_placement(context)
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
        self.slab
            .update_shape(state, world, pos, direction, neighbor_pos, neighbor_state)
    }

    fn can_place_liquid(&self, state: BlockStateId, fluid: FluidRef) -> bool {
        self.slab.can_place_liquid(state, fluid)
    }

    fn place_liquid(
        &self,
        level: &dyn LevelAccessor,
        pos: BlockPos,
        state: BlockStateId,
        fluid_state: FluidState,
    ) -> bool {
        self.slab.place_liquid(level, pos, state, fluid_state)
    }

    fn random_tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        self.weathering.change_over_time(state, world, pos);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{init_vanilla_registry, vanilla_blocks};
    use steel_utils::types::{InteractionHand, UpdateFlags};
    use steel_utils::{BlockPos, ChunkPos};

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::behavior::{BlockItem, PlacementOrientation, PlacementSource};
    use crate::test_support::{fresh_test_world, insert_ready_full_chunk};
    use crate::world::World;
    use steel_registry::items::item::BlockHitResult;

    fn world(name: &'static str) -> Arc<World> {
        init_vanilla_registry();
        init_behaviors();
        let fixture = fresh_test_world(name);
        insert_ready_full_chunk(&fixture.world, ChunkPos::new(0, 0));
        Arc::clone(&fixture.world)
    }

    /// Places `block`'s item as a player would, clicking `face` of `pos` at `location`.
    fn place(world: &Arc<World>, block: BlockRef, pos: BlockPos, face: Direction, location: DVec3) {
        let mut stack = ItemStack::new(REGISTRY.items.by_block(block));
        let source = PlacementSource::direct(
            None,
            InteractionHand::MainHand,
            &mut stack,
            PlacementOrientation::Player {
                rotation: 0.0,
                pitch: 90.0,
            },
            false,
        );
        let hit = BlockHitResult {
            location,
            direction: face,
            block_pos: pos,
            miss: false,
            inside: false,
            world_border_hit: false,
        };
        BlockItem::new(block).place(BlockPlaceContext::new(world, source, &hit));
    }

    #[test]
    fn clicking_the_top_of_a_bottom_slab_merges_it() {
        let world = world("slab_merge_top");
        let pos = BlockPos::new(4, 64, 4);
        let bottom = vanilla_blocks::OAK_SLAB
            .default_state()
            .set_value(SLAB_TYPE, SlabType::Bottom)
            .set_value(WATERLOGGED, true);
        world.set_block(pos, bottom, UpdateFlags::UPDATE_ALL);

        place(
            &world,
            &vanilla_blocks::OAK_SLAB,
            pos,
            Direction::Up,
            DVec3::new(4.5, 64.5, 4.5),
        );
        let state = world.get_block_state(pos);
        assert_eq!(state.get_value(SLAB_TYPE), SlabType::Double);
        assert!(
            !state.get_value(WATERLOGGED),
            "a double slab drops its water"
        );
    }

    #[test]
    fn a_different_slab_stacks_above_instead_of_merging() {
        let world = world("slab_merge_other");
        let pos = BlockPos::new(4, 64, 4);
        world.set_block(
            pos,
            vanilla_blocks::OAK_SLAB.default_state(),
            UpdateFlags::UPDATE_ALL,
        );

        place(
            &world,
            &vanilla_blocks::STONE_SLAB,
            pos,
            Direction::Up,
            DVec3::new(4.5, 64.5, 4.5),
        );
        assert_eq!(
            world.get_block_state(pos).get_value(SLAB_TYPE),
            SlabType::Bottom
        );
        assert_eq!(
            world.get_block_state(pos.above()).get_block(),
            &vanilla_blocks::STONE_SLAB
        );
    }

    #[test]
    fn side_clicks_merge_only_in_the_empty_half() {
        let world = world("slab_merge_side");
        let low = BlockPos::new(4, 64, 4);
        let high = BlockPos::new(8, 64, 4);
        world.set_block(
            low,
            vanilla_blocks::OAK_SLAB.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        world.set_block(
            high,
            vanilla_blocks::OAK_SLAB.default_state(),
            UpdateFlags::UPDATE_ALL,
        );

        // Lower half of a bottom slab's side: the slab is not replaced.
        place(
            &world,
            &vanilla_blocks::OAK_SLAB,
            low,
            Direction::North,
            DVec3::new(4.5, 64.25, 4.0),
        );
        assert_eq!(
            world.get_block_state(low).get_value(SLAB_TYPE),
            SlabType::Bottom
        );
        // Upper half: the slab fills up to a double slab.
        place(
            &world,
            &vanilla_blocks::OAK_SLAB,
            high,
            Direction::North,
            DVec3::new(8.5, 64.75, 4.0),
        );
        assert_eq!(
            world.get_block_state(high).get_value(SLAB_TYPE),
            SlabType::Double
        );
    }

    #[test]
    fn double_slabs_cannot_be_waterlogged() {
        init_vanilla_registry();
        let behavior = SlabBlock::new(&vanilla_blocks::SMOOTH_STONE_SLAB);
        let double_slab = vanilla_blocks::SMOOTH_STONE_SLAB
            .default_state()
            .set_value(SLAB_TYPE, SlabType::Double)
            .set_value(WATERLOGGED, false);

        assert!(!behavior.can_place_liquid(double_slab, &vanilla_fluids::WATER));
    }

    #[test]
    fn single_slabs_accept_source_water_for_container_admission() {
        init_vanilla_registry();
        let behavior = SlabBlock::new(&vanilla_blocks::SMOOTH_STONE_SLAB);
        let bottom_slab = vanilla_blocks::SMOOTH_STONE_SLAB
            .default_state()
            .set_value(SLAB_TYPE, SlabType::Bottom)
            .set_value(WATERLOGGED, false);

        assert!(behavior.can_place_liquid(bottom_slab, &vanilla_fluids::WATER));
    }

    #[test]
    fn single_slabs_reject_flowing_water_for_container_admission() {
        init_vanilla_registry();
        let behavior = SlabBlock::new(&vanilla_blocks::SMOOTH_STONE_SLAB);
        let bottom_slab = vanilla_blocks::SMOOTH_STONE_SLAB
            .default_state()
            .set_value(SLAB_TYPE, SlabType::Bottom)
            .set_value(WATERLOGGED, false);

        assert!(!behavior.can_place_liquid(bottom_slab, &vanilla_fluids::FLOWING_WATER));
    }
}
