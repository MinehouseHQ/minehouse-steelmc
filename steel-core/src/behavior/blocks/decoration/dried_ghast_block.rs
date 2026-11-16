//! Vanilla `DriedGhastBlock`: a dried ghast soaks up water over three
//! stages and then hatches into a ghastling; out of water it dries again.

use std::sync::Arc;

use glam::DVec3;
use steel_macros::block_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty, IntProperty,
};
use steel_registry::fluid::FluidState;
use steel_registry::sound_event::SoundEventHolder;
use steel_registry::{sound_events, vanilla_entities, vanilla_game_events};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::block::{
    BlockBehavior, place_simple_waterlogged_liquid, schedule_water_tick_if_waterlogged,
};
use crate::behavior::context::{BlockPlaceContext, PlacementSource};
use crate::entity::ai::path::PathComputationType;
use crate::entity::create_entity_instance;
use crate::world::game_event::GameEventContext;
use crate::world::{LevelAccessor, ScheduledTickAccess, World};

/// Vanilla `DriedGhastBlock.MAX_HYDRATION_LEVEL`.
pub const MAX_HYDRATION_LEVEL: u8 = 3;
/// Vanilla `DriedGhastBlock.HYDRATION_TICK_DELAY`.
pub const HYDRATION_TICK_DELAY: i32 = 5000;

const FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;
const HYDRATION: &IntProperty = &BlockStateProperties::DRIED_GHAST_HYDRATION_LEVELS;
const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;

/// Behavior for the dried ghast block.
#[block_behavior]
pub struct DriedGhastBlock {
    block: BlockRef,
}

impl DriedGhastBlock {
    /// Creates a dried ghast block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `tickWaterlogged`: one more hydration stage, or the ghastling.
    fn tick_waterlogged(state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        let hydration = state.get_value(HYDRATION);
        if hydration == MAX_HYDRATION_LEVEL {
            Self::spawn_ghastling(world, pos, state);
            return;
        }
        world.play_block_sound(
            &sound_events::BLOCK_DRIED_GHAST_TRANSITION,
            pos,
            1.0,
            1.0,
            None,
        );
        world.set_block(
            pos,
            state.set_value(HYDRATION, hydration + 1),
            UpdateFlags::UPDATE_CLIENTS,
        );
        world.game_event(
            &vanilla_game_events::BLOCK_CHANGE,
            pos,
            &GameEventContext::new(None, Some(state)),
        );
    }

    /// Vanilla `spawnGhastling`: the block turns into a baby happy ghast
    /// facing the way the block did.
    fn spawn_ghastling(world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        world.remove_block(pos, false);
        let spawn_at = DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()),
            f64::from(pos.z()) + 0.5,
        );
        let Ok(ghastling) = create_entity_instance(world, &vanilla_entities::HAPPY_GHAST, spawn_at)
        else {
            return;
        };
        if let Some(mob) = ghastling.as_mob() {
            mob.set_baby(true);
        }
        let block_rotation = state.get_value(FACING).to_yaw();
        if let Some(living) = ghastling.as_living_entity() {
            living.set_y_head_rot(block_rotation);
        }
        ghastling.snap_to(spawn_at, block_rotation, 0.0);
        let entity_id = ghastling.id();
        let position = ghastling.position();
        if let Err(error) = world.try_add_entity(ghastling) {
            log::debug!("failed to add a ghastling at {pos:?}: {error}");
            return;
        }
        world.play_sound_from_entity(
            &SoundEventHolder::Registry(&sound_events::ENTITY_GHASTLING_SPAWN),
            SoundSource::Blocks,
            entity_id,
            position,
            1.0,
            1.0,
            None,
        );
    }
}

impl BlockBehavior for DriedGhastBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(WATERLOGGED, context.is_water_source())
                .set_value(FACING, context.horizontal_direction().opposite()),
        )
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        _direction: steel_utils::Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        schedule_water_tick_if_waterlogged(state, world, pos);
        state
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if state.get_value(WATERLOGGED) {
            Self::tick_waterlogged(state, world, pos);
            return;
        }
        let hydration = state.get_value(HYDRATION);
        if hydration > 0 {
            world.set_block(
                pos,
                state.set_value(HYDRATION, hydration - 1),
                UpdateFlags::UPDATE_CLIENTS,
            );
            world.game_event(
                &vanilla_game_events::BLOCK_CHANGE,
                pos,
                &GameEventContext::new(None, Some(state)),
            );
        }
    }

    fn random_tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if (state.get_value(WATERLOGGED) || state.get_value(HYDRATION) > 0)
            && !world.has_scheduled_block_tick(pos, self.block)
        {
            world.schedule_block_tick_default(pos, self.block, HYDRATION_TICK_DELAY);
        }
    }

    fn place_liquid(
        &self,
        level: &dyn LevelAccessor,
        pos: BlockPos,
        state: BlockStateId,
        fluid_state: FluidState,
    ) -> bool {
        let placed = place_simple_waterlogged_liquid(level, pos, state, fluid_state);
        if placed {
            level.play_block_sound(
                &sound_events::BLOCK_DRIED_GHAST_PLACE_IN_WATER,
                pos,
                1.0,
                1.0,
                None,
            );
        }
        placed
    }

    fn set_placed_by(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source: &PlacementSource<'_>,
    ) {
        let sound = if state.get_value(WATERLOGGED) {
            &sound_events::BLOCK_DRIED_GHAST_PLACE_IN_WATER
        } else {
            &sound_events::BLOCK_DRIED_GHAST_PLACE
        };
        world.play_block_sound(sound, pos, 1.0, 1.0, None);
    }

    fn is_pathfindable(
        &self,
        _state: BlockStateId,
        _computation_type: PathComputationType,
    ) -> bool {
        false
    }
}
