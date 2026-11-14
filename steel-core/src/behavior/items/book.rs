//! Writable and written book item behaviors.

use steel_macros::item_behavior;
use steel_registry::stat::vanilla_stat_types;

use crate::behavior::{InteractionResult, ItemBehavior, UseItemContext};

/// Vanilla `WritableBookItem.use` and `WrittenBookItem.use`.
fn open_book(context: &mut UseItemContext) -> InteractionResult {
    context.player.open_item_gui(context.hand);
    context.inv.with_item(|item| {
        context
            .player
            .award_stat(&vanilla_stat_types::ITEM_USED, item.item());
    });
    InteractionResult::Success
}

/// Behavior for the book and quill.
#[item_behavior]
pub struct WritableBookItem;

impl ItemBehavior for WritableBookItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        open_book(context)
    }
}

/// Behavior for the signed written book.
#[item_behavior]
pub struct WrittenBookItem;

impl ItemBehavior for WrittenBookItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        open_book(context)
    }
}
