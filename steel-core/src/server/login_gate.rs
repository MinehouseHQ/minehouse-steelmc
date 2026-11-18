//! A hook that holds a verified login in the configuration phase until an embedder lets it in.
//!
//! Vanilla has no such step: with no gate installed the configuration phase is unchanged.

use std::{fs, future::Future, net::IpAddr, pin::Pin, sync::Arc};

use steel_protocol::{
    packet_traits::{ClientPacket, CompressionInfo, EncodedPacket},
    utils::ConnectionProtocol,
};
use steel_registry::game_profile::{GameProfile, ProfileProperty};
use steel_utils::Identifier;
use text_components::TextComponent;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

use crate::{player::connection::OutboundPacket, server::Server};

/// What a [`LoginGate`] decided about a login.
#[derive(Debug)]
pub enum LoginDecision {
    /// The configuration phase finishes and the player joins.
    Allow,
    /// The player joins with this profile: the one of the login with other properties (a
    /// skin). A profile with another UUID or name is ignored.
    AllowAs(Box<GameProfile>),
    /// The client is disconnected with this text.
    Deny(Box<TextComponent>),
}

/// The client of a login that waits at the gate, still in the configuration protocol.
pub struct LoginConnection {
    outgoing: UnboundedSender<OutboundPacket>,
    compression: Option<CompressionInfo>,
    closed: CancellationToken,
    clicks: UnboundedReceiver<Identifier>,
}

impl LoginConnection {
    /// Builds the gate's view of a connection. `clicks` gets the id of every custom click
    /// action the client sends while it waits.
    #[must_use]
    pub const fn new(
        outgoing: UnboundedSender<OutboundPacket>,
        compression: Option<CompressionInfo>,
        closed: CancellationToken,
        clicks: UnboundedReceiver<Identifier>,
    ) -> Self {
        Self {
            outgoing,
            compression,
            closed,
            clicks,
        }
    }

    /// Queues a configuration packet. Returns `false` when the client is gone or the packet
    /// can't be encoded.
    pub fn send<P: ClientPacket>(&self, packet: P) -> bool {
        match EncodedPacket::from_bare(packet, self.compression, ConnectionProtocol::Config) {
            Ok(packet) => self.outgoing.send(OutboundPacket::Packet(packet)).is_ok(),
            Err(error) => {
                log::error!("Could not encode a configuration packet: {error}");
                false
            }
        }
    }

    /// Whether the client has disconnected.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.is_cancelled()
    }

    /// Completes when the client disconnects.
    pub async fn closed(&self) {
        self.closed.cancelled().await;
    }

    /// The id of the next custom click action (a dialog button), or `None` once the
    /// connection is gone.
    pub async fn next_click(&mut self) -> Option<Identifier> {
        self.clicks.recv().await
    }
}

/// A verified login waiting at the gate.
pub struct PendingLogin {
    /// The profile the player joins with.
    pub profile: GameProfile,
    /// The player's address: the one the proxy forwarded, or the socket's.
    pub address: IpAddr,
    /// Properties named by [`LoginGate::private_properties`] that the proxy sent along.
    pub private_properties: Vec<ProfileProperty>,
    /// The waiting client.
    pub connection: LoginConnection,
}

/// The future [`LoginGate::admit`] returns.
pub type LoginAdmission = Pin<Box<dyn Future<Output = LoginDecision> + Send>>;

/// Decides which verified logins may join. Installed once with [`Server::set_login_gate`].
pub trait LoginGate: Send + Sync {
    /// Names of profile properties a proxy sends for the gate alone. They are taken out of
    /// the profile before the client or other players see it and handed to
    /// [`LoginGate::admit`] in [`PendingLogin::private_properties`].
    fn private_properties(&self) -> &[&str] {
        &[]
    }

    /// Called in the login phase, before an online player with the same UUID would be
    /// kicked to make room. A text refuses the new login instead.
    fn refuse_login(
        &self,
        _server: &Server,
        _profile: &GameProfile,
        _address: IpAddr,
    ) -> Option<TextComponent> {
        None
    }

    /// Called in the configuration phase once registries and tags are sent. The client
    /// stays there, kept alive, until the future completes; dropping the future means the
    /// client left.
    fn admit(&self, login: PendingLogin) -> LoginAdmission;
}

impl Server {
    /// Installs the login gate. Returns `false` if one is already installed.
    pub fn set_login_gate(&self, gate: Arc<dyn LoginGate>) -> bool {
        self.login_gate.set(gate).is_ok()
    }

    /// Replaces the MOTD of the server list until `None` puts the configured one back: a
    /// notice while the server is closed, for example.
    pub fn set_status_motd(&self, motd: Option<String>) {
        *self.status_motd.lock() = motd;
    }

    /// The MOTD for the server list.
    #[must_use]
    pub fn status_motd(&self) -> String {
        self.status_motd
            .lock()
            .clone()
            .unwrap_or_else(|| self.config.motd.clone())
    }

    /// The PNG server icon from the configured favicon file, when enabled and
    /// readable.
    #[must_use]
    pub fn favicon_bytes(&self) -> Option<Vec<u8>> {
        if !self.config.use_favicon {
            return None;
        }
        fs::read(&self.config.favicon).ok()
    }

    /// The installed login gate.
    #[must_use]
    pub fn login_gate(&self) -> Option<&Arc<dyn LoginGate>> {
        self.login_gate.get()
    }
}
