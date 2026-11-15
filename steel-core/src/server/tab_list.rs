//! Lets an extension draw the tab list: header, footer, names and order.

use std::sync::Arc;

use crate::server::Server;

/// Draws the tab list in place of the built-in header and footer. Installed once with
/// [`Server::set_tab_list_provider`].
pub trait TabListProvider: Send + Sync {
    /// Called on the server tick once a second: send whatever the tab list needs.
    fn refresh(&self, server: &Server);
}

impl Server {
    /// Installs the tab list provider. Returns `false` if one is already installed.
    pub fn set_tab_list_provider(&self, provider: Arc<dyn TabListProvider>) -> bool {
        self.tab_list_provider.set(provider).is_ok()
    }

    /// The player limit of the server list.
    #[must_use]
    pub fn max_players(&self) -> u32 {
        self.config.max_players
    }
}
