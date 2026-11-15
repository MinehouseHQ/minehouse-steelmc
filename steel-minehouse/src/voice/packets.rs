//! The packets of the Plasmo Voice 2 protocol that proximity voice needs. The client mod
//! talks on the plugin channel `plasmo:voice/v2` (the "TCP" packets) and sends the sound over
//! UDP. The formats follow the `protocol` module of Plasmo Voice (LGPL-3.0, by Apehum).

use std::ops::Range;

use uuid::Uuid;

use super::wire::{In, Out, WireError};

/// `UUID.nameUUIDFromBytes("proximity_activation")`.
pub const PROXIMITY_ACTIVATION: Uuid = Uuid::from_u128(0x4aec_07ba_d109_3345_9a0a_9202_2a11_6cd0);
/// `UUID.nameUUIDFromBytes("proximity_line")`.
pub const PROXIMITY_LINE: Uuid = Uuid::from_u128(0xd939_2647_f6b2_332d_9adf_30a6_4008_28d6);

const PROXIMITY_NAME: &str = "proximity";
const PROXIMITY_TRANSLATION: &str = "pv.activation.proximity";
/// The id Plasmo Voice gives its own sources.
const ADDON_ID: &str = "plasmovoice";

/// What a client says on the plugin channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Serverbound {
    /// The answer to [`player_info_request`]: the client has the mod.
    PlayerInfo {
        voice_disabled: bool,
        microphone_muted: bool,
        version: String,
        /// RSA, X.509.
        public_key: Vec<u8>,
    },
    PlayerState {
        voice_disabled: bool,
        microphone_muted: bool,
    },
    /// The player stopped talking.
    AudioEnd {
        sequence: i64,
        activation: Uuid,
        distance: i16,
    },
    SourceInfoRequest {
        source: Uuid,
    },
    LanguageRequest {
        language: String,
    },
    /// A packet proximity voice has no use for.
    Other,
}

impl Serverbound {
    pub fn decode(data: &[u8]) -> Result<Self, WireError> {
        let mut input = In::new(data);
        Ok(match input.byte()? {
            5 => Self::LanguageRequest {
                language: input.utf(32)?,
            },
            10 => {
                let voice_disabled = input.bool()?;
                let microphone_muted = input.bool()?;
                // The Minecraft version.
                input.utf(64)?;
                Self::PlayerInfo {
                    voice_disabled,
                    microphone_muted,
                    version: input.utf(64)?,
                    public_key: input.bytes(2048)?.to_vec(),
                }
            }
            11 => Self::PlayerState {
                voice_disabled: input.bool()?,
                microphone_muted: input.bool()?,
            },
            12 => Self::AudioEnd {
                sequence: input.long()?,
                activation: input.uuid()?,
                distance: input.short()?,
            },
            15 => Self::SourceInfoRequest {
                source: input.uuid()?,
            },
            _ => Self::Other,
        })
    }
}

/// Tells the client where the voice server is; `0.0.0.0` means the address it joined with.
pub fn connection(secret: Uuid, host: &str, port: u16) -> Vec<u8> {
    let mut out = tcp(1);
    out.uuid(secret);
    out.utf(host);
    out.int(i32::from(port));
    out.0
}

/// Asks the client whether it has the mod: a client with it answers with its key.
pub fn player_info_request() -> Vec<u8> {
    tcp(2).0
}

/// The voice settings of the server.
pub struct Settings<'settings> {
    pub server_id: Uuid,
    pub sample_rate: i32,
    pub mtu_size: i32,
    pub opus_mode: &'settings str,
    pub opus_bitrate: i32,
    pub distances: &'settings [i32],
    pub default_distance: i32,
    pub icon_visibility: &'settings [String],
    pub icon_y_offset: f64,
}

