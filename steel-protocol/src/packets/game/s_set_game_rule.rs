use std::io::{Cursor, Error, Result};

use steel_macros::ServerPacket;
use steel_utils::Identifier;
use steel_utils::codec::VarInt;
use steel_utils::serial::{ReadFrom, prefixed_read::read_utf};

/// Vanilla `ByteBufCodecs.STRING_UTF8` length limit.
const MAX_VALUE_LENGTH: usize = 32767;

/// One rule change from the game rules screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameRuleEntry {
    /// The game rule's registry key.
    pub key: Identifier,
    /// The new value, serialized like `GameRule.serialize`.
    pub value: String,
}

/// Serverbound packet sent when an operator edits game rules from the game
/// rules screen.
#[derive(ServerPacket, Clone, Debug, PartialEq, Eq)]
pub struct SSetGameRule {
    /// The changed rules, in the order the client sent them.
    pub entries: Vec<GameRuleEntry>,
}

impl ReadFrom for SSetGameRule {
    fn read(data: &mut Cursor<&[u8]>) -> Result<Self> {
        let count = usize::try_from(VarInt::read(data)?.0)
            .map_err(|_| Error::other("negative game rule count"))?;
        let mut entries = Vec::with_capacity(count.min(256));
        for _ in 0..count {
            let key = Identifier::read(data)?;
            let value = read_utf(data, MAX_VALUE_LENGTH)?;
            entries.push(GameRuleEntry { key, value });
        }
        Ok(Self { entries })
    }
}

#[cfg(test)]
mod tests {
    use steel_utils::serial::WriteTo as _;

    use super::*;

    #[test]
    fn reads_each_rule_key_and_value() {
        let mut bytes = Vec::new();
        VarInt(1).write(&mut bytes).expect("count");
        for text in ["minecraft:keep_inventory", "true"] {
            VarInt(i32::try_from(text.len()).expect("short"))
                .write(&mut bytes)
                .expect("len");
            bytes.extend_from_slice(text.as_bytes());
        }

        let packet = SSetGameRule::read(&mut Cursor::new(bytes.as_slice())).expect("parses");

        assert_eq!(
            packet.entries,
            vec![GameRuleEntry {
                key: Identifier::vanilla_static("keep_inventory"),
                value: "true".to_owned(),
            }]
        );
    }
}
