//! Serverbound packet saving a jigsaw block's settings from its screen.

use std::io::{Cursor, Result};

use steel_macros::ServerPacket;
use steel_registry::template_pool::JointType;
use steel_utils::codec::VarInt;
use steel_utils::serial::{ReadFrom, prefixed_read::read_utf};
use steel_utils::{BlockPos, Identifier};

/// Vanilla `FriendlyByteBuf.readUtf` default limit.
const MAX_STRING_LENGTH: usize = 32_767;

/// Sent when a game master presses "Done" on a jigsaw block's screen,
/// vanilla `ServerboundSetJigsawBlockPacket`.
#[derive(ServerPacket, Clone, Debug)]
pub struct SSetJigsawBlock {
    /// The jigsaw block's position.
    pub pos: BlockPos,
    /// The connector's name.
    pub name: Identifier,
    /// The connector name it attaches to.
    pub target: Identifier,
    /// The template pool the next piece comes from.
    pub pool: Identifier,
    /// The block state the jigsaw turns into.
    pub final_state: String,
    /// The joint; vanilla reads an unknown name as aligned.
    pub joint: JointType,
    /// Vanilla `selectionPriority`.
    pub selection_priority: i32,
    /// Vanilla `placementPriority`.
    pub placement_priority: i32,
}

impl ReadFrom for SSetJigsawBlock {
    fn read(data: &mut Cursor<&[u8]>) -> Result<Self> {
        Ok(Self {
            pos: BlockPos::read(data)?,
            name: Identifier::read(data)?,
            target: Identifier::read(data)?,
            pool: Identifier::read(data)?,
            final_state: read_utf(data, MAX_STRING_LENGTH)?,
            joint: JointType::by_name(&read_utf(data, MAX_STRING_LENGTH)?)
                .unwrap_or(JointType::Aligned),
            selection_priority: VarInt::read(data)?.0,
            placement_priority: VarInt::read(data)?.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use steel_utils::serial::WriteTo as _;

    use super::*;

    fn write_str(bytes: &mut Vec<u8>, value: &str) {
        VarInt(i32::try_from(value.len()).expect("short"))
            .write(bytes)
            .expect("length");
        bytes.extend_from_slice(value.as_bytes());
    }

    #[test]
    fn reads_the_vanilla_layout() {
        let mut bytes = Vec::new();
        BlockPos::new(1, 2, 3).write(&mut bytes).expect("position");
        write_str(&mut bytes, "minecraft:bottom");
        write_str(&mut bytes, "minecraft:top");
        write_str(&mut bytes, "minecraft:village/plains/houses");
        write_str(&mut bytes, "minecraft:stone");
        write_str(&mut bytes, "sideways");
        VarInt(2).write(&mut bytes).expect("selection");
        VarInt(-1).write(&mut bytes).expect("placement");

        let packet = SSetJigsawBlock::read(&mut Cursor::new(&bytes)).expect("decodes");
        assert_eq!(packet.pos, BlockPos::new(1, 2, 3));
        assert_eq!(packet.name, Identifier::vanilla_static("bottom"));
        assert_eq!(packet.target, Identifier::vanilla_static("top"));
        assert_eq!(packet.final_state, "minecraft:stone");
        assert_eq!(
            packet.joint,
            JointType::Aligned,
            "unknown joints are aligned"
        );
        assert_eq!(packet.selection_priority, 2);
        assert_eq!(packet.placement_priority, -1);
    }
}
