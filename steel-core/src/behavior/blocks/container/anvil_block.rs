use std::sync::Arc;

use steel_macros::block_behavior;
use steel_registry::{
    blocks::{
        BlockRef,
        block_state_ext::BlockStateExt,
        properties::{BlockStateProperties, EnumProperty},
    },
    items::item::BlockHitResult,
    level_events, vanilla_blocks, vanilla_custom_stats, vanilla_damage_types,
};
use steel_utils::{BlockPos, BlockStateId, Direction, translations};
use text_components::TextComponent;

use crate::{
    behavior::blocks::FallingBlock,
    behavior::{BlockBehavior, BlockPlaceContext, Fallable, InteractionResult, InventoryAccess},
    entity::Entity as _,
    entity::damage::DamageSource,
    entity::entities::FallingBlockEntity,
    inventory::menu::kinds::anvil,
    player::Player,
    world::{ScheduledTickAccess, World},
};

/// Vanilla `AnvilBlock`: a falling block that opens the repair menu.
#[block_behavior]
pub struct AnvilBlock {
    block: BlockRef,
    falling: FallingBlock,
}

const FACING: &EnumProperty<Direction> = &BlockStateProperties::FACING;
const HORIZONTAL_FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;

impl AnvilBlock {
    /// Creates a new Anvil Block Behavior
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self {
            block,
            falling: FallingBlock::new(block),
        }
    }

    /// Returns the next damage level block state for anvil `BlockStates`
    #[must_use]
    pub fn damage(state: BlockStateId) -> Option<BlockStateId> {
        let block = state.get_block();
        if block == &vanilla_blocks::ANVIL {
            Some(
                vanilla_blocks::CHIPPED_ANVIL
                    .default_state()
                    .copy_value(FACING, &state),
            )
        } else if block == &vanilla_blocks::CHIPPED_ANVIL {
            Some(
                vanilla_blocks::DAMAGED_ANVIL
                    .default_state()
                    .copy_value(FACING, &state),
            )
        } else {
            None
        }
    }
}

impl Fallable for AnvilBlock {
    /// Vanilla `AnvilBlock.onLand`.
    fn on_land(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        _state: BlockStateId,
        _replaced_state: BlockStateId,
        entity: &FallingBlockEntity,
    ) {
        if !entity.is_silent() {
            world.level_event(level_events::SOUND_ANVIL_LAND, pos, 0, None);
        }
    }

    /// Vanilla `AnvilBlock.onBrokenAfterFall`.
    fn on_broken_after_fall(&self, world: &Arc<World>, pos: BlockPos, entity: &FallingBlockEntity) {
        if !entity.is_silent() {
            world.level_event(level_events::SOUND_ANVIL_BROKEN, pos, 0, None);
        }
    }

    /// Vanilla `AnvilBlock.getFallDamageSource`: `DamageSources.anvil`.
    fn get_fall_damage_source(&self, entity: &Arc<FallingBlockEntity>) -> DamageSource {
        DamageSource::direct(
            &vanilla_damage_types::FALLING_ANVIL,
            Arc::<FallingBlockEntity>::clone(entity),
        )
    }
}

impl BlockBehavior for AnvilBlock {
    fn on_place(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _old_state: BlockStateId,
        _moved_by_piston: bool,
    ) {
        self.falling.on_place(world, pos);
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        ticks: &dyn ScheduledTickAccess,
        pos: BlockPos,
        _direction: Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        self.falling.update_shape(state, ticks, pos)
    }

    /// Vanilla `FallingBlock.tick` with `AnvilBlock.falling`: the anvil hurts what it lands on.
    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if let Some(entity) = FallingBlock::tick(state, world, pos) {
            entity.set_hurts_entities(2.0, 40);
        }
    }

    fn as_fallable(&self) -> Option<&dyn Fallable> {
        Some(self)
    }

    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state().set_value(
            HORIZONTAL_FACING,
            context.horizontal_direction().rotate_y_clockwise(),
        ))
    }

    fn open_menu_provider(
        &self,
        _state: BlockStateId,
        _world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
    ) -> bool {
        let inventory = Arc::clone(&player.inventory);
        player.open_menu(
            TextComponent::translated(translations::CONTAINER_REPAIR.msg()),
            move |context| anvil(inventory, context.container_id, pos, context.world),
        );
        true
    }

    fn use_without_item(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        self.open_menu_provider(state, world, pos, player);
        player.award_custom_stat(&vanilla_custom_stats::INTERACT_WITH_ANVIL);
        InteractionResult::Success
    }
}
