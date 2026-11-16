//! Voice chat for clients with the Plasmo Voice mod. On the Fabric server this was the
//! Plasmo Voice server mod; here the server speaks its protocol (version 2) itself, so
//! players keep the client mod they have.
//!
//! How a client gets in: once a player is online the server asks on the plugin channel
//! whether the mod is there, the client answers with an RSA key, the server names the UDP
//! address and a secret, and the first datagram with that secret opens the connection: the
//! settings and the list of players in voice chat go back over the plugin channel.
//!
//! The sound is opus, encrypted with an AES key every client gets under its RSA key. The
//! server never decodes it: a datagram from a speaker goes on to everyone within earshot in
//! the same world.
//!
//! Of Plasmo Voice this is proximity voice alone: no groups, no `/vmute`, no addons.

use std::{
    io,
    net::{SocketAddr, UdpSocket as StdUdpSocket},
    ptr,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rsa::Pkcs1v15Encrypt;
use rustc_hash::FxHashMap;
use steel_core::{
    entity::Entity as _,
    player::Player,
    server::{Server, plugin_channels::PluginChannelHandler},
};
use steel_protocol::packets::common::CPluginMessage;
use steel_utils::{Identifier, locks::SyncMutex};
use tokio::{net::UdpSocket, task::JoinHandle, time::sleep};
use uuid::Uuid;

use crate::config;

mod packets;
mod settings;
mod wire;

use packets::{PROXIMITY_ACTIVATION, PlayerInfo, Serverbound, UdpServerbound};
use settings::VoiceConfig;

/// The plugin channel of the client mod.
const CHANNEL: Identifier = Identifier::new_static("plasmo", "voice/v2");
/// How often joins, leaves and keep-alives are looked at.
const TICK: Duration = Duration::from_millis(500);
/// A client that doesn't answer is asked again after these waits, then left alone: it has
/// no mod.
const REQUEST_RETRIES: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(3),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(15),
];
/// The others hear about a muted microphone at most this often.
const STATE_BROADCAST_INTERVAL: Duration = Duration::from_millis(250);
const LANGUAGE_INTERVAL: Duration = Duration::from_secs(1);
/// The state of a player source: it never changes its shape, so it stays at the first one.
const SOURCE_STATE: u8 = 1;

/// Voice chat of one server.
pub struct Voice {
    config: VoiceConfig,
    server_id: Uuid,
    /// The key of the sound. Clients encrypt and decrypt, the server hands it out.
    sound_key: [u8; 16],
    socket: UdpSocket,
    state: SyncMutex<State>,
    tasks: SyncMutex<Vec<JoinHandle<()>>>,
}

#[derive(Default)]
struct State {
    /// Everyone online.
    members: FxHashMap<Uuid, Member>,
    /// Whose datagrams carry which secret.
    by_secret: FxHashMap<Uuid, Uuid>,
}

struct Member {
    player: Arc<Player>,
    /// When the client was last asked for its key, and how many times again.
    asked: Instant,
    retries: usize,
    /// The mod of the client, once it answered.
    client: Option<Client>,
}

struct Client {
    secret: Uuid,
    /// The key of the sound under the RSA key of this client.
    encrypted_key: Vec<u8>,
    voice_disabled: bool,
    microphone_muted: bool,
    /// The UDP connection, once the first datagram came.
    link: Option<Link>,
    /// The voice of this player as a source of sound.
    source: Uuid,
    /// Whether the listeners and the speaker were told who the source is.
    source_announced: bool,
    self_announced: bool,
    state_sent: Instant,
    state_pending: bool,
    language_sent: Option<Instant>,
}

struct Link {
    address: SocketAddr,
    received: Instant,
    next_ping: Instant,
}

impl Member {
    fn link(&self) -> Option<&Link> {
        self.client.as_ref()?.link.as_ref()
    }

    fn info(&self) -> PlayerInfo {
        PlayerInfo {
            id: self.player.gameprofile.id(),
            name: self.player.gameprofile.name().to_owned(),
            voice_disabled: self.client.as_ref().is_some_and(|c| c.voice_disabled),
            microphone_muted: self.client.as_ref().is_some_and(|c| c.microphone_muted),
        }
    }

    fn send(&self, data: &[u8]) {
        send(&self.player, data);
    }
}

fn send(player: &Player, data: &[u8]) {
    player.send_packet(CPluginMessage::new(CHANNEL, data.into()));
}