/// The settings and the key of the sound, encrypted for this client.
pub fn config(settings: &Settings, encrypted_key: &[u8]) -> Vec<u8> {
    let mut out = tcp(3);
    out.uuid(settings.server_id);
    // The capture: sample rate, packet size, the codec.
    out.int(settings.sample_rate);
    out.int(settings.mtu_size);
    out.bool(true);
    out.utf("opus");
    out.count(2);
    out.utf("mode");
    out.utf(settings.opus_mode);
    out.utf("bitrate");
    out.utf(&settings.opus_bitrate.to_string());
    // The sound is encrypted from client to client; the server only hands out the key.
    out.bool(true);
    out.utf("AES/CBC/PKCS5Padding");
    out.bytes(encrypted_key);
    // The source lines: only "nearby".
    out.count(1);
    out.utf(PROXIMITY_NAME);
    out.utf(PROXIMITY_TRANSLATION);
    out.utf("plasmovoice:textures/icons/speaker.png");
    out.double(1.0);
    out.int(1);
    out.bool(false);
    // The activations: only "nearby".
    out.count(1);
    out.utf(PROXIMITY_NAME);
    out.utf(PROXIMITY_TRANSLATION);
    out.utf("plasmovoice:textures/icons/microphone.png");
    out.count(settings.distances.len());
    for &distance in settings.distances {
        out.int(distance);
    }
    out.int(settings.default_distance);
    // Proximity, transitive, no stereo, the codec of the capture.
    out.bool(true);
    out.bool(true);
    out.bool(false);
    out.bool(false);
    out.int(1);
    // The permissions the client checks itself.
    out.count(1);
    out.utf("pv.allow_freecam");
    out.bool(true);
    // The icons above the heads.
    out.count(settings.icon_visibility.len());
    for visibility in settings.icon_visibility {
        out.utf(visibility);
    }
    out.double(0.0);
    out.double(settings.icon_y_offset);
    out.double(0.0);
    out.0
}

/// The texts of the server in the language the client asked for.
pub fn language(name: &str, texts: &[(&str, &str)]) -> Vec<u8> {
    let mut out = tcp(6);
    out.utf(name);
    out.count(texts.len());
    for (key, text) in texts {
        out.utf(key);
        out.utf(text);
    }
    out.0
}

/// A player in voice chat, as the others see them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfo {
    pub id: Uuid,
    pub name: String,
    pub voice_disabled: bool,
    pub microphone_muted: bool,
}

impl PlayerInfo {
    fn write(&self, out: &mut Out) {
        out.uuid(self.id);
        out.utf(&self.name);
        // Muted by the server.
        out.bool(false);
        out.bool(self.voice_disabled);
        out.bool(self.microphone_muted);
    }
}

pub fn player_list(players: &[PlayerInfo]) -> Vec<u8> {
    let mut out = tcp(7);
    out.count(players.len());
    for player in players {
        player.write(&mut out);
    }
    out.0
}

pub fn player_info_update(player: &PlayerInfo) -> Vec<u8> {
    let mut out = tcp(8);
    player.write(&mut out);
    out.0
}

pub fn player_disconnect(player: Uuid) -> Vec<u8> {
    let mut out = tcp(9);
    out.uuid(player);
    out.0
}

fn write_source(out: &mut Out, source: Uuid, state: u8, player: &PlayerInfo) {
    out.utf("PLAYER");
    out.utf(ADDON_ID);
    out.uuid(source);
    // No name.
    out.bool(false);
    out.byte(state);
    // The decoder: opus without parameters.
    out.bool(true);
    out.utf("opus");
    out.count(0);
    // Mono, on the proximity line, with an icon, no cone.
    out.bool(false);
    out.uuid(PROXIMITY_LINE);
    out.bool(true);
    out.int(0);
    player.write(out);
}

/// Who a source of sound is.
pub fn source_info(source: Uuid, state: u8, player: &PlayerInfo) -> Vec<u8> {
    let mut out = tcp(16);
    write_source(&mut out, source, state, player);
    out.0
}

