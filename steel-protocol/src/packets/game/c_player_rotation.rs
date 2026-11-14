//! Packet that turns the local player, vanilla `ClientboundPlayerRotationPacket`.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_PLAYER_ROTATION;

/// Sets the receiving player's rotation, each part absolute or added to the
/// client's current value.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_PLAYER_ROTATION)]
pub struct CPlayerRotation {
    /// Yaw, or the yaw change when `relative_y` is set.
    pub y_rot: f32,
    /// Whether `y_rot` is added to the current yaw.
    pub relative_y: bool,
    /// Pitch, or the pitch change when `relative_x` is set.
    pub x_rot: f32,
    /// Whether `x_rot` is added to the current pitch.
    pub relative_x: bool,
}
