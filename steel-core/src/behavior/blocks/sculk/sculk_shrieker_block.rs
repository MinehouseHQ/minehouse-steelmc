//! Vanilla `SculkShriekerBlock`: shrieks at players who set it off and calls
//! the warden.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, BoolProperty, Direction};
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_entity_types;
use steel_utils::types::UpdateFlags;
use steel_utils::value_providers::IntProvider;
use steel_utils::{BlockPos, BlockStateId, Downcast as _};

use crate::behavior::block::{
    BlockBehavior, BlockEntityCreation, schedule_water_tick_if_waterlogged, try_drop_experience,
};
use crate::behavior::context::BlockPlaceContext;
use crate::block_entity::entities::{SculkShriekerBlockEntity, try_get_player};
use crate::block_entity::{BLOCK_ENTITIES, BlockEntityTicker};
use crate::entity::Entity;
use crate::world::{ScheduledTickAccess, World};

const SHRIEKING: &BoolProperty = &BlockStateProperties::SHRIEKING;
const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;

/// Vanilla `SculkShriekerBlock.spawnAfterBreak` experience.
const EXPERIENCE: i32 = 5;

/// Vanilla `SculkShriekerBlock`.
#[block_behavior]
pub struct SculkShriekerBlock {
    block: BlockRef,
}

impl SculkShriekerBlock {
    /// Creates sculk shrieker behavior for `block`.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

impl BlockBehavior for SculkShriekerBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
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

    /// Vanilla `stepOn`: a player stepping on it, or riding over it, sets it
    /// off.
    fn step_on(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos, entity: &dyn Entity) {
        if let Some(player) = try_get_player(world, Some(entity))
            && let Some(block_entity) = world.get_block_entity(pos)
            && let Some(shrieker) = block_entity.downcast_ref::<SculkShriekerBlockEntity>()
        {
            shrieker.try_shriek(world, Some(&player));
        }
        self.default_step_on(state, world, pos, entity);
    }

    /// Vanilla `tick`: the shriek ends and the warden may answer.
    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if !state.get_value(SHRIEKING) {
            return;
        }
        world.set_block(
            pos,
            state.set_value(SHRIEKING, false),
            UpdateFlags::UPDATE_ALL,
        );
        if let Some(block_entity) = world.get_block_entity(pos)
            && let Some(shrieker) = block_entity.downcast_ref::<SculkShriekerBlockEntity>()
        {
            shrieker.try_respond(world);
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
            try_drop_experience(world, pos, tool, &IntProvider::Constant(EXPERIENCE));
        }
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::SCULK_SHRIEKER,
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
            &vanilla_block_entity_types::SCULK_SHRIEKER,
        )
    }
}
