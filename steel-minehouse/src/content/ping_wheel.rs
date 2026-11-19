//! The server half of the client mod Ping Wheel (1.12): a player marks a place or an entity,
//! the server passes the mark on to everyone in the same ping channel.
//!
//! The client only sends once the server has said it takes the channels, so a client that
//! lists its own channels (`minecraft:register`) is answered with ours. Marks go to the
//! players whose client listed the clientbound channel, and who are in the same ping
//! channel as the sender; the channel with no name is the one everybody starts in.

use std::{
    str,
    sync::Arc,
    time::{Duration, Instant},
};

use rustc_hash::{FxHashMap, FxHashSet};

use steel_core::{
    player::Player,
    server::{Server, plugin_channels::PluginChannelHandler},
};
use steel_protocol::packets::common::CPluginMessage;
use steel_utils::{Identifier, locks::SyncMutex};
use uuid::Uuid;

/// Fabric's list of channels a side takes, names split by a zero byte.
const REGISTER: &str = "minecraft:register";
/// A mark from a client.
const PING_C2S: &str = "ping-wheel-c2s:ping-location";
/// The ping channel a client switched to.
const CHANNEL_C2S: &str = "ping-wheel-c2s:update-channel";
/// A mark for a client.
const PING_S2C: &str = "ping-wheel-s2c:ping-location";

/// The longest name of a ping channel the mod writes.
const MAX_CHANNEL_LENGTH: usize = 128;
/// The marks a player may make in a row (the mod's `rateLimit`).
const BURST: u32 = 5;
/// How long until one more mark is allowed (the mod's `msToRegenerate`).
const REGENERATE: Duration = Duration::from_secs(1);

/// What is known of one client with the mod.
struct Client {
    /// The ping channel, empty for the common one.
    channel: String,
    /// Marks left before the limit.
    allowed: u32,
    /// When `allowed` was last counted up.
    counted: Instant,
}

impl Client {
    const fn new(now: Instant) -> Self {
        Self {
            channel: String::new(),
            allowed: BURST,
            counted: now,
        }
    }

    /// Takes one mark from the allowance. `false` if the player marks too fast.
    fn take(&mut self, now: Instant) -> bool {
        let regenerated = now.duration_since(self.counted).as_millis() / REGENERATE.as_millis();
        if regenerated > 0 {
            self.allowed = self
                .allowed
                .saturating_add(u32::try_from(regenerated).unwrap_or(BURST))
                .min(BURST);
            self.counted = now;
        }
        if self.allowed == 0 {
            return false;
        }
        self.allowed -= 1;
        true
    }
}

/// Passes marks on.
#[derive(Default)]
struct PingWheel {
    /// The players whose client takes marks.
    clients: SyncMutex<FxHashMap<Uuid, Client>>,
}

/// A mark as the client sent it.
#[derive(Debug, PartialEq)]
struct Ping<'a> {
    /// The ping channel it is for.
    channel: &'a str,
    /// Everything after the channel: the place, the entity, the sequence and the dimension.
    rest: &'a [u8],
}

/// Reads a string the way `FriendlyByteBuf.writeUtf` wrote it. Returns it with what follows.
fn read_string(data: &[u8]) -> Option<(&str, &[u8])> {
    let mut length = 0_usize;
    let mut used = 0;
    loop {
        let byte = *data.get(used)?;
        length |= usize::from(byte & 0x7f) << (7 * used);
        used += 1;
        if byte & 0x80 == 0 {
            break;
        }
        if used == 3 {
            return None;
        }
    }
    let text = data.get(used..used + length)?;
    let text = str::from_utf8(text).ok()?;
    (text.chars().count() <= MAX_CHANNEL_LENGTH).then_some((text, &data[used + length..]))
}

/// Checks a mark from a client: three doubles, an entity or none, two ints.
fn read_ping(data: &[u8]) -> Option<Ping<'_>> {
    const PLACE: usize = 3 * 8;
    const TAIL: usize = 2 * 4;
    let (channel, rest) = read_string(data)?;
    let entity = match *rest.get(PLACE)? {
        0 => 0,
        1 => 16,
        _ => return None,
    };
    (rest.len() == PLACE + 1 + entity + TAIL).then_some(Ping { channel, rest })
}

/// The mark as the clients get it: what the sender wrote and who the sender is.
fn relayed(data: &[u8], author: Uuid) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 16);
    out.extend_from_slice(data);
    out.extend_from_slice(author.as_bytes());
    out
}

