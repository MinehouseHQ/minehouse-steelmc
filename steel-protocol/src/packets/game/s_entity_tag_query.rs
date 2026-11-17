use steel_macros::{ReadFrom, ServerPacket};

/// Serverbound packet sent when an operator copies an entity's data
/// (F3 + I on an entity).
#[derive(ReadFrom, ServerPacket, Clone, Debug)]
pub struct SEntityTagQuery {
    /// Id echoed back in the answering `CTagQuery`.
    #[read(as = VarInt)]
    pub transaction_id: i32,
    /// Network id of the queried entity.
    #[read(as = VarInt)]
    pub entity_id: i32,
}
