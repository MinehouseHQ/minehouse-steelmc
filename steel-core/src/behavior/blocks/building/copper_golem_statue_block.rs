//! Vanilla `CopperGolemStatueBlock` and `WeatheringCopperGolemStatueBlock`:
//! the copper statue an oxidized copper golem turns into. Using it changes
//! the pose, scraping an unaffected one with an axe brings the golem back.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty, Pose,
};
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    sound_events, vanilla_block_entity_types, vanilla_game_events, vanilla_items,
};
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId, Downcast as _};

use crate::behavior::block::{BlockEntityCreation, schedule_water_tick_if_waterlogged};
use crate::behavior::blocks::{WeatherState, WeatheringCopper};
use crate::behavior::context::{BlockHitResult, BlockPlaceContext, InteractionResult};
use crate::behavior::{BlockBehavior, InventoryAccess};
use crate::block_entity::entities::CopperGolemStatueBlockEntity;
use crate::block_entity::{BLOCK_ENTITIES, SharedBlockEntity};
use crate::entity::ai::path::PathComputationType;
use crate::entity::entities::CopperGolemEntity;
use crate::player::Player;
use crate::world::ScheduledTickAccess;
use crate::world::World;
use crate::world::game_event::GameEventContext;

const FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;
const POSE: &EnumProperty<Pose> = &BlockStateProperties::COPPER_GOLEM_POSE;
const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;

/// Vanilla `Pose.getNextPose`: the poses in order, `star` back to
/// `standing`.
#[must_use]
pub const fn next_pose(pose: Pose) -> Pose {
    match pose {
        Pose::Standing => Pose::Sitting,
        Pose::Sitting => Pose::Running,
        Pose::Running => Pose::Star,
        Pose::Star => Pose::Standing,
    }
}

/// Vanilla `Pose.ordinal`.
const fn pose_ordinal(pose: Pose) -> i32 {
    match pose {
        Pose::Standing => 0,
        Pose::Sitting => 1,
        Pose::Running => 2,
        Pose::Star => 3,
    }
}

/// Vanilla `CopperGolemStatueBlock`, the waxed statues.
#[block_behavior]
pub struct CopperGolemStatueBlock {
    block: BlockRef,
}

impl CopperGolemStatueBlock {
    /// Creates a waxed statue block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

/// Vanilla `CopperGolemStatueBlock.updatePose`.
fn update_pose(world: &Arc<World>, state: BlockStateId, pos: BlockPos, player: &Player) {
    world.play_block_sound(
        &sound_events::ENTITY_COPPER_GOLEM_BECOME_STATUE,
        pos,
        1.0,
        1.0,
        None,
    );
    world.set_block(
        pos,
        state.set_value(POSE, next_pose(state.get_value(POSE))),
        UpdateFlags::UPDATE_ALL,
    );
    world.game_event(
        &vanilla_game_events::BLOCK_CHANGE,
        pos,
        &GameEventContext::new(Some(player), None),
    );
}

/// The statue block's shared vanilla behavior.
macro_rules! statue_block_behavior {
    () => {
        fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
            Some(
                self.block
                    .default_state()
                    .set_value(FACING, context.horizontal_direction().opposite())
                    .set_value(WATERLOGGED, context.is_water_source()),
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

        fn is_pathfindable(
            &self,
            state: BlockStateId,
            computation_type: PathComputationType,
        ) -> bool {
            computation_type == PathComputationType::Water && state.get_value(WATERLOGGED)
        }

        fn new_block_entity(
            &self,
            level: Weak<World>,
            pos: BlockPos,
            state: BlockStateId,
        ) -> BlockEntityCreation {
            BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
                &vanilla_block_entity_types::COPPER_GOLEM_STATUE,
                level,
                pos,
                state,
            ))
        }

        fn has_analog_output_signal(&self, _state: BlockStateId) -> bool {
            true
        }

        fn get_analog_output_signal(
            &self,
            state: BlockStateId,
            _world: &World,
            _pos: BlockPos,
            _direction: steel_utils::Direction,
        ) -> i32 {
            pose_ordinal(state.get_value(POSE)) + 1
        }

        fn affect_neighbors_after_removal(
            &self,
            state: BlockStateId,
            world: &Arc<World>,
            pos: BlockPos,
            _moved_by_piston: bool,
        ) {
            world.update_neighbor_for_output_signal(pos, state.get_block());
        }

        fn get_clone_item_stack(
            &self,
            block: BlockRef,
            state: BlockStateId,
            block_entity: Option<SharedBlockEntity>,
            _include_data: bool,
        ) -> Option<steel_registry::item_stack::ItemStack> {
            Some(
                block_entity
                    .as_ref()
                    .and_then(|entity| entity.downcast_ref::<CopperGolemStatueBlockEntity>())
                    .map_or_else(
                        || {
                            steel_registry::item_stack::ItemStack::new(
                                steel_registry::REGISTRY.items.by_block(block),
                            )
                        },
                        |statue| statue.item(state),
                    ),
            )
        }
    };
}

impl BlockBehavior for CopperGolemStatueBlock {
    statue_block_behavior!();

    fn use_item_on(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hand: InteractionHand,
        _hit_result: &BlockHitResult,
        inv: &mut InventoryAccess,
    ) -> InteractionResult {
        if inv.with_item(|item| item.item().has_tag(&ItemTag::AXES)) {
            return InteractionResult::Pass;
        }
        update_pose(world, state, pos, player);
        InteractionResult::Success
    }
}

/// Vanilla `WeatheringCopperGolemStatueBlock`, the statues that oxidize.
#[block_behavior]
pub struct WeatheringCopperGolemStatueBlock {
    block: BlockRef,
    #[json_arg(r#enum = "WeatherState", json = "weathering_state")]
    weathering: WeatheringCopper,
    weather_state: WeatherState,
}

impl WeatheringCopperGolemStatueBlock {
    /// Creates a weathering statue block behavior.
    #[must_use]
    pub const fn new(block: BlockRef, weather_state: WeatherState) -> Self {
        Self {
            block,
            weathering: WeatheringCopper::new(weather_state),
            weather_state,
        }
    }
}

impl BlockBehavior for WeatheringCopperGolemStatueBlock {
    statue_block_behavior!();

    fn random_tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        self.weathering.change_over_time(state, world, pos);
    }

    fn use_item_on(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hand: InteractionHand,
        _hit_result: &BlockHitResult,
        inv: &mut InventoryAccess,
    ) -> InteractionResult {
        let Some(block_entity) = world.get_block_entity(pos) else {
            return InteractionResult::Pass;
        };
        let Some(statue_entity) = block_entity.downcast_ref::<CopperGolemStatueBlockEntity>()
        else {
            return InteractionResult::Pass;
        };
        if !inv.with_item(|item| item.item().has_tag(&ItemTag::AXES)) {
            if inv.with_item(|item| item.is(&vanilla_items::HONEYCOMB)) {
                return InteractionResult::Pass;
            }
            update_pose(world, state, pos, player);
            return InteractionResult::Success;
        }
        if self.weather_state == WeatherState::Unaffected {
            let golem =
                CopperGolemEntity::from_statue(world, pos, state, statue_entity.golem_name());
            let has_infinite_materials = player.has_infinite_materials();
            let change =
                inv.with_item(|item| item.hurt_and_break_tracked(1, has_infinite_materials).1);
            player.trigger_item_durability_changed(change);
            if let Err(error) = world.try_add_entity(golem) {
                log::debug!("world rejected a revived copper golem: {error}");
            }
            world.remove_block(pos, false);
            return InteractionResult::Success;
        }
        InteractionResult::Pass
    }
}
