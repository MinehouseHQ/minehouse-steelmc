//! Block item behavior implementation.

use steel_macros::item_behavior;
use steel_registry::{
    REGISTRY,
    block_entity_type::BlockEntityTypeRef,
    blocks::{BlockRef, block_state_ext::BlockStateExt, shapes::OffsetVoxelShape},
    data_components::vanilla_components::{BLOCK_ENTITY_DATA, BLOCK_STATE},
    sound_event::SoundEventRef,
    vanilla_block_entity_types, vanilla_blocks, vanilla_game_events,
};
use steel_utils::{BlockPos, BlockStateId, types::UpdateFlags};

use crate::behavior::items::bundle::on_container_destroyed;
use crate::behavior::{BLOCK_BEHAVIORS, BlockCollisionContext, ItemBehavior};
use crate::block_entity::load_item_block_entity_data;
use crate::entity::Entity;
use crate::entity::entities::ItemEntity;
use crate::fluid::{FluidStateExt as _, get_fluid_state};
use crate::player::advancement::criteria::PLACED_BLOCK;
use crate::world::World;
use crate::world::game_event::GameEventContext;
use crate::{
    behavior::context::{BlockPlaceContext, InteractionResult, UseOnContext},
    player::Player,
};
use std::ptr;
use std::sync::Arc;
use steel_registry::ItemStackTemplate;
use steel_registry::data_components::components::ItemContainerContents;
use steel_registry::data_components::vanilla_components::CONTAINER;
use steel_registry::item_stack::ItemStack;

pub(super) enum SurvivalCheck {
    Required,
    Skipped,
}

/// Behavior for items that place blocks.
#[item_behavior]
pub struct BlockItem {
    /// The block this item places.
    #[json_arg(vanilla_blocks, json = "block")]
    pub block: BlockRef,
}

impl BlockItem {
    const PLACE_BLOCK_FLAGS: UpdateFlags = UpdateFlags::UPDATE_ALL_IMMEDIATE;

    /// Creates a new block item behavior for the given block.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    pub(super) fn place_with(
        &self,
        context: BlockPlaceContext<'_>,
        place_block: impl FnOnce(&BlockPlaceContext<'_>, BlockStateId) -> bool,
    ) -> InteractionResult {
        self.place_with_policy(
            context,
            Some,
            SurvivalCheck::Required,
            place_block,
            self.block.config.sound_type.place_sound,
            |place_context| self.get_state_for_placement(place_context),
        )
    }

