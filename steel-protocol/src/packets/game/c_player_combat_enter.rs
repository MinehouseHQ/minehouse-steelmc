//! Clientbound player combat enter packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_PLAYER_COMBAT_ENTER;

/// Vanilla `ClientboundPlayerCombatEnterPacket`: the player was hit by a
/// living attacker and its combat log started. It has no fields.
#[derive(ClientPacket, WriteTo, Clone, Debug, Default)]
#[packet_id(Play = C_PLAYER_COMBAT_ENTER)]
pub struct CPlayerCombatEnter;
