//! Solid bucket item behavior implementation.

use std::sync::Arc;

use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::{
    blocks::BlockRef, item_stack::ItemStack, sound_event::SoundEventRef, vanilla_game_events,
    vanilla_items,
};
use steel_utils::BlockPos;
use steel_utils::types::UpdateFlags;

use crate::behavior::items::BlockItem;
use crate::behavior::{InteractionResult, ItemBehavior, UseOnContext};
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Behavior for buckets that place a solid block, such as powder snow.
#[item_behavior]
pub struct SolidBucketItem {
    #[json_arg(vanilla_blocks, json = "block")]
    block: BlockRef,
    #[json_arg(sound_events, json = "place_sound")]
    place_sound: SoundEventRef,
    base: BlockItem,
}

impl SolidBucketItem {
    /// Creates a solid bucket item behavior.
    #[must_use]
    pub const fn new(block: BlockRef, place_sound: SoundEventRef) -> Self {
        Self {
            block,
            place_sound,
            base: BlockItem::new(block),
        }
    }

    /// Vanilla `SolidBucketItem.emptyContents` without a user: places the
    /// block at `pos` when it is in bounds and empty.
    pub(crate) fn empty_contents(&self, world: &Arc<World>, pos: BlockPos) -> bool {
        if !world.is_in_world_bounds(pos) || !world.get_block_state(pos).is_air() {
            return false;
        }
        world.set_block(pos, self.block.default_state(), UpdateFlags::UPDATE_ALL);
        world.game_event(
            &vanilla_game_events::FLUID_PLACE,
            pos,
            &GameEventContext::new(None, None),
        );
        world.play_sound(self.place_sound, SoundSource::Blocks, pos, 1.0, 1.0, None);
        true
    }
}

impl ItemBehavior for SolidBucketItem {
    fn as_solid_bucket_item(&self) -> Option<&SolidBucketItem> {
        Some(self)
    }

    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        let result = self.base.place_with_sound_and_block(
            context.build_place_context(),
            BlockItem::place_block,
            self.place_sound,
        );

        if matches!(result, InteractionResult::Success) && !context.player.has_infinite_materials()
        {
            context
                .inv
                .with_item(|item| *item = ItemStack::new(&vanilla_items::BUCKET));
        }

        result
    }
}
