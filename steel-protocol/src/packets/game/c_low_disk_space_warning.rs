//! Clientbound low disk space warning, vanilla `ClientboundLowDiskSpaceWarningPacket`.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_LOW_DISK_SPACE_WARNING;

/// Tells an admin that the server may fail to save the world. It has no fields.
#[derive(ClientPacket, WriteTo, Clone, Debug, Default)]
#[packet_id(Play = C_LOW_DISK_SPACE_WARNING)]
pub struct CLowDiskSpaceWarning;
