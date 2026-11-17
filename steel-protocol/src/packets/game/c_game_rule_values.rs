use std::io::Write;

use steel_macros::ClientPacket;
use steel_registry::packets::play::C_GAME_RULE_VALUES;
use steel_utils::Identifier;
use steel_utils::codec::VarInt;
use steel_utils::serial::{PrefixedWrite as _, WriteTo};

/// Vanilla `ClientboundGameRuleValuesPacket`: every available game rule's key
/// and serialized value, answering a gamemaster's request from the client.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_GAME_RULE_VALUES)]
pub struct CGameRuleValues {
    /// Game rule keys with their values serialized like vanilla `GameRule.serialize`.
    pub values: Vec<(Identifier, String)>,
}

impl WriteTo for CGameRuleValues {
    fn write(&self, writer: &mut impl Write) -> std::io::Result<()> {
        VarInt(self.values.len() as i32).write(writer)?;
        for (key, value) in &self.values {
            key.write(writer)?;
            value.write_prefixed::<VarInt>(writer)?;
        }
        Ok(())
    }
}
