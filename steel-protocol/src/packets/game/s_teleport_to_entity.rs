use steel_macros::{ReadFrom, ServerPacket};
use uuid::Uuid;

/// Serverbound packet sent when a spectator picks a player to teleport to from the spectator menu.
#[derive(ReadFrom, ServerPacket, Clone, Debug)]
pub struct STeleportToEntity {
    /// UUID of the entity to teleport to.
    pub uuid: Uuid,
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use steel_utils::serial::ReadFrom as _;

    use super::*;

    #[test]
    fn reads_the_target_uuid() {
        let uuid = Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
        let mut data = Cursor::new(uuid.as_bytes().as_slice());
        let packet = STeleportToEntity::read(&mut data).expect("packet should parse");

        assert_eq!(packet.uuid, uuid);
    }
}
