//! Runs the server's login gate ([`steel_core::server::login_gate`]) for a client in the
//! configuration phase.

use std::{
    net::IpAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use steel_core::{
    player::{GameProfile, connection::OutboundPacket},
    server::login_gate::{LoginConnection, LoginDecision, PendingLogin},
};
use steel_protocol::{
    packet_traits::EncodedPacket,
    packets::{
        common::{CDisconnect, CKeepAlive, SCustomClickAction},
        config::CFinishConfiguration,
    },
    utils::ConnectionProtocol,
};
use steel_registry::game_profile::ProfileProperty;
use steel_utils::Identifier;
use tokio::{
    sync::mpsc::{self, UnboundedSender},
    time::{MissedTickBehavior, interval},
};

use crate::tcp_client::JavaTcpClient;

/// Vanilla pings a configuring client every 15 seconds; the client gives up after 30
/// seconds of silence.
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// Where a client stands with the login gate.
#[derive(Default)]
pub(crate) struct GateState {
    /// Properties taken out of the forwarded profile for the gate.
    private_properties: Vec<ProfileProperty>,
    /// Set while the gate is deciding: the client may not finish the configuration yet.
    undecided: Option<Arc<AtomicBool>>,
    /// Custom click actions go here while the gate is deciding.
    clicks: Option<UnboundedSender<Identifier>>,
}

impl JavaTcpClient {
    /// The player's address: the one the proxy forwarded, or the socket's.
    pub(crate) fn player_address(&self) -> IpAddr {
        self.forwarded_address
            .load()
            .unwrap_or_else(|| self.address.ip())
    }

    /// Takes the gate's private properties out of a forwarded profile and keeps them for
    /// [`Self::start_login_gate`].
    pub(crate) fn take_private_properties(&self, profile: GameProfile) -> GameProfile {
        let Some(gate) = self.server.login_gate() else {
            return profile;
        };
        let private = gate.private_properties();
        if !profile
            .properties()
            .iter()
            .any(|property| private.contains(&property.name()))
        {
            return profile;
        }
        let (taken, kept): (Vec<_>, Vec<_>) = profile
            .properties()
            .iter()
            .cloned()
            .partition(|property| private.contains(&property.name()));
        match GameProfile::new(profile.id(), profile.name().to_owned(), kept) {
            Ok(stripped) => {
                self.gate.lock().private_properties = taken;
                stripped
            }
            // Removing properties can't make a valid profile invalid.
            Err(_) => profile,
        }
    }

    /// Hands the client to the login gate, if the server has one. Returns `false` when
    /// there is no gate and the caller finishes the configuration itself.
    pub(crate) fn start_login_gate(&self) -> bool {
        let Some(gate) = self.server.login_gate() else {
            return false;
        };
        let Some(profile) = self.pre_play_state.lock().configuring_profile() else {
            return false;
        };

        let (clicks, click_receiver) = mpsc::unbounded_channel();
        let undecided = Arc::new(AtomicBool::new(true));
        let private_properties = {
            let mut state = self.gate.lock();
            state.undecided = Some(Arc::clone(&undecided));
            state.clicks = Some(clicks);
            state.private_properties.clone()
        };

        let compression = self.compression.load();
        let admission = gate.admit(PendingLogin {
            profile,
            address: self.player_address(),
            private_properties,
            connection: LoginConnection::new(
                self.outgoing_queue.clone(),
                compression,
                self.cancel_token.clone(),
                click_receiver,
            ),
        });

        let outgoing = self.outgoing_queue.clone();
        let cancel_token = self.cancel_token.clone();
        let id = self.id;
        let encode = move |packet: Result<EncodedPacket, _>| match packet {
            Ok(packet) => Some(packet),
            Err(error) => {
                log::error!("Could not encode a configuration packet for client {id}: {error}");
                None
            }
        };
        // The gate may wait for minutes (a code typed on the site): it runs beside the
        // packet reader, which keeps taking keep-alive answers and button clicks.
        self.task_tracker.spawn(async move {
            tokio::pin!(admission);
            let mut keep_alive = interval(KEEP_ALIVE_INTERVAL);
            keep_alive.set_missed_tick_behavior(MissedTickBehavior::Delay);
            keep_alive.tick().await;

            let decision = loop {
                tokio::select! {
                    biased;
                    () = cancel_token.cancelled() => return,
                    decision = &mut admission => break decision,
                    _ = keep_alive.tick() => {
                        let packet = EncodedPacket::from_bare(
                            CKeepAlive::new(rand::random()),
                            compression,
                            ConnectionProtocol::Config,
                        );
                        if let Some(packet) = encode(packet) {
                            let _ = outgoing.send(OutboundPacket::Packet(packet));
                        }
                    }
                }
            };

            let outbound = match decision {
                LoginDecision::Allow => {
                    // Before the packet: the client answers it at once.
                    undecided.store(false, Ordering::Release);
                    encode(EncodedPacket::from_bare(
                        CFinishConfiguration {},
                        compression,
                        ConnectionProtocol::Config,
                    ))
                    .map(OutboundPacket::Packet)
                }
                LoginDecision::Deny(reason) => {
                    log::info!("Kicking client {id}: {reason:p}");
                    encode(EncodedPacket::from_bare(
                        CDisconnect { reason: *reason },
                        compression,
                        ConnectionProtocol::Config,
                    ))
                    .map(OutboundPacket::Disconnect)
                }
            };
            if outbound.is_none_or(|outbound| outgoing.send(outbound).is_err()) {
                cancel_token.cancel();
            }
        });
        true
    }

    /// Whether the client may finish the configuration: there is no gate, or it allowed
    /// the client. `false` means the client finished before the gate decided.
    pub(crate) fn login_gate_passed(&self) -> bool {
        let mut state = self.gate.lock();
        if state
            .undecided
            .as_ref()
            .is_some_and(|undecided| undecided.load(Ordering::Acquire))
        {
            return false;
        }
        state.clicks = None;
        true
    }

    /// Passes a dialog button click to the waiting gate.
    pub(crate) fn handle_config_custom_click(&self, packet: SCustomClickAction) {
        if let Some(clicks) = &self.gate.lock().clicks {
            let _ = clicks.send(packet.id);
        }
    }
}
