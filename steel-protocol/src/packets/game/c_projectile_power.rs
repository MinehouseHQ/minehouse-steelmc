//! Clientbound projectile power packet, vanilla
//! `ClientboundProjectilePowerPacket`.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_PROJECTILE_POWER;

/// Tells the client how fast a fireball-like projectile accelerates, sent
/// together with its motion.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_PROJECTILE_POWER)]
pub struct CProjectilePower {
    /// The projectile's entity id.
    #[write(as = VarInt)]
    pub entity_id: i32,
    /// Speed added along the flight direction every tick.
    pub acceleration_power: f64,
}

impl CProjectilePower {
    /// Creates a projectile power packet.
    #[must_use]
    pub const fn new(entity_id: i32, acceleration_power: f64) -> Self {
        Self {
            entity_id,
            acceleration_power,
        }
    }
}
