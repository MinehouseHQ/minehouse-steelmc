//! Serverbound packet saving a structure block's settings from its screen.

use std::io::{Cursor, Error, ErrorKind, Result};

use steel_macros::ServerPacket;
use steel_registry::blocks::properties::StructureMode;
use steel_utils::codec::{VarInt, VarLong};
use steel_utils::serial::{ReadFrom, prefixed_read::read_utf};
use steel_utils::{BlockPos, Rotation};

/// Vanilla `FriendlyByteBuf.readUtf` default limit.
const MAX_STRING_LENGTH: usize = 32_767;
/// Vanilla `StructureBlockEntity.MAX_OFFSET_PER_AXIS`.
const MAX_OFFSET_PER_AXIS: i8 = 48;
/// Vanilla `StructureBlockEntity.MAX_SIZE_PER_AXIS`.
const MAX_SIZE_PER_AXIS: i8 = 48;
/// Vanilla `FLAG_IGNORE_ENTITIES`.
const FLAG_IGNORE_ENTITIES: u8 = 1;
/// Vanilla `FLAG_SHOW_AIR`.
const FLAG_SHOW_AIR: u8 = 2;
/// Vanilla `FLAG_SHOW_BOUNDING_BOX`.
const FLAG_SHOW_BOUNDING_BOX: u8 = 4;
/// Vanilla `FLAG_STRICT`.
const FLAG_STRICT: u8 = 8;

/// Vanilla `StructureBlockEntity.UpdateType`: which button closed the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StructureBlockUpdateType {
    /// "Done": only the settings change.
    UpdateData,
    /// "Save".
    SaveArea,
    /// "Load".
    LoadArea,
    /// "Detect".
    ScanArea,
}

/// Vanilla `Mirror`, in its ordinal order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StructureBlockMirror {
    /// No mirror.
    None,
    /// Vanilla `LEFT_RIGHT`.
    LeftRight,
    /// Vanilla `FRONT_BACK`.
    FrontBack,
}

/// Sent when a game master closes a structure block's screen with a button,
/// vanilla `ServerboundSetStructureBlockPacket`.
#[derive(ServerPacket, Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "vanilla sends these four settings as one flag byte"
)]
pub struct SSetStructureBlock {
    /// The structure block's position.
    pub pos: BlockPos,
    /// The button pressed.
    pub update_type: StructureBlockUpdateType,
    /// The block's mode.
    pub mode: StructureMode,
    /// The structure name as typed.
    pub name: String,
    /// The structure's offset from the block, each axis in -48..=48.
    pub offset: [i32; 3],
    /// The structure's size, each axis in 0..=48.
    pub size: [i32; 3],
    /// The mirror to load with.
    pub mirror: StructureBlockMirror,
    /// The rotation to load with.
    pub rotation: Rotation,
    /// A data block's metadata.
    pub data: String,
    /// Whether entities are left out of saving and loading.
    pub ignore_entities: bool,
    /// Whether blocks load without updating their shape.
    pub strict: bool,
    /// Whether air shows in the save box.
    pub show_air: bool,
    /// Whether the box shows.
    pub show_bounding_box: bool,
    /// How much of the structure loads, in 0..=1.
    pub integrity: f32,
    /// The seed of the integrity random; 0 means a fresh one.
    pub seed: i64,
}

fn read_ordinal(data: &mut Cursor<&[u8]>, count: usize, name: &str) -> Result<usize> {
    let ordinal = VarInt::read(data)?.0;
    usize::try_from(ordinal)
        .ok()
        .filter(|ordinal| *ordinal < count)
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, format!("invalid {name} {ordinal}")))
}

fn read_clamped(data: &mut Cursor<&[u8]>, min: i8, max: i8) -> Result<[i32; 3]> {
    Ok([
        i32::from(i8::read(data)?.clamp(min, max)),
        i32::from(i8::read(data)?.clamp(min, max)),
        i32::from(i8::read(data)?.clamp(min, max)),
    ])
}

