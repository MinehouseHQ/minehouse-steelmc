//! `MineHouse` server features for Steel: our survival mods, ported from the Fabric server.
//!
//! Everything `MineHouse`-specific lives in this crate so it stays out of the way of the vanilla
//! work in `steel-core`. The modules talk to the site (`/api/mc/*`) with the same signed
//! protocol as the Fabric mods, so the site, the Discord bot and Gate don't change.

use std::sync::Arc;

use steel_core::server::Server;

pub mod auth;
pub mod config;
pub mod console;
pub mod site;

use auth::Auth;
use console::Console;

/// The running `MineHouse` modules of one server.
pub struct Minehouse {
    console: Option<Arc<Console>>,
    /// Kept for the commands and syncs that will use it.
    _auth: Option<Arc<Auth>>,
}

impl Minehouse {
    /// Starts every enabled module. Call once the server is ready for players.
    #[must_use]
    pub fn start(server: &Arc<Server>) -> Self {
        Self {
            console: Console::start(server),
            _auth: Auth::start(server),
        }
    }

    /// Lets the modules report their last state while the server is still up.
    pub async fn shutdown(&self) {
        if let Some(console) = &self.console {
            console.stop().await;
        }
    }
}
