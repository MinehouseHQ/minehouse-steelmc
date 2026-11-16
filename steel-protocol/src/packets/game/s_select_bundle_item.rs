use steel_macros::{ReadFrom, ServerPacket};

/// Vanilla `ServerboundSelectBundleItemPacket`: the player scrolled to a stack
/// inside the bundle in `slot_id` of their open menu.
#[derive(ServerPacket, ReadFrom, Clone, Debug)]
pub struct SSelectBundleItem {
    /// The menu slot holding the bundle.
    #[read(as = VarInt)]
    pub slot_id: i32,
    /// The selected stack index, `-1` for none.
    #[read(as = VarInt)]
    pub selected_item_index: i32,
}