    pub(super) fn place_with_sound_and_block(
        &self,
        context: BlockPlaceContext<'_>,
        place_block: impl FnOnce(&BlockPlaceContext<'_>, BlockStateId) -> bool,
        place_sound: SoundEventRef,
    ) -> InteractionResult {
        self.place_with_policy(
            context,
            Some,
            SurvivalCheck::Required,
            place_block,
            place_sound,
            |place_context| self.get_state_for_placement(place_context),
        )
    }

    #[expect(
        clippy::manual_midpoint,
        reason = "Matches vanilla BlockItem::place's sound volume formula"
    )]
    pub(super) fn place_with_policy<'a>(
        &self,
        context: BlockPlaceContext<'a>,
        update_context: impl FnOnce(BlockPlaceContext<'a>) -> Option<BlockPlaceContext<'a>>,
        survival_check: SurvivalCheck,
        place_block: impl FnOnce(&BlockPlaceContext<'a>, BlockStateId) -> bool,
        place_sound: SoundEventRef,
        get_state_for_placement: impl FnOnce(&BlockPlaceContext<'a>) -> Option<BlockStateId>,
    ) -> InteractionResult {
        if !context.can_place() {
            return InteractionResult::Fail;
        }
        let Some(mut context) = update_context(context) else {
            return InteractionResult::Fail;
        };
        let place_pos = context.place_pos();

        let Some(new_state) = get_state_for_placement(&context) else {
            return InteractionResult::Fail;
        };

        let behavior = BLOCK_BEHAVIORS.get_behavior(new_state.get_block());
        if matches!(survival_check, SurvivalCheck::Required)
            && !behavior.can_survive(new_state, context.world.as_ref(), place_pos)
        {
            return InteractionResult::Fail;
        }

        let collision_context = context.player().map_or_else(
            BlockCollisionContext::placement_without_entity,
            |player| {
                BlockCollisionContext::with_position(player.position().y, player.is_descending())
            },
        );
        let collision_shape = OffsetVoxelShape::new(
            behavior.get_collision_shape(
                new_state,
                context.world.as_ref(),
                place_pos,
                collision_context,
            ),
            behavior.get_collision_shape_offset(
                new_state,
                context.world.as_ref(),
                place_pos,
                collision_context,
            ),
        );
        if !context.world.is_unobstructed(collision_shape, place_pos) {
            return InteractionResult::Fail;
        }

        let replaced_state = context.world.get_block_state(place_pos);
        if !place_block(&context, new_state) {
            return InteractionResult::Fail;
        }

        let placed_state = context.world.get_block_state(place_pos);
        if let Some(player) = context.player()
            && let Some(world_log) = player.world_log()
        {
            world_log.block_placed(
                player,
                context.world,
                place_pos,
                replaced_state,
                placed_state,
            );
        }
        if placed_state.get_block() == new_state.get_block() {
            let placed_state = Self::update_block_state_from_tag(&context, place_pos, placed_state);
            update_custom_block_entity_tag(context.world, context.player(), place_pos, &context);
            Self::update_block_entity_components(&context, place_pos);
            let placed_behavior = BLOCK_BEHAVIORS.get_behavior(placed_state.get_block());
            placed_behavior.set_placed_by(placed_state, context.world, place_pos, context.source());
            if let Some(player) = context.player() {
                let tool = context.with_item(ItemStack::clone);
                player.trigger_item_used_on_location(&PLACED_BLOCK, place_pos, &tool);
            }
        }

        // Play place sound (exclude the placing player, they hear it client-side)
        context.world.play_block_sound(
            place_sound,
            place_pos,
            (self.block.config.sound_type.volume + 1.0) / 2.0,
            self.block.config.sound_type.pitch * 0.8,
            context.player().map(Entity::id),
        );
        context.world.game_event(
            &vanilla_game_events::BLOCK_PLACE,
            place_pos,
            &GameEventContext::new(
                context.player().map(|player| player as &dyn Entity),
                Some(placed_state),
            ),
        );

        let has_infinite_materials = context.player().is_some_and(Player::has_infinite_materials);
        context.with_item_mut(|item| item.consume_one(has_infinite_materials));

        InteractionResult::Success
    }

    /// Places this block using an already constructed placement context.
    pub fn place(&self, context: BlockPlaceContext<'_>) -> InteractionResult {
        self.place_with(context, Self::place_block)
    }

    /// Vanilla `updateBlockStateFromTag`: the stack's block state component
    /// sets the properties it names on the placed block.
    fn update_block_state_from_tag(
        context: &BlockPlaceContext<'_>,
        pos: BlockPos,
        placed_state: BlockStateId,
    ) -> BlockStateId {
        let modified = context.with_item(|stack| {
            stack.get(BLOCK_STATE).map_or(placed_state, |properties| {
                properties
                    .properties()
                    .iter()
                    .fold(placed_state, |state, (name, value)| {
                        REGISTRY
                            .blocks
                            .try_set_property_by_name(state, name, value)
                            .unwrap_or(state)
                    })
            })
        });
        if modified != placed_state {
            context
                .world
                .set_block(pos, modified, UpdateFlags::UPDATE_CLIENTS);
        }
        modified
    }

    fn update_block_entity_components(context: &BlockPlaceContext<'_>, pos: BlockPos) {
        let Some(block_entity) = context.world.get_block_entity(pos) else {
            return;
        };
        context
            .source()
            .with_item(|stack| block_entity.apply_components_from_item_stack(stack));
        block_entity.set_changed();
    }

    pub(super) fn place_block(context: &BlockPlaceContext<'_>, state: BlockStateId) -> bool {
        context
            .world
            .set_block(context.place_pos(), state, Self::PLACE_BLOCK_FLAGS)
    }

    pub(super) fn get_state_for_placement(
        &self,
        context: &BlockPlaceContext<'_>,
    ) -> Option<BlockStateId> {
        let behavior = BLOCK_BEHAVIORS.get_behavior(self.block);
        behavior.get_state_for_placement(context)
    }
}

impl ItemBehavior for BlockItem {
    fn as_block_item(&self) -> Option<&BlockItem> {
        Some(self)
    }

    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        self.place(context.build_place_context())
    }

    fn can_fit_inside_container_items(&self) -> bool {
        BLOCK_BEHAVIORS
            .get_behavior(self.block)
            .fits_inside_container_items()
    }

    /// Vanilla `BlockItem.onDestroyed`: a destroyed shulker box spills its items.
    fn on_destroyed(&self, entity: &ItemEntity, world: &Arc<World>) {
        let mut item = entity.get_item();
        let Some(container) = item.get(CONTAINER).cloned() else {
            return;
        };
        item.set(CONTAINER, ItemContainerContents::empty());
        entity.set_item(item);
        on_container_destroyed(
            entity,
            world,
            container
                .items()
                .iter()
                .flatten()
                .map(ItemStackTemplate::create)
                .filter(|stack| !stack.is_empty()),
        );
    }
}

