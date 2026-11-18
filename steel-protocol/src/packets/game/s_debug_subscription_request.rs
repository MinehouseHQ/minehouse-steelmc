//! Serverbound debug subscription request, vanilla `ServerboundDebugSubscriptionRequestPacket`.

use std::io::{Cursor, Error, ErrorKind, Result};

use steel_macros::ServerPacket;
use steel_utils::codec::VarInt;
use steel_utils::serial::ReadFrom;

/// Vanilla `ByteBufCodecs.collection(ReferenceOpenHashSet::new, 32)`.
const MAX_SUBSCRIPTIONS: usize = 32;

/// Vanilla `DebugSubscriptions` registers 16 entries, in this order.
const DEBUG_SUBSCRIPTION_COUNT: i32 = 16;

/// Vanilla `DebugSubscriptions.DEDICATED_SERVER_TICK_TIME`, the first registry entry.
pub const DEDICATED_SERVER_TICK_TIME: i32 = 0;

/// The full set of debug data the client wants to receive; it replaces the previous set.
#[derive(ServerPacket, Clone, Debug, PartialEq, Eq)]
pub struct SDebugSubscriptionRequest {
    /// Registry ids from the client's built-in `debug_subscription` registry.
    pub subscriptions: Vec<i32>,
}

impl ReadFrom for SDebugSubscriptionRequest {
    fn read(data: &mut Cursor<&[u8]>) -> Result<Self> {
        let count = VarInt::read(data)?.0;
        let count = usize::try_from(count)
            .ok()
            .filter(|count| *count <= MAX_SUBSCRIPTIONS)
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "too many debug subscriptions"))?;
        let mut subscriptions = Vec::with_capacity(count);
        for _ in 0..count {
            let id = VarInt::read(data)?.0;
            if !(0..DEBUG_SUBSCRIPTION_COUNT).contains(&id) {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("unknown debug subscription id {id}"),
                ));
            }
            if !subscriptions.contains(&id) {
                subscriptions.push(id);
            }
        }
        Ok(Self { subscriptions })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use steel_utils::codec::VarInt;
    use steel_utils::serial::{ReadFrom as _, WriteTo as _};

    use super::SDebugSubscriptionRequest;

    fn encode(ids: &[i32]) -> Vec<u8> {
        let mut bytes = Vec::new();
        VarInt(ids.len() as i32)
            .write(&mut bytes)
            .expect("count should encode");
        for id in ids {
            VarInt(*id).write(&mut bytes).expect("id should encode");
        }
        bytes
    }

    #[test]
    fn reads_a_set_of_subscriptions() {
        let packet = SDebugSubscriptionRequest::read(&mut Cursor::new(&encode(&[0, 3, 0])[..]))
            .expect("known ids should decode");
        assert_eq!(packet.subscriptions, vec![0, 3]);
    }

    #[test]
    fn rejects_unknown_ids_and_oversized_sets() {
        assert!(SDebugSubscriptionRequest::read(&mut Cursor::new(&encode(&[16])[..])).is_err());
        assert!(SDebugSubscriptionRequest::read(&mut Cursor::new(&encode(&[0; 33])[..])).is_err());
    }
}
