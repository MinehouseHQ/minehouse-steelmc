//! Lets an extension talk to client mods over plugin channels (custom payloads).

use std::sync::Arc;

use steel_utils::Identifier;

use crate::{player::Player, server::Server};

/// Receives the plugin messages of players in the world. Installed once with
/// [`Server::set_plugin_channel_handler`]. Answers go out as
/// `steel_protocol::packets::common::CPluginMessage`.
pub trait PluginChannelHandler: Send + Sync {
    /// Called on the server tick for every custom payload of a player in the play state.
    fn plugin_message(
        &self,
        server: &Arc<Server>,
        player: &Player,
        channel: &Identifier,
        data: &[u8],
    );
}

impl Server {
    /// Installs the plugin channel handler. Returns `false` if one is already installed.
    pub fn set_plugin_channel_handler(&self, handler: Arc<dyn PluginChannelHandler>) -> bool {
        self.plugin_channel_handler.set(handler).is_ok()
    }

    /// The installed plugin channel handler.
    #[must_use]
    pub fn plugin_channel_handler(&self) -> Option<&Arc<dyn PluginChannelHandler>> {
        self.plugin_channel_handler.get()
    }
}
