//! Lets an extension draw the tab list: header, footer, names and order, and say which game
//! mode of a player the tab list of another player is told.
//!
//! The vanilla client greys a spectator out in the tab list, puts the name in italics and
//! sorts it to the bottom, going by the game mode in the player info packet. An extension can
//! show another mode instead. A player's own entry is never changed: the client takes its own
//! mode from it.

use std::sync::{Arc, OnceLock};

use steel_protocol::packets::game::CPlayerInfoUpdate;
use steel_utils::types::GameType;

use crate::{player::Player, server::Server};

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

/// Lets a content crate change the game mode a tab list shows. With no observer every
/// viewer is told the real one.
pub trait TabModeObserver: Send + Sync {
    /// The game mode `viewer` is told for `subject`, who is another player in `real`.
    fn shown(&self, _viewer: &Player, _subject: &Player, real: GameType) -> GameType {
        real
    }
}

static TAB_MODE_OBSERVER: OnceLock<&'static dyn TabModeObserver> = OnceLock::new();

/// Installs the tab mode observer. Returns `false` if one is already installed.
pub fn set_tab_mode_observer(observer: &'static dyn TabModeObserver) -> bool {
    TAB_MODE_OBSERVER.set(observer).is_ok()
}

/// Whether tab lists can differ between viewers.
pub(crate) fn observed() -> bool {
    TAB_MODE_OBSERVER.get().is_some()
}

/// The game mode the tab list of `viewer` is told for `subject`.
pub(crate) fn shown(viewer: &Player, subject: &Player) -> GameType {
    let real = subject.game_mode();
    if viewer.gameprofile.id() == subject.gameprofile.id() {
        return real;
    }
    TAB_MODE_OBSERVER
        .get()
        .map_or(real, |observer| observer.shown(viewer, subject, real))
}

/// `player` changed game mode: tells them the modes of everyone else again, because what
/// they are shown may go by their own mode.
pub(crate) fn resend_to(player: &Player) {
    if !observed() {
        return;
    }
    let Some(server) = player.try_server() else {
        return;
    };
    let mut others = Vec::new();
    server.online_players.iter_players(|_, other| {
        if other.gameprofile.id() != player.gameprofile.id() {
            others.push((other.gameprofile.id(), shown(player, other)));
        }
        true
    });
    for (uuid, mode) in others {
        player.send_packet(CPlayerInfoUpdate::update_game_mode(uuid, mode as i32));
    }
}
