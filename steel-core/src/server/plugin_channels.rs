//! Lets an extension talk to client mods over plugin channels (custom payloads).

use std::sync::Arc;

use steel_utils::Identifier;

use crate::{player::Player, server::Server};

/// Receives the plugin messages of players in the world. Added with
/// [`Server::add_plugin_channel_handler`]; every handler gets every message and picks the
/// channels it knows. Answers go out as
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
    /// Adds a plugin channel handler next to the ones already there.
    pub fn add_plugin_channel_handler(&self, handler: Arc<dyn PluginChannelHandler>) {
        self.plugin_channel_handlers.write().push(handler);
    }

    /// The plugin channel handlers, in the order they were added.
    #[must_use]
    pub fn plugin_channel_handlers(&self) -> Vec<Arc<dyn PluginChannelHandler>> {
        self.plugin_channel_handlers.read().clone()
    }
}