/// Behavior for double-high block items (doors, tall flowers, etc.).
///
/// Vanilla's `DoubleHighBlockItem` extends `BlockItem` and overrides `placeBlock`
/// to place the upper half block above the lower half.
///
/// The `_block` field is read by the build script via `#[json_arg]` to generate constructor
/// calls from `classes.json`. The actual value is forwarded into `base`.
#[item_behavior]
pub struct DoubleHighBlockItem {
    #[json_arg(vanilla_blocks, json = "block")]
    _block: BlockRef,
    base: BlockItem,
}

impl DoubleHighBlockItem {
    const PREPARE_UPPER_FLAGS: UpdateFlags =
        UpdateFlags::UPDATE_ALL_IMMEDIATE.union(UpdateFlags::UPDATE_KNOWN_SHAPE);

    /// Creates a new double-high block item behavior for the given block.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self {
            _block: block,
            base: BlockItem::new(block),
        }
    }

    fn place_block(context: &BlockPlaceContext<'_>, state: BlockStateId) -> bool {
        let above = context.place_pos().above();
        let above_state = if get_fluid_state(context.world, above).is_water() {
            vanilla_blocks::WATER.default_state()
        } else {
            vanilla_blocks::AIR.default_state()
        };
        let _ = context
            .world
            .set_block(above, above_state, Self::PREPARE_UPPER_FLAGS);

        BlockItem::place_block(context, state)
    }
}

impl ItemBehavior for DoubleHighBlockItem {
    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        self.base
            .place_with(context.build_place_context(), Self::place_block)
    }
}

/// Vanilla `GameMasterBlockItem`: a block item, such as a command block, that
/// only players who can use game master blocks may place.
#[item_behavior]
pub struct GameMasterBlockItem {
    #[json_arg(vanilla_blocks, json = "block")]
    _block: BlockRef,
    base: BlockItem,
}

impl GameMasterBlockItem {
    /// Creates a game master block item behavior for the given block.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self {
            _block: block,
            base: BlockItem::new(block),
        }
    }
}

impl ItemBehavior for GameMasterBlockItem {
    fn as_block_item(&self) -> Option<&BlockItem> {
        Some(&self.base)
    }

    /// Vanilla `BlockItem.useOn` with `GameMasterBlockItem.getPlacementState`.
    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        self.base.place_with_policy(
            context.build_place_context(),
            Some,
            SurvivalCheck::Required,
            BlockItem::place_block,
            self.base.block.config.sound_type.place_sound,
            |place_context| {
                if place_context
                    .player()
                    .is_some_and(|player| !player.can_use_game_master_blocks())
                {
                    None
                } else {
                    self.base.get_state_for_placement(place_context)
                }
            },
        )
    }

    fn can_fit_inside_container_items(&self) -> bool {
        self.base.can_fit_inside_container_items()
    }
}

/// Vanilla `BlockEntityType.OP_ONLY_CUSTOM_DATA`: block entities whose item
/// data only game masters may place.
const OP_ONLY_CUSTOM_DATA: [BlockEntityTypeRef; 6] = [
    &vanilla_block_entity_types::COMMAND_BLOCK,
    &vanilla_block_entity_types::LECTERN,
    &vanilla_block_entity_types::SIGN,
    &vanilla_block_entity_types::HANGING_SIGN,
    &vanilla_block_entity_types::MOB_SPAWNER,
    &vanilla_block_entity_types::TRIAL_SPAWNER,
];

/// Vanilla `BlockItem.updateCustomBlockEntityTag`: merges the stack's block
/// entity data into the placed block entity of the same type. Returns
/// whether it changed anything.
pub(crate) fn update_custom_block_entity_tag(
    world: &World,
    player: Option<&Player>,
    pos: BlockPos,
    context: &BlockPlaceContext<'_>,
) -> bool {
    let Some(data) = context.with_item(|stack| stack.get(BLOCK_ENTITY_DATA).cloned()) else {
        return false;
    };
    let Some(block_entity) = world.get_block_entity(pos) else {
        return false;
    };
    let block_entity_type = block_entity.get_type();
    if !ptr::eq(block_entity_type, data.block_entity_type()) {
        return false;
    }
    let op_only = OP_ONLY_CUSTOM_DATA
        .iter()
        .any(|op_only| ptr::eq(*op_only, block_entity_type));
    if op_only && !player.is_some_and(Player::can_use_game_master_blocks) {
        return false;
    }
    load_item_block_entity_data(block_entity.as_ref(), &data.data().copy_tag())
}
