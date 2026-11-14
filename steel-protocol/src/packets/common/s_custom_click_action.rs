use steel_macros::{ReadFrom, ServerPacket};
use steel_utils::Identifier;

/// A dialog button or text click with a `custom` action. The optional NBT payload after
/// the id is not read.
#[derive(ReadFrom, ServerPacket, Clone, Debug)]
pub struct SCustomClickAction {
    pub id: Identifier,
}
