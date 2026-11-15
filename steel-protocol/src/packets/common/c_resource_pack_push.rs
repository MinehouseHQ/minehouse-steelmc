use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::config::C_RESOURCE_PACK_PUSH;
use text_components::TextComponent;
use uuid::Uuid;

/// Vanilla `ClientboundResourcePackPushPacket`: offers a resource pack to the client.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Config = C_RESOURCE_PACK_PUSH)]
pub struct CResourcePackPush {
    /// Names the pack in the client's answers.
    pub id: Uuid,
    /// Where the client downloads the pack.
    #[write(as = Prefixed(VarInt))]
    pub url: String,
    /// The SHA-1 of the pack as 40 hex digits, or empty.
    #[write(as = Prefixed(VarInt))]
    pub hash: String,
    /// Whether the client is disconnected when it declines the pack.
    pub required: bool,
    /// Shown on the client's prompt.
    pub prompt: Option<TextComponent>,
}

#[cfg(test)]
mod tests {
    use steel_utils::serial::WriteTo as _;

    use super::*;

    #[test]
    fn push_is_written_in_the_vanilla_order() {
        let packet = CResourcePackPush {
            id: Uuid::from_u128(1),
            url: "http://a".to_owned(),
            hash: "ab".to_owned(),
            required: true,
            prompt: None,
        };
        let mut bytes = Vec::new();
        packet.write(&mut bytes).expect("should encode");

        let mut expected = vec![0; 15];
        expected.push(1); // the id, most significant bits first
        expected.push(8);
        expected.extend_from_slice(b"http://a");
        expected.push(2);
        expected.extend_from_slice(b"ab");
        expected.push(1); // required
        expected.push(0); // no prompt
        assert_eq!(bytes, expected);
    }
}
