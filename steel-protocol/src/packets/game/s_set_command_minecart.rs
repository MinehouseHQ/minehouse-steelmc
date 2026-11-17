//! Serverbound packet saving a command block minecart's command.

use std::io::{Cursor, Result};

use steel_macros::ServerPacket;
use steel_utils::codec::VarInt;
use steel_utils::serial::{ReadFrom, prefixed_read::read_utf};

/// Vanilla `FriendlyByteBuf.readUtf` default limit.
const MAX_COMMAND_LENGTH: usize = 32_767;

/// Sent when a game master presses "Done" on a command block minecart's
/// screen, vanilla `ServerboundSetCommandMinecartPacket`.
#[derive(ServerPacket, Clone, Debug)]
pub struct SSetCommandMinecart {
    /// The minecart's network entity id.
    pub entity: i32,
    /// The command to run.
    pub command: String,
    /// Whether the minecart keeps its last output.
    pub track_output: bool,
}

impl ReadFrom for SSetCommandMinecart {
    fn read(data: &mut Cursor<&[u8]>) -> Result<Self> {
        Ok(Self {
            entity: VarInt::read(data)?.0,
            command: read_utf(data, MAX_COMMAND_LENGTH)?,
            track_output: bool::read(data)?,
        })
    }
}