impl State {
    /// Sends a plugin message to everyone in voice chat.
    fn broadcast(&self, data: &[u8]) {
        for member in self.members.values() {
            if member.link().is_some() {
                member.send(data);
            }
        }
    }

    /// Who hears a voice that carries `distance` blocks from `speaker`: in voice chat, with
    /// sound on, in the same world, close enough.
    fn listeners(
        &self,
        speaker: Uuid,
        reach: f64,
    ) -> impl Iterator<Item = (&Member, &Client, &Link)> {
        let origin = self
            .members
            .get(&speaker)
            .map(|member| (member.player.get_world(), member.player.position()));
        self.members.iter().filter_map(move |(&id, member)| {
            let (world, position) = origin.as_ref()?;
            let client = member.client.as_ref()?;
            let link = client.link.as_ref()?;
            let hears = id != speaker
                && !client.voice_disabled
                && member.player.get_world().key == world.key
                && member.player.position().distance_squared(*position) <= reach * reach;
            hears.then_some((member, client, link))
        })
    }
}

impl Voice {
    /// Loads the settings, opens the UDP port and starts voice chat on `server`.
    #[must_use]
    pub fn start(server: &Arc<Server>) -> Option<Arc<Self>> {
        let config: VoiceConfig = match config::load(&config::path("voice")) {
            Ok(config) => config,
            Err(error) => {
                log::error!("MineHouse voice chat is off: {error}");
                return None;
            }
        };
        if !config.enabled {
            log::info!("MineHouse voice chat is off in config/minehouse/voice.toml");
            return None;
        }
        let socket = match bind(&config.host, config.port) {
            Ok(socket) => socket,
            Err(error) => {
                log::error!(
                    "MineHouse voice chat is off: UDP {}:{}: {error}",
                    config.host,
                    config.port
                );
                return None;
            }
        };
        let voice = Arc::new(Self {
            server_id: config.resolve_server_id(),
            config,
            sound_key: rand::random(),
            socket,
            state: SyncMutex::new(State::default()),
            tasks: SyncMutex::new(Vec::new()),
        });
        server.add_plugin_channel_handler(Arc::clone(&voice) as Arc<dyn PluginChannelHandler>);
        let tasks = vec![
            tokio::spawn(Arc::clone(&voice).receive()),
            tokio::spawn(Arc::clone(&voice).watch(Arc::clone(server))),
        ];
        *voice.tasks.lock() = tasks;
        log::info!(
            "MineHouse voice chat on: UDP {}:{}",
            voice.config.host,
            voice.config.port
        );
        Some(voice)
    }

    /// Stops voice chat.
    pub fn stop(&self) {
        for task in self.tasks.lock().drain(..) {
            task.abort();
        }
    }