/// Tells the speaker which source their own voice is.
pub fn self_source_info(source: Uuid, state: u8, player: &PlayerInfo) -> Vec<u8> {
    let mut out = tcp(17);
    write_source(&mut out, source, state, player);
    out.uuid(player.id);
    out.uuid(PROXIMITY_ACTIVATION);
    out.long(-1);
    out.0
}

/// The source went silent.
pub fn source_audio_end(source: Uuid, sequence: i64) -> Vec<u8> {
    let mut out = tcp(18);
    out.uuid(source);
    out.long(sequence);
    out.0
}

fn tcp(id: u8) -> Out {
    let mut out = Out::default();
    out.byte(id);
    out
}

/// Marks a datagram as Plasmo Voice.
const UDP_MAGIC: i32 = 0x4e90_04e9;
/// Where the secret of the receiver sits in a datagram.
const UDP_SECRET: Range<usize> = 5..21;

/// What a client sends over UDP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UdpServerbound {
    Ping,
    /// 20 ms of encrypted opus.
    Audio {
        sequence: i64,
        data: Vec<u8>,
        activation: Uuid,
        distance: i16,
    },
}

/// The secret of the sender and their packet, or `None` for a datagram that isn't ours.
pub fn decode_udp(data: &[u8]) -> Option<(Uuid, UdpServerbound)> {
    let mut input = In::new(data);
    let read = |input: &mut In| -> Result<Option<(Uuid, UdpServerbound)>, WireError> {
        if input.int()? != UDP_MAGIC {
            return Ok(None);
        }
        let kind = input.byte()?;
        let secret = input.uuid()?;
        // When the client sent it.
        input.long()?;
        Ok(match kind {
            1 => Some((secret, UdpServerbound::Ping)),
            2 => {
                let sequence = input.long()?;
                let data = input.bytes(2048)?.to_vec();
                Some((
                    secret,
                    UdpServerbound::Audio {
                        sequence,
                        data,
                        activation: input.uuid()?,
                        distance: input.short()?,
                    },
                ))
            }
            _ => None,
        })
    };
    read(&mut input).ok().flatten()
}

fn udp(kind: u8, secret: Uuid, now_millis: i64) -> Out {
    let mut out = Out::default();
    out.int(UDP_MAGIC);
    out.byte(kind);
    out.uuid(secret);
    out.long(now_millis);
    out
}

/// A keep-alive: the client answers with its own.
pub fn ping(secret: Uuid, now_millis: i64) -> Vec<u8> {
    let mut out = udp(1, secret, now_millis);
    out.long(now_millis);
    out.0
}

/// The sound of a source, with a blank secret: [`set_secret`] addresses it to a listener.
pub fn source_audio(
    now_millis: i64,
    sequence: i64,
    data: &[u8],
    source: Uuid,
    state: u8,
    distance: i16,
) -> Vec<u8> {
    let mut out = udp(3, Uuid::nil(), now_millis);
    out.long(sequence);
    out.bytes(data);
    out.uuid(source);
    out.byte(state);
    out.short(distance);
    out.0
}

/// Addresses a datagram to the client with this secret.
pub fn set_secret(datagram: &mut [u8], secret: Uuid) {
    if let Some(bytes) = datagram.get_mut(UDP_SECRET) {
        bytes.copy_from_slice(secret.as_bytes());
    }
}

