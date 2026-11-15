//! Lets an extension take over player chat: its own channels, formats and mutes.

use std::sync::Arc;

use crate::{player::Player, server::Server};

/// Sees every chat message before vanilla delivers it. Installed once with
/// [`Server::set_chat_interceptor`].
pub trait ChatInterceptor: Send + Sync {
    /// Called on the server tick for a message that passed the signature checks. `true`
    /// means the interceptor delivered (or dropped) it and vanilla sends nothing.
    fn player_chat(&self, server: &Arc<Server>, player: &Player, message: &str) -> bool;

    /// Whether `player` may send a chat message through a command (`/me`, `/say`).
    /// `false` drops it; telling the player why is up to the interceptor.
    fn allow_command_message(&self, _player: &Player) -> bool {
        true
    }
}

impl Server {
    /// Installs the chat interceptor. Returns `false` if one is already installed.
    pub fn set_chat_interceptor(&self, interceptor: Arc<dyn ChatInterceptor>) -> bool {
        self.chat_interceptor.set(interceptor).is_ok()
    }

    /// The installed chat interceptor.
    #[must_use]
    pub fn chat_interceptor(&self) -> Option<&Arc<dyn ChatInterceptor>> {
        self.chat_interceptor.get()
    }
}
