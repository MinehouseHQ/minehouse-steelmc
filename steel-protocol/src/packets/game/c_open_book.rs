use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_OPEN_BOOK;
use steel_utils::types::InteractionHand;

/// Clientbound packet that opens the written book held in a hand.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_OPEN_BOOK)]
pub struct COpenBook {
    /// The hand holding the book.
    pub hand: InteractionHand,
}

#[cfg(test)]
mod tests {
    use steel_utils::serial::WriteTo as _;

    use super::*;

    #[test]
    fn open_book_writes_hand_as_varint() {
        let mut bytes = Vec::new();
        COpenBook {
            hand: InteractionHand::OffHand,
        }
        .write(&mut bytes)
        .expect("packet should encode");
        assert_eq!(bytes, [1]);
    }
}
