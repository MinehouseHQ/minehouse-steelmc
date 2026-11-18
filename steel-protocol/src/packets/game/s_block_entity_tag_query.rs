use steel_macros::{ReadFrom, ServerPacket};
use steel_utils::BlockPos;

/// Serverbound packet sent when an operator copies a block's data
/// (F3 + I on a block entity).
#[derive(ReadFrom, ServerPacket, Clone, Debug)]
pub struct SBlockEntityTagQuery {
    /// Id echoed back in the answering `CTagQuery`.
    #[read(as = VarInt)]
    pub transaction_id: i32,
    /// Position of the queried block.
    pub pos: BlockPos,
}
