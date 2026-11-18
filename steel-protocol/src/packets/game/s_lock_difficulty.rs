use steel_macros::{ReadFrom, ServerPacket};

/// Serverbound packet sent when an operator locks the difficulty from the
/// options screen.
#[derive(ReadFrom, ServerPacket, Clone, Debug)]
pub struct SLockDifficulty {
    /// Whether the difficulty becomes locked.
    pub locked: bool,
}
