//! Vanilla `CommandBlock`: impulse, repeating and chain command blocks.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_protocol::packets::game::CBlockEntityData;
use steel_registry::RegistryEntry as _;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction};
use steel_registry::data_components::vanilla_components::BLOCK_ENTITY_DATA;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_game_rules::{MAX_COMMAND_SEQUENCE_LENGTH, SEND_COMMAND_FEEDBACK};
use steel_utils::serial::OptionalNbt;
use steel_utils::{BlockPos, BlockStateId, Downcast as _};

use crate::behavior::InventoryAccess;
use crate::behavior::block::{BlockBehavior, BlockEntityCreation};
use crate::behavior::context::{
    BlockHitResult, BlockPlaceContext, InteractionResult, PlacementSource,
};
use crate::block_entity::BlockEntity as _;
use crate::block_entity::entities::{CommandBlockEntity, CommandBlockMode, is_command_block};
use crate::command::command_block::CommandBlockRunner;
use crate::player::Player;
use crate::world::{CommandBlockRun, SignalGetter as _, World};

/// Vanilla `CommandBlock`.
#[block_behavior]
pub struct CommandBlock {
    block: BlockRef,
    /// Whether a newly placed block is always active: only chain command blocks are.
    #[json_arg(value)]
    automatic: bool,
}

impl CommandBlock {
    /// Creates the behavior for one of the three command blocks.
    #[must_use]
    pub const fn new(block: BlockRef, automatic: bool) -> Self {
        Self { block, automatic }
    }

    /// Vanilla `setPoweredAndUpdate`: a rising signal runs an impulse or
    /// repeating block that is not always active on the next tick.
    fn set_powered_and_update(
        &self,
        world: &World,
        pos: BlockPos,
        entity: &CommandBlockEntity,
        is_powered: bool,
    ) {
        if is_powered == entity.is_powered() {
            return;
        }
        entity.set_powered(is_powered);
        if is_powered {
            if entity.is_automatic() || entity.mode() == CommandBlockMode::Sequence {
                return;
            }
            entity.mark_condition_met(world);
            world.schedule_block_tick_default(pos, self.block, 1);
        }
    }
}

impl BlockBehavior for CommandBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state().set_value(
            &BlockStateProperties::FACING,
            context.get_nearest_looking_direction().opposite(),
        ))
    }

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
        if let Some(entity) = block_entity.downcast_ref::<CommandBlockEntity>() {
            self.set_powered_and_update(world, pos, entity, world.has_neighbor_signal(pos));
        }
    }

    /// Vanilla `CommandBlock.tick`. Commands can reach any world of the
    /// domain, so the server runs the tick once every world has ticked; see
    /// [`run_command_block_tick`].
    fn tick(&self, _state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        world.queue_command_block_run(CommandBlockRun::Block(pos));
    }

    /// Vanilla `useWithoutItem`: game masters get the block's data, which opens
    /// the command block screen.
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
        if block_entity.downcast_ref::<CommandBlockEntity>().is_none()
            || !player.can_use_game_master_blocks()
        {
            return InteractionResult::Pass;
        }
        // Vanilla `ServerPlayer.openCommandBlock`.
        player.send_packet(CBlockEntityData {
            pos,
            block_entity_type: block_entity.get_type().id() as i32,
            nbt: OptionalNbt(Some(block_entity.save_custom_only())),
        });
        InteractionResult::Success
    }

    fn has_analog_output_signal(&self, _state: BlockStateId) -> bool {
        true
    }

    fn get_analog_output_signal(
        &self,
        _state: BlockStateId,
        world: &World,
        pos: BlockPos,
        _direction: Direction,
    ) -> i32 {
        world.get_block_entity(pos).map_or(0, |block_entity| {
            block_entity
                .downcast_ref::<CommandBlockEntity>()
                .map_or(0, |entity| entity.command_block().success_count())
        })
    }

    /// Vanilla `setPlacedBy`: a fresh block tracks output when command
    /// feedback is on, then picks up the redstone signal it was placed into.
    fn set_placed_by(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        source: &PlacementSource<'_>,
    ) {
        let Some(block_entity) = world.get_block_entity(pos) else {
            return;
        };
        let Some(entity) = block_entity.downcast_ref::<CommandBlockEntity>() else {
            return;
        };
        if !source.with_item(|stack| stack.has(BLOCK_ENTITY_DATA)) {
            entity
                .command_block()
                .set_track_output(world.get_game_rule(&SEND_COMMAND_FEEDBACK));
            entity.set_automatic(world, self.automatic);
        }
        self.set_powered_and_update(world, pos, entity, world.has_neighbor_signal(pos));
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        let entity = CommandBlockEntity::new(level, pos, state);
        entity.load_automatic(self.automatic);
        BlockEntityCreation::from_registered_factory(Some(Arc::new(entity)))
    }

    fn is_game_master_block(&self) -> bool {
        true
    }
}