impl PluginChannelHandler for PingWheel {
    fn plugin_message(
        &self,
        server: &Arc<Server>,
        player: &Player,
        channel: &Identifier,
        data: &[u8],
    ) {
        let uuid = player.gameprofile.id();
        let now = Instant::now();
        match channel.to_string().as_str() {
            REGISTER => {
                let takes_pings = data
                    .split(|byte| *byte == 0)
                    .any(|name| name == PING_S2C.as_bytes());
                if !takes_pings {
                    return;
                }
                // Whoever left is forgotten when the next client comes.
                let online: FxHashSet<Uuid> = server
                    .get_players()
                    .iter()
                    .map(|player| player.gameprofile.id())
                    .collect();
                let mut clients = self.clients.lock();
                clients.retain(|other, _| online.contains(other));
                clients.insert(uuid, Client::new(now));
                drop(clients);
                let ours = [PING_C2S, CHANNEL_C2S].join("\0");
                player.send_packet(CPluginMessage::new(
                    Identifier::new_static("minecraft", "register"),
                    ours.into_bytes().into(),
                ));
            }
            CHANNEL_C2S => {
                let Some((name, [])) = read_string(data) else {
                    return;
                };
                if let Some(client) = self.clients.lock().get_mut(&uuid) {
                    name.clone_into(&mut client.channel);
                }
            }
            PING_C2S => {
                let Some(ping) = read_ping(data) else {
                    log::warn!(
                        "Ping Wheel: a mark of {} could not be read",
                        player.gameprofile.name()
                    );
                    return;
                };
                let receivers: FxHashSet<Uuid> = {
                    let mut clients = self.clients.lock();
                    let Some(sender) = clients.get_mut(&uuid) else {
                        return;
                    };
                    if !sender.take(now) {
                        return;
                    }
                    // The mark says which channel it is for: a client that switched without
                    // telling is believed.
                    ping.channel.clone_into(&mut sender.channel);
                    clients
                        .iter()
                        .filter(|(_, client)| client.channel == ping.channel)
                        .map(|(receiver, _)| *receiver)
                        .collect()
                };
                let payload = relayed(data, uuid);
                for receiver in server.get_players() {
                    if receivers.contains(&receiver.gameprofile.id()) {
                        receiver.send_packet(CPluginMessage::new(
                            Identifier::new_static("ping-wheel-s2c", "ping-location"),
                            payload.clone().into(),
                        ));
                    }
                }
            }
            _ => {}
        }
    }
}

/// Starts taking marks.
pub(super) fn start(server: &Server) {
    server.add_plugin_channel_handler(Arc::new(PingWheel::default()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ping(channel: &str, entity: Option<Uuid>) -> Vec<u8> {
        let mut data = vec![u8::try_from(channel.len()).unwrap_or_default()];
        data.extend_from_slice(channel.as_bytes());
        for coordinate in [1.5_f64, 64.0, -3.25] {
            data.extend_from_slice(&coordinate.to_be_bytes());
        }
        match entity {
            Some(entity) => {
                data.push(1);
                data.extend_from_slice(entity.as_bytes());
            }
            None => data.push(0),
        }
        data.extend_from_slice(&7_i32.to_be_bytes());
        data.extend_from_slice(&0_i32.to_be_bytes());
        data
    }

    #[test]
    fn a_mark_is_read_with_its_channel() {
        let bare = ping("", None);
        assert_eq!(read_ping(&bare).map(|ping| ping.channel), Some(""));
        let aimed = ping("база", Some(Uuid::from_u128(5)));
        assert_eq!(read_ping(&aimed).map(|ping| ping.channel), Some("база"));
    }

    #[test]
    fn a_cut_or_padded_mark_is_refused() {
        let mut data = ping("a", None);
        data.pop();
        assert_eq!(read_ping(&data), None);
        let mut data = ping("a", None);
        data.push(0);
        assert_eq!(read_ping(&data), None);
        assert_eq!(read_ping(&[200, 200, 200, 1]), None);
    }

    #[test]
    fn the_author_is_added_at_the_end() {
        let data = ping("", None);
        let author = Uuid::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10);
        let out = relayed(&data, author);
        assert_eq!(&out[..data.len()], &data[..]);
        assert_eq!(out[data.len()], 1);
        assert_eq!(out[out.len() - 1], 0x10);
    }

    #[test]
    fn marks_are_limited_and_come_back() {
        let start = Instant::now();
        let mut client = Client::new(start);
        for _ in 0..BURST {
            assert!(client.take(start));
        }
        assert!(!client.take(start));
        assert!(client.take(start + REGENERATE));
        assert!(!client.take(start + REGENERATE));
    }
}