    /// Reads the datagrams of the clients.
    async fn receive(self: Arc<Self>) {
        let mut buffer = [0u8; 4096];
        loop {
            match self.socket.recv_from(&mut buffer).await {
                Ok((length, address)) => {
                    let datagram = buffer.get(..length).unwrap_or_default();
                    if let Some((secret, packet)) = packets::decode_udp(datagram) {
                        let out = self.datagram(address, secret, packet);
                        self.send_datagrams(&out);
                    }
                }
                Err(error) => {
                    log::debug!("Voice UDP receive failed: {error}");
                    sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }

    /// Follows who is online and keeps the UDP connections alive.
    async fn watch(self: Arc<Self>, server: Arc<Server>) {
        loop {
            sleep(TICK).await;
            let out = self.tick(&server.get_players(), Instant::now());
            self.send_datagrams(&out);
        }
    }

    fn send_datagrams(&self, datagrams: &[(SocketAddr, Vec<u8>)]) {
        for (address, data) in datagrams {
            // A full buffer drops the datagram: sound that waits is of no use.
            if let Err(error) = self.socket.try_send_to(data, *address) {
                log::trace!("Voice UDP send to {address} failed: {error}");
            }
        }
    }

    /// One datagram of a client: the first opens its connection, the next are keep-alives
    /// and sound. Returns the datagrams to send.
    fn datagram(
        &self,
        address: SocketAddr,
        secret: Uuid,
        packet: UdpServerbound,
    ) -> Vec<(SocketAddr, Vec<u8>)> {
        let now = Instant::now();
        let mut state = self.state.lock();
        let Some(&id) = state.by_secret.get(&secret) else {
            return Vec::new();
        };
        let Some(client) = state
            .members
            .get_mut(&id)
            .and_then(|member| member.client.as_mut())
        else {
            return Vec::new();
        };
        if let Some(link) = &mut client.link {
            link.address = address;
            link.received = now;
        } else {
            client.link = Some(Link {
                address,
                received: now,
                next_ping: now,
            });
            self.connected(&state, id);
            return Vec::new();
        }
        match packet {
            UdpServerbound::Ping => Vec::new(),
            UdpServerbound::Audio {
                sequence,
                data,
                activation,
                distance,
            } => self.relay(&mut state, id, sequence, &data, activation, distance),
        }
    }

    /// A client opened its UDP connection: it gets the settings and the players in voice
    /// chat, and everyone hears about it.
    fn connected(&self, state: &State, id: Uuid) {
        let Some(member) = state.members.get(&id) else {
            return;
        };
        let Some(client) = &member.client else {
            return;
        };
        log::info!("{} joined voice chat", member.player.gameprofile.name());
        let settings = packets::Settings {
            server_id: self.server_id,
            sample_rate: self.config.sample_rate,
            mtu_size: self.config.mtu_size,
            opus_mode: &self.config.opus_mode,
            opus_bitrate: self.config.opus_bitrate,
            distances: &self.config.distances,
            default_distance: self.config.default_distance,
            icon_visibility: &self.config.player_icon_visibility,
            icon_y_offset: self.config.player_icon_y_offset,
        };
        member.send(&packets::config(&settings, &client.encrypted_key));
        let players: Vec<PlayerInfo> = state
            .members
            .values()
            .filter(|member| member.link().is_some())
            .map(Member::info)
            .collect();
        member.send(&packets::player_list(&players));
        state.broadcast(&packets::player_info_update(&member.info()));
    }

    /// Passes 20 ms of a voice on to everyone within earshot.
    fn relay(
        &self,
        state: &mut State,
        speaker: Uuid,
        sequence: i64,
        data: &[u8],
        activation: Uuid,
        distance: i16,
    ) -> Vec<(SocketAddr, Vec<u8>)> {
        let Some(member) = state.members.get(&speaker) else {
            return Vec::new();
        };
        let (Some(client), Some(link)) = (&member.client, member.link()) else {
            return Vec::new();
        };
        if client.microphone_muted || activation != PROXIMITY_ACTIVATION {
            return Vec::new();
        }
        let distance = self.config.allowed_distance(distance);
        let now = now_millis();
        let template =
            packets::source_audio(now, sequence, data, client.source, SOURCE_STATE, distance);
        let source_info = (!client.source_announced)
            .then(|| packets::source_info(client.source, SOURCE_STATE, &member.info()));
        let mut out = vec![(
            link.address,
            packets::self_audio_info(client.secret, now, client.source, sequence, distance),
        )];
        if !client.self_announced {
            member.send(&packets::self_source_info(
                client.source,
                SOURCE_STATE,
                &member.info(),
            ));
        }
        let reach = self.config.listeners_distance(distance);
        for (listener, listener_client, listener_link) in state.listeners(speaker, reach) {
            if let Some(source_info) = &source_info {
                listener.send(source_info);
            }
            let mut datagram = template.clone();
            packets::set_secret(&mut datagram, listener_client.secret);
            out.push((listener_link.address, datagram));
        }
        if let Some(client) = state
            .members
            .get_mut(&speaker)
            .and_then(|member| member.client.as_mut())
        {
            client.source_announced = true;
            client.self_announced = true;
        }
        out
    }

    /// Joins, leaves, repeated questions, keep-alives. Returns the datagrams to send.
    fn tick(&self, online: &[Arc<Player>], now: Instant) -> Vec<(SocketAddr, Vec<u8>)> {
        let timeout = Duration::from_millis(self.config.keep_alive_timeout_ms);
        let mut state = self.state.lock();

        let online_by_id: FxHashMap<Uuid, &Arc<Player>> = online
            .iter()
            .map(|player| (player.gameprofile.id(), player))
            .collect();
        // A player who left, or joined again as a new connection.
        let gone: Vec<Uuid> = state
            .members
            .iter()
            .filter(|(id, member)| {
                !online_by_id
                    .get(id)
                    .is_some_and(|player| Arc::ptr_eq(player, &member.player))
            })
            .map(|(&id, _)| id)
            .collect();
        for id in gone {
            Self::forget(&mut state, id);
        }
        for player in online {
            state
                .members
                .entry(player.gameprofile.id())
                .or_insert_with(|| {
                    send(player, &packets::player_info_request());
                    Member {
                        player: Arc::clone(player),
                        asked: now,
                        retries: 0,
                        client: None,
                    }
                });
        }

        let mut out = Vec::new();
        let mut timed_out = Vec::new();
        let mut changed = Vec::new();
        for (&id, member) in &mut state.members {
            let Some(client) = &mut member.client else {
                let wait = REQUEST_RETRIES.get(member.retries);
                if wait.is_some_and(|&wait| now.duration_since(member.asked) >= wait) {
                    member.asked = now;
                    member.retries += 1;
                    member.send(&packets::player_info_request());
                }
                continue;
            };
            let Some(link) = &mut client.link else {
                continue;
            };
            if now.duration_since(link.received) > timeout {
                timed_out.push(id);
                continue;
            }
            if now >= link.next_ping {
                link.next_ping = now + Duration::from_millis(rand::random_range(2500..4000));
                out.push((link.address, packets::ping(client.secret, now_millis())));
            }
            if client.state_pending {
                client.state_pending = false;
                client.state_sent = now;
                changed.push(id);
            }
        }
        for id in changed {
            if let Some(member) = state.members.get(&id) {
                state.broadcast(&packets::player_info_update(&member.info()));
            }
        }
        for id in timed_out {
            let Some(member) = state.members.get_mut(&id) else {
                continue;
            };
            log::info!(
                "{} left voice chat: no answer over UDP",
                member.player.gameprofile.name()
            );
            // Start over: the client answers the question with a new key.
            let secret = member.client.take().map(|client| client.secret);
            member.asked = now;
            member.retries = 0;
            member.send(&packets::player_info_request());
            if let Some(secret) = secret {
                state.by_secret.remove(&secret);
            }
            state.broadcast(&packets::player_disconnect(id));
        }
        out
    }

    /// A player went offline.
    fn forget(state: &mut State, id: Uuid) {
        let Some(member) = state.members.remove(&id) else {
            return;
        };
        let Some(client) = member.client else {
            return;
        };
        state.by_secret.remove(&client.secret);
        if client.link.is_some() {
            state.broadcast(&packets::player_disconnect(id));
        }
    }

    /// The client has the mod and sent its key: it gets the UDP address and a secret.
    fn client_info(
        &self,
        player: &Player,
        voice_disabled: bool,
        microphone_muted: bool,
        version: &str,
        public_key: &[u8],
    ) {
        let name = player.gameprofile.name();
        if !self.config.accepts_version(version) {
            log::info!("{name} has Plasmo Voice {version}: not supported, no voice chat");
            // No use asking again.
            if let Some(member) = member_of(&mut self.state.lock().members, player) {
                member.retries = REQUEST_RETRIES.len();
            }
            return;
        }
        let encrypted_key = steel_crypto::public_key_from_bytes(public_key)
            .map_err(|error| error.to_string())
            .and_then(|key| {
                key.encrypt(&mut rand::rng(), Pkcs1v15Encrypt, &self.sound_key)
                    .map_err(|error| error.to_string())
            });
        let encrypted_key = match encrypted_key {
            Ok(encrypted_key) => encrypted_key,
            Err(error) => {
                log::warn!("{name} sent a bad voice chat key: {error}");
                return;
            }
        };

        let mut state = self.state.lock();
        let Some(member) = member_of(&mut state.members, player) else {
            return;
        };
        let now = Instant::now();
        let secret = Uuid::new_v4();
        let old = member.client.replace(Client {
            secret,
            encrypted_key,
            voice_disabled,
            microphone_muted,
            link: None,
            source: Uuid::new_v4(),
            source_announced: false,
            self_announced: false,
            state_sent: now,
            state_pending: false,
            language_sent: None,
        });
        let host = match self.config.public_host.trim() {
            "" => "0.0.0.0",
            host => host,
        };
        let port = match self.config.public_port {
            0 => self.config.port,
            port => port,
        };
        member.send(&packets::connection(secret, host, port));
        if let Some(old) = old {
            state.by_secret.remove(&old.secret);
        }
        state.by_secret.insert(secret, player.gameprofile.id());
    }

    /// The client turned its sound or microphone on or off.
    fn client_state(&self, player: &Player, voice_disabled: bool, microphone_muted: bool) {
        let mut state = self.state.lock();
        let Some(member) = member_of(&mut state.members, player) else {
            return;
        };
        let Some(client) = &mut member.client else {
            return;
        };
        if client.voice_disabled == voice_disabled && client.microphone_muted == microphone_muted {
            return;
        }
        client.voice_disabled = voice_disabled;
        client.microphone_muted = microphone_muted;
        if client.link.is_none() {
            return;
        }
        let now = Instant::now();
        if now.duration_since(client.state_sent) < STATE_BROADCAST_INTERVAL {
            // The watch sends it within half a second.
            client.state_pending = true;
            return;
        }
        client.state_sent = now;
        client.state_pending = false;
        let update = packets::player_info_update(&member.info());
        state.broadcast(&update);
    }

    /// The player stopped talking: the listeners close the source.
    fn audio_end(&self, player: &Player, sequence: i64, activation: Uuid, distance: i16) {
        let state = self.state.lock();
        let id = player.gameprofile.id();
        let Some(member) = state.members.get(&id) else {
            return;
        };
        let Some(client) = &member.client else {
            return;
        };
        if client.link.is_none() || client.microphone_muted || activation != PROXIMITY_ACTIVATION {
            return;
        }
        let end = packets::source_audio_end(client.source, sequence);
        member.send(&end);
        let reach = self
            .config
            .listeners_distance(self.config.allowed_distance(distance));
        for (listener, _, _) in state.listeners(id, reach) {
            listener.send(&end);
        }
    }

    /// A client hears a source it doesn't know yet.
    fn source_info_request(&self, player: &Player, source: Uuid) {
        let state = self.state.lock();
        let asks = state
            .members
            .get(&player.gameprofile.id())
            .is_some_and(|member| member.link().is_some());
        if !asks {
            return;
        }
        let speaker = state.members.values().find(|member| {
            member
                .client
                .as_ref()
                .is_some_and(|client| client.source == source && client.link.is_some())
        });
        if let Some(speaker) = speaker {
            send(
                player,
                &packets::source_info(source, SOURCE_STATE, &speaker.info()),
            );
        }
    }

    /// The texts the client shows for what the server named.
    fn language_request(&self, player: &Player, language: &str) {
        let mut state = self.state.lock();
        let Some(member) = member_of(&mut state.members, player) else {
            return;
        };
        let Some(client) = &mut member.client else {
            return;
        };
        let now = Instant::now();
        if client
            .language_sent
            .is_some_and(|sent| now.duration_since(sent) < LANGUAGE_INTERVAL)
        {
            return;
        }
        client.language_sent = Some(now);
        let proximity = if language.eq_ignore_ascii_case("ru_ru") {
            "Рядом"
        } else {
            "Proximity"
        };
        member.send(&packets::language(
            language,
            &[("pv.activation.proximity", proximity)],
        ));
    }
}

impl PluginChannelHandler for Voice {
    fn plugin_message(
        &self,
        _server: &Arc<Server>,
        player: &Player,
        channel: &Identifier,
        data: &[u8],
    ) {
        if *channel != CHANNEL {
            return;
        }
        let Ok(packet) = Serverbound::decode(data) else {
            log::debug!(
                "{} sent a broken voice chat packet",
                player.gameprofile.name()
            );
            return;
        };
        match packet {
            Serverbound::PlayerInfo {
                voice_disabled,
                microphone_muted,
                version,
                public_key,
            } => self.client_info(
                player,
                voice_disabled,
                microphone_muted,
                &version,
                &public_key,
            ),
            Serverbound::PlayerState {
                voice_disabled,
                microphone_muted,
            } => self.client_state(player, voice_disabled, microphone_muted),
            Serverbound::AudioEnd {
                sequence,
                activation,
                distance,
            } => self.audio_end(player, sequence, activation, distance),
            Serverbound::SourceInfoRequest { source } => self.source_info_request(player, source),
            Serverbound::LanguageRequest { language } => self.language_request(player, &language),
            Serverbound::Other => {}
        }
    }
}

/// The member of this very connection: not of an earlier one of the same player.
fn member_of<'members>(
    members: &'members mut FxHashMap<Uuid, Member>,
    player: &Player,
) -> Option<&'members mut Member> {
    members
        .get_mut(&player.gameprofile.id())
        .filter(|member| ptr::eq(Arc::as_ptr(&member.player), player))
}

fn bind(host: &str, port: u16) -> io::Result<UdpSocket> {
    let socket = StdUdpSocket::bind((host, port))?;
    socket.set_nonblocking(true)?;
    UdpSocket::from_std(socket)
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or_default()
}
