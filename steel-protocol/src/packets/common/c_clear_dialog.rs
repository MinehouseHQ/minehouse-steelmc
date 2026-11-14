use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::config::C_CLEAR_DIALOG;
use steel_registry::packets::play::C_CLEAR_DIALOG as PLAY_C_CLEAR_DIALOG;

/// Closes the dialog the client has open.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Config = C_CLEAR_DIALOG, Play = PLAY_C_CLEAR_DIALOG)]
pub struct CClearDialog {}