/// Tells the speaker their sound went out unchanged, as which source and how far.
pub fn self_audio_info(
    secret: Uuid,
    now_millis: i64,
    source: Uuid,
    sequence: i64,
    distance: i16,
) -> Vec<u8> {
    let mut out = udp(4, secret, now_millis);
    out.uuid(source);
    out.long(sequence);
    out.bool(false);
    out.short(distance);
    out.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player() -> PlayerInfo {
        PlayerInfo {
            id: Uuid::from_u128(7),
            name: "Timur".to_owned(),
            voice_disabled: false,
            microphone_muted: true,
        }
    }

    #[test]
    fn decodes_the_handshake_of_a_client() {
        let mut out = Out::default();
        out.byte(10);
        out.bool(false);
        out.bool(true);
        out.utf("26.3");
        out.utf("2.1.17");
        out.bytes(&[1, 2, 3]);
        assert_eq!(
            Serverbound::decode(&out.0),
            Ok(Serverbound::PlayerInfo {
                voice_disabled: false,
                microphone_muted: true,
                version: "2.1.17".to_owned(),
                public_key: vec![1, 2, 3],
            })
        );
        // Cut short.
        assert_eq!(
            Serverbound::decode(&out.0[..out.0.len() - 1]),
            Err(WireError)
        );
        assert_eq!(Serverbound::decode(&[13, 0, 0]), Ok(Serverbound::Other));
        assert_eq!(Serverbound::decode(&[]), Err(WireError));
    }

    #[test]
    fn connection_packet_bytes() {
        let bytes = connection(Uuid::from_u128(1), "0.0.0.0", 24454);
        let mut expected = vec![1];
        expected.extend_from_slice(&1u128.to_be_bytes());
        expected.extend_from_slice(&[0, 7]);
        expected.extend_from_slice(b"0.0.0.0");
        expected.extend_from_slice(&24454i32.to_be_bytes());
        assert_eq!(bytes, expected);
    }

    #[test]
    fn player_packets_end_with_the_player() {
        let mut tail = Vec::new();
        tail.extend_from_slice(&7u128.to_be_bytes());
        tail.extend_from_slice(&[0, 5]);
        tail.extend_from_slice(b"Timur");
        tail.extend_from_slice(&[0, 0, 1]);
        let update = player_info_update(&player());
        assert_eq!(update[0], 8);
        assert_eq!(&update[1..], tail);
        let info = source_info(Uuid::from_u128(9), 1, &player());
        assert_eq!(info[0], 16);
        assert!(info.ends_with(&tail));
        assert_eq!(&info[1..9], b"\0\x06PLAYER");
    }

    #[test]
    fn proximity_ids_match_java() {
        assert_eq!(
            PROXIMITY_ACTIVATION.to_string(),
            "4aec07ba-d109-3345-9a0a-92022a116cd0"
        );
        assert_eq!(
            PROXIMITY_LINE.to_string(),
            "d9392647-f6b2-332d-9adf-30a6400828d6"
        );
    }

    #[test]
    fn audio_goes_in_and_out() {
        let secret = Uuid::from_u128(0xABCD);
        let mut datagram = udp(2, secret, 1000);
        datagram.long(42);
        datagram.bytes(&[5; 40]);
        datagram.uuid(PROXIMITY_ACTIVATION);
        datagram.short(16);
        datagram.bool(false);
        assert_eq!(
            decode_udp(&datagram.0),
            Some((
                secret,
                UdpServerbound::Audio {
                    sequence: 42,
                    data: vec![5; 40],
                    activation: PROXIMITY_ACTIVATION,
                    distance: 16,
                }
            ))
        );
        assert_eq!(decode_udp(&datagram.0[..30]), None);
        assert_eq!(decode_udp(b"not plasmo voice at all, just noise"), None);

        let listener = Uuid::from_u128(0x1234);
        let mut out = source_audio(2000, 42, &[5; 40], Uuid::from_u128(9), 1, 16);
        set_secret(&mut out, listener);
        assert_eq!(&out[..5], &[0x4e, 0x90, 0x04, 0xe9, 3]);
        assert_eq!(&out[5..21], listener.as_bytes());
        // The header, the sequence, the length and the sound, the source, state, distance.
        assert_eq!(out.len(), 29 + 8 + 4 + 40 + 16 + 1 + 2);
        assert_eq!(&out[out.len() - 3..], &[1, 0, 16]);
    }

    #[test]
    fn ping_is_read_back() {
        let secret = Uuid::from_u128(3);
        assert_eq!(
            decode_udp(&ping(secret, 5)),
            Some((secret, UdpServerbound::Ping))
        );
    }
}