impl ReadFrom for SSetStructureBlock {
    fn read(data: &mut Cursor<&[u8]>) -> Result<Self> {
        let pos = BlockPos::read(data)?;
        let update_type = [
            StructureBlockUpdateType::UpdateData,
            StructureBlockUpdateType::SaveArea,
            StructureBlockUpdateType::LoadArea,
            StructureBlockUpdateType::ScanArea,
        ][read_ordinal(data, 4, "update type")?];
        let mode = [
            StructureMode::Save,
            StructureMode::Load,
            StructureMode::Corner,
            StructureMode::Data,
        ][read_ordinal(data, 4, "structure mode")?];
        let name = read_utf(data, MAX_STRING_LENGTH)?;
        let offset = read_clamped(data, -MAX_OFFSET_PER_AXIS, MAX_OFFSET_PER_AXIS)?;
        let size = read_clamped(data, 0, MAX_SIZE_PER_AXIS)?;
        let mirror = [
            StructureBlockMirror::None,
            StructureBlockMirror::LeftRight,
            StructureBlockMirror::FrontBack,
        ][read_ordinal(data, 3, "mirror")?];
        // Vanilla `Rotation.STREAM_CODEC` maps ids out of range to `NONE`.
        let rotation = match VarInt::read(data)?.0 {
            1 => Rotation::Clockwise90,
            2 => Rotation::Clockwise180,
            3 => Rotation::CounterClockwise90,
            _ => Rotation::None,
        };
        let metadata = read_utf(data, 128)?;
        let integrity = f32::read(data)?.clamp(0.0, 1.0);
        let seed = VarLong::read(data)?.0;
        let flags = u8::read(data)?;
        Ok(Self {
            pos,
            update_type,
            mode,
            name,
            offset,
            size,
            mirror,
            rotation,
            data: metadata,
            ignore_entities: flags & FLAG_IGNORE_ENTITIES != 0,
            strict: flags & FLAG_STRICT != 0,
            show_air: flags & FLAG_SHOW_AIR != 0,
            show_bounding_box: flags & FLAG_SHOW_BOUNDING_BOX != 0,
            integrity,
            seed,
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
        VarInt(1).write(&mut bytes).expect("update type");
        VarInt(0).write(&mut bytes).expect("mode");
        write_str(&mut bytes, "minecraft:house");
        bytes.extend([0, 1, 0x9c_u8]); // offset 0, 1, -100
        bytes.extend([5, 60, 7]); // size 5, 60, 7
        VarInt(2).write(&mut bytes).expect("mirror");
        VarInt(3).write(&mut bytes).expect("rotation");
        write_str(&mut bytes, "");
        bytes.extend(1.5_f32.to_be_bytes());
        VarLong(-7).write(&mut bytes).expect("seed");
        bytes.push(FLAG_IGNORE_ENTITIES | FLAG_STRICT);

        let packet = SSetStructureBlock::read(&mut Cursor::new(&bytes)).expect("decodes");
        assert_eq!(packet.pos, BlockPos::new(1, 2, 3));
        assert_eq!(packet.update_type, StructureBlockUpdateType::SaveArea);
        assert_eq!(packet.mode, StructureMode::Save);
        assert_eq!(packet.name, "minecraft:house");
        assert_eq!(packet.offset, [0, 1, -48], "offsets clamp to 48");
        assert_eq!(packet.size, [5, 48, 7], "sizes clamp to 48");
        assert_eq!(packet.mirror, StructureBlockMirror::FrontBack);
        assert_eq!(packet.rotation, Rotation::CounterClockwise90);
        assert!((packet.integrity - 1.0).abs() < f32::EPSILON);
        assert_eq!(packet.seed, -7);
        assert!(packet.ignore_entities && packet.strict);
        assert!(!packet.show_air && !packet.show_bounding_box);
    }
}
