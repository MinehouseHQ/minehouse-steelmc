//! Clientbound add transient block packet, vanilla
//! `ClientboundAddTransientBlockPacket`.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_ADD_TRANSIENT_BLOCK;
use steel_utils::{BlockPos, BlockStateId};

/// Sent after a falling block lands so the client keeps showing the landed
/// block until its own chunk data catches up.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_ADD_TRANSIENT_BLOCK)]
pub struct CAddTransientBlock {
    /// Where the block landed.
    pub pos: BlockPos,
    /// The block state now at `pos`.
    pub block_state: BlockStateId,
}
