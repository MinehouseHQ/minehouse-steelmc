//! Serverbound packet saving a test block's settings from its screen.

use std::io::{Cursor, Result};

use steel_macros::ServerPacket;
use steel_registry::blocks::properties::TestBlockMode;
use steel_utils::BlockPos;
use steel_utils::codec::VarInt;
use steel_utils::serial::{ReadFrom, prefixed_read::read_utf};

/// Vanilla `ByteBufCodecs.STRING_UTF8` limit.
const MAX_STRING_LENGTH: usize = 32_767;

/// Sent when a game master presses "Done" on a test block's screen, vanilla
/// `ServerboundSetTestBlockPacket`.
#[derive(ServerPacket, Clone, Debug)]
pub struct SSetTestBlock {
    /// The test block's position.
    pub pos: BlockPos,
    /// The chosen mode.
    pub mode: TestBlockMode,
    /// The message it logs.
    pub message: String,
}

impl ReadFrom for SSetTestBlock {
    fn read(data: &mut Cursor<&[u8]>) -> Result<Self> {
        let pos = BlockPos::read(data)?;
        // Vanilla `TestBlockMode.STREAM_CODEC` maps ids out of range to `START`.
        let mode = match VarInt::read(data)?.0 {
            1 => TestBlockMode::Log,
            2 => TestBlockMode::Fail,
            3 => TestBlockMode::Accept,
            _ => TestBlockMode::Start,
        };
        Ok(Self {
            pos,
            mode,
            message: read_utf(data, MAX_STRING_LENGTH)?,
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
        BlockPos::new(1, 2, 3).write(&mut bytes).expect("position");
        VarInt(3).write(&mut bytes).expect("mode");
        VarInt(2).write(&mut bytes).expect("length");
        bytes.extend_from_slice(b"ok");

        let packet = SSetTestBlock::read(&mut Cursor::new(&bytes)).expect("decodes");
        assert_eq!(packet.pos, BlockPos::new(1, 2, 3));
        assert_eq!(packet.mode, TestBlockMode::Accept);
        assert_eq!(packet.message, "ok");
    }
}
