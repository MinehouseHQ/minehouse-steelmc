//! The resource pack task of the configuration phase: the server's pack is offered after
//! the registries, and the client joins the world once it gave its last word about it.

use steel_core::player::connection::OutboundPacket;
use steel_protocol::{
    packet_traits::EncodedPacket,
    packets::common::{CKeepAlive, CResourcePackPush, ResourcePackAction, SResourcePack},
    utils::ConnectionProtocol,
};
use steel_utils::translations;
use text_components::TextComponent;
use tokio::time::{MissedTickBehavior, interval};

use crate::{
    handlers::gate::KEEP_ALIVE_INTERVAL,
    pre_play_state::{ConfigurationPhase, PrePlayPhase},
    tcp_client::{ConnectionAction, JavaTcpClient},
};

impl JavaTcpClient {
    /// Offers the server's resource pack. The client answers with [`SResourcePack`]
    /// packets while it downloads and applies the pack.
    pub(crate) async fn offer_resource_pack(&self, pack: CResourcePackPush) {
        self.send_bare_packet_now(pack).await;

        // A download can outlast the 30 seconds of silence a client tolerates.
        let outgoing = self.outgoing_queue.clone();
        let cancel_token = self.cancel_token.clone();
        let answered = self.resource_pack_wait.clone();
        let compression = self.compression.load();
        let id = self.id;
        self.task_tracker.spawn(async move {
            let mut keep_alive = interval(KEEP_ALIVE_INTERVAL);
            keep_alive.set_missed_tick_behavior(MissedTickBehavior::Delay);
            keep_alive.tick().await;
            loop {
                tokio::select! {
                    biased;
                    () = cancel_token.cancelled() => return,
                    () = answered.cancelled() => return,
                    _ = keep_alive.tick() => {}
                }
                match EncodedPacket::from_bare(
                    CKeepAlive::new(rand::random()),
                    compression,
                    ConnectionProtocol::Config,
                ) {
                    Ok(packet) => {
                        if outgoing.send(OutboundPacket::Packet(packet)).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        log::error!("Could not encode a keep-alive for client {id}: {error}");
                    }
                }
            }
        });
    }

    /// Handles the client's progress with the offered pack. Vanilla
    /// `ServerConfigurationPacketListenerImpl.handleResourcePackResponse`.
    pub(crate) async fn handle_resource_pack(&self, packet: SResourcePack) -> ConnectionAction {
        log::debug!("Resource pack packet: {packet:?}");
        if packet.action == ResourcePackAction::Declined
            && self.server.config.resource_pack_required()
        {
            log::info!(
                "Disconnecting client {} due to resource pack {} rejection",
                self.id,
                packet.id
            );
            self.kick(TextComponent::translated(
                translations::MULTIPLAYER_REQUIRED_TEXTURE_PROMPT_DISCONNECT.msg(),
            ))
            .await;
            return ConnectionAction::none();
        }
        if !packet.action.is_terminal() {
            return ConnectionAction::none();
        }

        // Behind Gate the client also answers packs that the proxy handles itself, so a last
        // word about another pack, or one after the task ended, is left alone. Vanilla throws
        // here, which with a proxy keeps every player with a pack out.
        // TODO: vanilla has no proxy in between; drop this once Gate stops relaying those answers.
        let offered = self
            .server
            .config
            .resource_pack_packet()
            .map(|pack| pack.id);
        let in_task = self.pre_play_state.lock().phase()
            == PrePlayPhase::Configuration(ConfigurationPhase::ResourcePack);
        if offered != Some(packet.id) || !in_task {
            log::info!(
                "Client {} answered {:?} about resource pack {} outside the pack task; ignored",
                self.id,
                packet.action,
                packet.id
            );
            return ConnectionAction::none();
        }

        // Only the last word ends the task, and only while that task runs.
        let sequence_result = self.pre_play_state.lock().resource_pack_answered();
        if let Err(error) = sequence_result {
            return self.reject_unexpected_packet(error).await;
        }
        self.resource_pack_wait.cancel();
        self.join_world().await;
        ConnectionAction::none()
    }
}
