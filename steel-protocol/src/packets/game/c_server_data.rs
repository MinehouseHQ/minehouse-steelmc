//! Clientbound server data packet, vanilla `ClientboundServerDataPacket`.

use std::io::{Result, Write};

use steel_macros::ClientPacket;
use steel_registry::packets::play::C_SERVER_DATA;
use steel_utils::codec::VarInt;
use steel_utils::serial::{PrefixedWrite as _, WriteTo};
use text_components::TextComponent;

/// The server list description and icon, sent once a player joins so the
/// client can update its saved server entry.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_SERVER_DATA)]
pub struct CServerData {
    /// The message of the day.
    pub motd: TextComponent,
    /// The PNG server icon.
    pub icon_bytes: Option<Vec<u8>>,
}

impl WriteTo for CServerData {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.motd.write(writer)?;
        match &self.icon_bytes {
            Some(icon) => {
                true.write(writer)?;
                icon.write_prefixed::<VarInt>(writer)
            }
            None => false.write(writer),
        }
    }
}
