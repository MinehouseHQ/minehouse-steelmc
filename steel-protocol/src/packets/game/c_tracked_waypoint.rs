//! Vanilla `ClientboundTrackedWaypointPacket`: one locator bar waypoint.

use std::io::{Result, Write};

use steel_macros::ClientPacket;
use steel_registry::packets::play::C_WAYPOINT;
use steel_utils::serial::{PrefixedWrite, WriteTo};
use steel_utils::{BlockPos, ChunkPos, Identifier, codec::VarInt};
use uuid::Uuid;

/// Vanilla `WaypointStyleAssets.DEFAULT`.
pub const DEFAULT_WAYPOINT_STYLE: Identifier = Identifier::vanilla_static("default");

/// Vanilla `Waypoint.Icon`: how a waypoint looks on the locator bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaypointIcon {
    /// Vanilla `style`, a `waypoint_style_asset` id.
    pub style: Identifier,
    /// Vanilla `color` as `0xRRGGBB`; `None` lets the client pick one.
    pub color: Option<i32>,
}

impl Default for WaypointIcon {
    fn default() -> Self {
        Self {
            style: DEFAULT_WAYPOINT_STYLE,
            color: None,
        }
    }
}

impl WaypointIcon {
    /// Vanilla `hasData`: whether the icon differs from the default one.
    #[must_use]
    pub fn has_data(&self) -> bool {
        self.style != DEFAULT_WAYPOINT_STYLE || self.color.is_some()
    }
}

impl WriteTo for WaypointIcon {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.style.write(writer)?;
        self.color.is_some().write(writer)?;
        if let Some(color) = self.color {
            // Vanilla `ByteBufCodecs.RGB_COLOR`: red, green and blue bytes.
            let [_, red, green, blue] = color.to_be_bytes();
            red.write(writer)?;
            green.write(writer)?;
            blue.write(writer)?;
        }
        Ok(())
    }
}

/// Vanilla `TrackedWaypoint` identifier: an entity UUID or a name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaypointId {
    /// The transmitting entity.
    Uuid(Uuid),
    /// A named waypoint.
    Name(String),
}

/// Vanilla `TrackedWaypoint.Type` with its contents.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WaypointPosition {
    /// `EMPTY`: no location.
    Empty,
    /// `VEC3I`: an exact block.
    Block(BlockPos),
    /// `CHUNK`: somewhere in a chunk.
    Chunk(ChunkPos),
    /// `AZIMUTH`: only a direction, in radians.
    Azimuth(f32),
}

/// Vanilla `TrackedWaypoint`.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackedWaypoint {
    /// Which waypoint this is.
    pub id: WaypointId,
    /// How it looks.
    pub icon: WaypointIcon,
    /// Where it is.
    pub position: WaypointPosition,
}

impl WriteTo for TrackedWaypoint {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        match &self.id {
            WaypointId::Uuid(uuid) => {
                true.write(writer)?;
                uuid.write(writer)?;
            }
            WaypointId::Name(name) => {
                false.write(writer)?;
                name.write_prefixed::<VarInt>(writer)?;
            }
        }
        self.icon.write(writer)?;
        match self.position {
            WaypointPosition::Empty => VarInt(0).write(writer),
            WaypointPosition::Block(pos) => {
                VarInt(1).write(writer)?;
                VarInt(pos.x()).write(writer)?;
                VarInt(pos.y()).write(writer)?;
                VarInt(pos.z()).write(writer)
            }
            WaypointPosition::Chunk(pos) => {
                VarInt(2).write(writer)?;
                VarInt(pos.0.x).write(writer)?;
                VarInt(pos.0.y).write(writer)
            }
            WaypointPosition::Azimuth(angle) => {
                VarInt(3).write(writer)?;
                angle.write(writer)
            }
        }
    }
}

/// Vanilla `ClientboundTrackedWaypointPacket.Operation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaypointOperation {
    /// `TRACK`: show a new waypoint.
    Track,
    /// `UNTRACK`: hide a waypoint.
    Untrack,
    /// `UPDATE`: move a waypoint.
    Update,
}

/// Shows, hides or moves a waypoint on the client's locator bar.
#[derive(ClientPacket, Clone, Debug, PartialEq)]
#[packet_id(Play = C_WAYPOINT)]
pub struct CTrackedWaypoint {
    /// What happens to the waypoint.
    pub operation: WaypointOperation,
    /// The waypoint.
    pub waypoint: TrackedWaypoint,
}

impl CTrackedWaypoint {
    /// Vanilla `removeWaypoint`: hides the entity's waypoint.
    #[must_use]
    pub fn remove(uuid: Uuid) -> Self {
        Self {
            operation: WaypointOperation::Untrack,
            waypoint: TrackedWaypoint {
                id: WaypointId::Uuid(uuid),
                icon: WaypointIcon::default(),
                position: WaypointPosition::Empty,
            },
        }
    }

    /// Vanilla `addWaypoint*` and `updateWaypoint*` for an entity.
    #[must_use]
    pub const fn entity(
        operation: WaypointOperation,
        uuid: Uuid,
        icon: WaypointIcon,
        position: WaypointPosition,
    ) -> Self {
        Self {
            operation,
            waypoint: TrackedWaypoint {
                id: WaypointId::Uuid(uuid),
                icon,
                position,
            },
        }
    }
}

impl WriteTo for CTrackedWaypoint {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        let operation = match self.operation {
            WaypointOperation::Track => 0,
            WaypointOperation::Untrack => 1,
            WaypointOperation::Update => 2,
        };
        VarInt(operation).write(writer)?;
        self.waypoint.write(writer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(packet: &CTrackedWaypoint) -> Vec<u8> {
        let mut bytes = Vec::new();
        packet.write(&mut bytes).expect("packet should encode");
        bytes
    }

    #[test]
    fn untrack_writes_operation_uuid_default_icon_and_empty_type() {
        let uuid = Uuid::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10);
        let bytes = encode(&CTrackedWaypoint::remove(uuid));
        let mut expected = vec![1, 1];
        expected.extend_from_slice(uuid.as_bytes());
        expected.push(17);
        expected.extend_from_slice(b"minecraft:default");
        expected.extend_from_slice(&[0, 0]);
        assert_eq!(bytes, expected);
    }

    #[test]
    fn colored_azimuth_writes_rgb_bytes_and_angle() {
        let packet = CTrackedWaypoint::entity(
            WaypointOperation::Update,
            Uuid::nil(),
            WaypointIcon {
                style: Identifier::vanilla_static("bowtie"),
                color: Some(0xFF12_3456_u32 as i32),
            },
            WaypointPosition::Azimuth(1.5),
        );
        let bytes = encode(&packet);
        let tail = &bytes[bytes.len() - 8..];
        assert_eq!(tail[..4], [0x12, 0x34, 0x56, 3]);
        assert_eq!(tail[4..], 1.5_f32.to_be_bytes());
        assert_eq!(bytes[0], 2);
    }
}
