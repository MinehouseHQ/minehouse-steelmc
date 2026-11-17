//! Vanilla `TestBlock`: the start, log, fail and accept markers of game
//! tests.

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::REGISTRY;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, PropertyEnum as _, TestBlockMode};
use steel_registry::data_components::vanilla_components::BLOCK_STATE;
use steel_registry::data_components::vanilla_components::BlockItemStateProperties;
use steel_utils::{BlockPos, BlockStateId, Downcast as _};

use crate::behavior::block::{BlockBehavior, BlockEntityCreation};
use crate::behavior::blocks::redstone::{MAX_REDSTONE_SIGNAL, MIN_REDSTONE_SIGNAL};
use crate::behavior::{BlockHitResult, BlockPlaceContext, InteractionResult, InventoryAccess};
use crate::block_entity::SharedBlockEntity;
use crate::block_entity::entities::{TestBlockEntity, test_block_mode_by_name};
use crate::player::Player;
use crate::world::{LevelReader, SignalGetter as _, SignalQueryContext, World};
use steel_registry::item_stack::ItemStack;

const MODE_NAME: &str = "mode";

/// Vanilla `TestBlock`.
#[block_behavior]
pub struct TestBlock {
    block: BlockRef,
}

impl TestBlock {
    /// Creates the behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

/// The test block entity at `pos`, if any.
fn test_block_at(world: &World, pos: BlockPos) -> Option<SharedBlockEntity> {
    world
        .get_block_entity(pos)
        .filter(|block_entity| block_entity.downcast_ref::<TestBlockEntity>().is_some())
}

impl BlockBehavior for TestBlock {
    /// Vanilla `getStateForPlacement`: the mode named by the stack's block
    /// state component, if it names one.
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        let mode = context.with_item(|stack| {
            stack
                .get(BLOCK_STATE)
                .and_then(|properties| properties.properties().get(MODE_NAME).cloned())
        });
        let state = self.block.default_state();
        Some(match mode.as_deref().and_then(test_block_mode_by_name) {
            Some(mode) => state.set_value(&BlockStateProperties::TEST_BLOCK_MODE, mode),
            None => state,
        })
    }

    /// Vanilla `useWithoutItem`: the client opens the screen itself.
    fn use_without_item(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        if test_block_at(world, pos).is_some() && player.can_use_game_master_blocks() {
            InteractionResult::Success
        } else {
            InteractionResult::Pass
        }
    }

    /// Vanilla `tick`: resets the block.
    fn tick(&self, _state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if let Some(block_entity) = test_block_at(world, pos)
            && let Some(test) = block_entity.downcast_ref::<TestBlockEntity>()
        {
            test.reset(world);
        }
    }

    /// Vanilla `neighborChanged`: a rising signal triggers every mode but start.
    fn handle_neighbor_changed(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source_block: BlockRef,
        _moved_by_piston: bool,
    ) {
        let Some(block_entity) = test_block_at(world, pos) else {
            return;
        };
        let Some(test) = block_entity.downcast_ref::<TestBlockEntity>() else {
            return;
        };
        if test.mode() == TestBlockMode::Start {
            return;
        }
        let should_trigger = world.has_neighbor_signal(pos);
        let is_powered = test.is_powered();
        if should_trigger && !is_powered {
            test.set_powered(true);
            test.trigger(world);
        } else if !should_trigger && is_powered {
            test.set_powered(false);
        }
    }

    /// Vanilla `ownSignal`: a powered start block emits a full signal.
    fn get_own_signal(
        &self,
        state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
        _context: SignalQueryContext,
    ) -> i32 {
        if state.try_get_value(&BlockStateProperties::TEST_BLOCK_MODE) != Some(TestBlockMode::Start)
        {
            return MIN_REDSTONE_SIGNAL;
        }
        let powered = world.get_block_entity(pos).is_some_and(|block_entity| {
            block_entity
                .downcast_ref::<TestBlockEntity>()
                .is_some_and(TestBlockEntity::is_powered)
        });
        if powered {
            MAX_REDSTONE_SIGNAL
        } else {
            MIN_REDSTONE_SIGNAL
        }
    }

    /// Vanilla `getCloneItemStack`: the stack keeps the block's mode.
    fn get_clone_item_stack(
        &self,
        block: BlockRef,
        state: BlockStateId,
        _block_entity: Option<SharedBlockEntity>,
        _include_data: bool,
    ) -> Option<ItemStack> {
        let mut stack = ItemStack::new(REGISTRY.items.by_block(block));
        let mode = state
            .try_get_value(&BlockStateProperties::TEST_BLOCK_MODE)
            .unwrap_or(TestBlockMode::Start);
        let mut properties = stack
            .get(BLOCK_STATE)
            .map_or_else(BTreeMap::new, |component| component.properties().clone());
        properties.insert(MODE_NAME.to_owned(), mode.as_str().to_owned());
        stack.set(BLOCK_STATE, BlockItemStateProperties::new(properties));
        Some(stack)
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(Some(Arc::new(TestBlockEntity::new(
            level, pos, state,
        ))))
    }

    fn is_game_master_block(&self) -> bool {
        true
    }
}
