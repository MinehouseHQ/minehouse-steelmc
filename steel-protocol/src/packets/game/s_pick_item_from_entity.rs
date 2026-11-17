use steel_macros::{ReadFrom, ServerPacket};

/// Serverbound packet sent when a player uses the pick block key (middle click) on an entity.
#[derive(ReadFrom, ServerPacket, Clone, Debug)]
pub struct SPickItemFromEntity {
    /// Network id of the picked entity.
    #[read(as = VarInt)]
    pub id: i32,
    /// Whether the client asked for the entity's data (Ctrl + pick).
    pub include_data: bool,
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use steel_utils::serial::ReadFrom as _;

    use super::*;

    #[test]
    fn reads_the_entity_id_as_a_var_int_then_the_data_flag() {
        let mut data = Cursor::new([0xac, 0x02, 1].as_slice());
        let packet = SPickItemFromEntity::read(&mut data).expect("packet should parse");

        assert_eq!(packet.id, 300);
        assert!(packet.include_data);
    }
}
