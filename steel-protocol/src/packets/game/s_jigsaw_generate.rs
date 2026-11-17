//! Serverbound packet asking a jigsaw block to generate its structure.

use std::io::{Cursor, Result};

use steel_macros::ServerPacket;
use steel_utils::BlockPos;
use steel_utils::codec::VarInt;
use steel_utils::serial::ReadFrom;

/// Sent when a game master presses "Generate" on a jigsaw block's screen,
/// vanilla `ServerboundJigsawGeneratePacket`.
#[derive(ServerPacket, Clone, Debug)]
pub struct SJigsawGenerate {
    /// The jigsaw block's position.
    pub pos: BlockPos,
    /// How many pieces deep to generate.
    pub levels: i32,
    /// Whether the placed pieces keep their jigsaw blocks.
    pub keep_jigsaws: bool,
}

impl ReadFrom for SJigsawGenerate {
    fn read(data: &mut Cursor<&[u8]>) -> Result<Self> {
        Ok(Self {
            pos: BlockPos::read(data)?,
            levels: VarInt::read(data)?.0,
            keep_jigsaws: bool::read(data)?,
        })
    }
}
