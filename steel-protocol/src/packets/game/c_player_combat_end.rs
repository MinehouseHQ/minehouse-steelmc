//! Clientbound player combat end packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_PLAYER_COMBAT_END;

/// Vanilla `ClientboundPlayerCombatEndPacket`: the player's combat log ended.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_PLAYER_COMBAT_END)]
pub struct CPlayerCombatEnd {
    /// Vanilla `CombatTracker.getCombatDuration`, in ticks.
    #[write(as = VarInt)]
    pub duration: i32,
}