/// Runs the command block tick queued at `pos` on the server thread: vanilla
/// `CommandBlock.tick` with its command execution and chain.
pub(crate) fn run_command_block_tick(
    world: &Arc<World>,
    pos: BlockPos,
    runner: &mut CommandBlockRunner<'_>,
) {
    let state = world.get_block_state(pos);
    if !is_command_block(state) {
        return;
    }
    let Some(block_entity) = world.get_block_entity(pos) else {
        return;
    };
    let Some(entity) = block_entity.downcast_ref::<CommandBlockEntity>() else {
        return;
    };
    let base = entity.command_block();
    let command_set = !base.command().is_empty();
    let mode = entity.mode();
    let was_condition_met = entity.was_condition_met();
    match mode {
        CommandBlockMode::Auto => {
            entity.mark_condition_met(world);
            if was_condition_met {
                execute(state, world, entity, command_set, runner);
            } else if entity.is_conditional(world) {
                base.set_success_count(0);
            }
            if entity.is_powered() || entity.is_automatic() {
                world.schedule_block_tick_default(pos, state.get_block(), 1);
            }
        }
        CommandBlockMode::Redstone => {
            if was_condition_met {
                execute(state, world, entity, command_set, runner);
            } else if entity.is_conditional(world) {
                base.set_success_count(0);
            }
        }
        CommandBlockMode::Sequence => {}
    }
    world.update_neighbor_for_output_signal(pos, state.get_block());
}

/// Vanilla `CommandBlock.execute`.
fn execute(
    state: BlockStateId,
    world: &Arc<World>,
    entity: &CommandBlockEntity,
    command_set: bool,
    runner: &mut CommandBlockRunner<'_>,
) {
    if command_set {
        entity
            .command_block()
            .perform_command(runner, world, entity.origin());
    } else {
        entity.command_block().set_success_count(0);
    }
    execute_chain(
        world,
        entity.get_block_pos(),
        state.get_value(&BlockStateProperties::FACING),
        runner,
    );
}

/// Vanilla `CommandBlock.executeChain`: runs the chain command blocks that
/// follow, up to `maxCommandChainLength` of them.
fn execute_chain(
    world: &Arc<World>,
    start: BlockPos,
    mut direction: Direction,
    runner: &mut CommandBlockRunner<'_>,
) {
    let mut pos = start;
    let mut max_iterations = world.get_game_rule(&MAX_COMMAND_SEQUENCE_LENGTH);
    loop {
        // Vanilla `while (maxIterations-- > 0)`.
        let remaining = max_iterations;
        max_iterations -= 1;
        if remaining <= 0 {
            break;
        }
        pos = pos.relative(direction);
        let state = world.get_block_state(pos);
        let block = state.get_block();
        if block != &vanilla_blocks::CHAIN_COMMAND_BLOCK {
            break;
        }
        let Some(block_entity) = world.get_block_entity(pos) else {
            break;
        };
        let Some(entity) = block_entity.downcast_ref::<CommandBlockEntity>() else {
            break;
        };
        if entity.mode() != CommandBlockMode::Sequence {
            break;
        }
        if entity.is_powered() || entity.is_automatic() {
            let base = entity.command_block();
            if entity.mark_condition_met(world) {
                if !base.perform_command(runner, world, entity.origin()) {
                    break;
                }
                world.update_neighbor_for_output_signal(pos, block);
            } else if entity.is_conditional(world) {
                base.set_success_count(0);
            }
        }
        direction = state.get_value(&BlockStateProperties::FACING);
    }
    if max_iterations <= 0 {
        let limit = world.get_game_rule(&MAX_COMMAND_SEQUENCE_LENGTH).max(0);
        log::warn!("Command Block chain tried to execute more than {limit} steps!");
    }
}
