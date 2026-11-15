//! The magic herb. So far only the block of magic leaves: nine leaves packed into one block
//! for storage, and back into nine on the crafting table. A port of the Fabric `LeafBlock`.
//! The plant, the cauldron and what the leaves are for are not ported yet.

use steel_registry::{
    blocks::{Block, BlockRegistry, StateByteData, behavior::BlockConfig},
    sound_types, vanilla_blocks,
};
use steel_utils::Identifier;

use super::blocks::show_full_block;

/// Vanilla `MapColor.COLOR_CYAN`.
const MAP_COLOR_CYAN: u8 = 23;

/// «Блок магических листьев».
pub static LEAF_BLOCK: Block = Block::new(
    Identifier::new_static("minehouse-herbs", "magic_leaf_block"),
    BlockConfig::new()
        .destroy_time(1.0)
        .explosion_resistance(1.0)
        .sound_type(sound_types::WART_BLOCK),
    &[],
)
.with_map_color(StateByteData::new(MAP_COLOR_CYAN, &[]));

/// Has clients see the block as its note block state, or as a warped wart block, which is
/// the same teal, without one. It breaks like a wart block too.
pub(super) fn show_to_clients(blocks: &mut BlockRegistry) {
    let wart = blocks.get_default_state_id(&vanilla_blocks::WARPED_WART_BLOCK);
    show_full_block(blocks, &LEAF_BLOCK, wart, wart);
}
