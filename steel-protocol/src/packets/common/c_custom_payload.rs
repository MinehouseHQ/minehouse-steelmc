use std::io::{Result, Write};

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::config::C_CUSTOM_PAYLOAD;
use steel_registry::packets::play::C_CUSTOM_PAYLOAD as PLAY_C_CUSTOM_PAYLOAD;
use steel_utils::{Identifier, serial::WriteTo};

#[derive(WriteTo, ClientPacket, Clone, Debug)]
#[packet_id(Config = C_CUSTOM_PAYLOAD, Play = PLAY_C_CUSTOM_PAYLOAD)]
pub struct CCustomPayload {
    pub identifier: Identifier,
    #[write(as = Prefixed(VarInt))]
    pub payload: Box<[u8]>,
}

impl CCustomPayload {
    #[must_use]
    pub const fn new(identifier: Identifier, payload: Box<[u8]>) -> Self {
        Self {
            identifier,
            payload,
        }
    }
}

/// A message for a client mod on a plugin channel. The data goes out as is, to the end of
/// the packet: every channel has its own format ([`CCustomPayload`] prefixes a length, which
/// is right only for `minecraft:brand`).
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Config = C_CUSTOM_PAYLOAD, Play = PLAY_C_CUSTOM_PAYLOAD)]
pub struct CPluginMessage {
    pub channel: Identifier,
    pub data: Box<[u8]>,
}

impl CPluginMessage {
    #[must_use]
    pub const fn new(channel: Identifier, data: Box<[u8]>) -> Self {
        Self { channel, data }
    }
}

impl WriteTo for CPluginMessage {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.channel.write(writer)?;
        writer.write_all(&self.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_message_data_has_no_length_prefix() {
        let packet =
            CPluginMessage::new(Identifier::new_static("plasmo", "voice/v2"), [1, 2].into());
        let mut bytes = Vec::new();
        packet
            .write(&mut bytes)
            .unwrap_or_else(|error| panic!("{error}"));
        let mut expected = vec![15];
        expected.extend_from_slice(b"plasmo:voice/v2");
        expected.extend_from_slice(&[1, 2]);
        assert_eq!(bytes, expected);
    }
}
