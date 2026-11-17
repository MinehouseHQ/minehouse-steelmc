//! Answer to an operator's entity or block entity data query.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_TAG_QUERY;
use steel_utils::serial::OptionalNbt;

/// Packet answering `SEntityTagQuery` and `SBlockEntityTagQuery` with the
/// queried data, or an end tag when there is none.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_TAG_QUERY)]
pub struct CTagQuery {
    /// The query's transaction id.
    #[write(as = VarInt)]
    pub transaction_id: i32,
    /// The queried data.
    pub tag: OptionalNbt,
}
