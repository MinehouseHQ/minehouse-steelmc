//! Serverbound packet saving a command block's settings from its screen.

use std::io::{Cursor, Error, ErrorKind, Result};

use steel_macros::ServerPacket;
use steel_utils::BlockPos;
use steel_utils::codec::VarInt;
use steel_utils::serial::{ReadFrom, prefixed_read::read_utf};

/// Vanilla `FriendlyByteBuf.readUtf` default limit.
const MAX_COMMAND_LENGTH: usize = 32_767;

const FLAG_TRACK_OUTPUT: u8 = 1;
const FLAG_CONDITIONAL: u8 = 2;
const FLAG_AUTOMATIC: u8 = 4;

/// Vanilla `CommandBlockEntity.Mode`, in wire order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandBlockPacketMode {
    /// A chain command block.
    Sequence,
    /// A repeating command block.
    Auto,
    /// An impulse command block.
    Redstone,
}

/// Sent when a game master presses "Done" on a command block screen,
/// vanilla `ServerboundSetCommandBlockPacket`.
#[derive(ServerPacket, Clone, Debug)]
pub struct SSetCommandBlock {
    /// The command block's position.
    pub pos: BlockPos,
    /// The command to run.
    pub command: String,
    /// Which command block it should be.
    pub mode: CommandBlockPacketMode,
    /// Whether the block keeps its last output.
    pub track_output: bool,
    /// Whether the block only runs when the one behind it succeeded.
    pub conditional: bool,
    /// Whether the block runs without redstone.
    pub automatic: bool,
}

impl ReadFrom for SSetCommandBlock {
    fn read(data: &mut Cursor<&[u8]>) -> Result<Self> {
        let pos = BlockPos::read(data)?;
        let command = read_utf(data, MAX_COMMAND_LENGTH)?;
        let mode = match VarInt::read(data)?.0 {
            0 => CommandBlockPacketMode::Sequence,
            1 => CommandBlockPacketMode::Auto,
            2 => CommandBlockPacketMode::Redstone,
            other => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("invalid command block mode {other}"),
                ));
            }
        };
        let flags = u8::read(data)?;
        Ok(Self {
            pos,
            command,
            mode,
            track_output: flags & FLAG_TRACK_OUTPUT != 0,
            conditional: flags & FLAG_CONDITIONAL != 0,
            automatic: flags & FLAG_AUTOMATIC != 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use steel_utils::serial::WriteTo as _;

    use super::*;

    #[test]
    fn reads_the_vanilla_layout() {
        let mut bytes = Vec::new();
        BlockPos::new(3, 64, -7)
            .write(&mut bytes)
            .expect("position encodes");
        VarInt(4).write(&mut bytes).expect("length encodes");
        bytes.extend_from_slice(b"/say");
        VarInt(1).write(&mut bytes).expect("mode encodes");
        bytes.push(FLAG_TRACK_OUTPUT | FLAG_AUTOMATIC);

        let packet = SSetCommandBlock::read(&mut Cursor::new(&bytes)).expect("packet decodes");
        assert_eq!(packet.pos, BlockPos::new(3, 64, -7));
        assert_eq!(packet.command, "/say");
        assert_eq!(packet.mode, CommandBlockPacketMode::Auto);
        assert!(packet.track_output);
        assert!(!packet.conditional);
        assert!(packet.automatic);
    }

    #[test]
    fn rejects_an_unknown_mode() {
        let mut bytes = Vec::new();
        BlockPos::new(0, 0, 0)
            .write(&mut bytes)
            .expect("position encodes");
        VarInt(0).write(&mut bytes).expect("length encodes");
        VarInt(3).write(&mut bytes).expect("mode encodes");
        bytes.push(0);
        assert!(SSetCommandBlock::read(&mut Cursor::new(&bytes)).is_err());
    }
}
