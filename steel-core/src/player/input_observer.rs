//! A hook for what a client does to things the server itself does not know: entities sent
//! to one player only, and the hotbar wheel while such a thing is shown.

use std::sync::OnceLock;

use steel_utils::Identifier;
use steel_utils::types::InteractionHand;

use super::Player;

/// Sees a player's entity clicks and hotbar changes before the server acts on them.
pub trait PlayerInputObserver: Send + Sync {
    /// The player used entity `entity_id`. `true` if the observer took the click; the
    /// server then does nothing with it.
    fn interact(&self, _player: &Player, _entity_id: i32, _hand: InteractionHand) -> bool {
        false
    }

    /// The player hit entity `entity_id`. `true` if the observer took the hit.
    fn attack(&self, _player: &Player, _entity_id: i32) -> bool {
        false
    }

    /// The player is changing the hotbar slot `from` to `to`; the change goes on after.
    fn carried_item(&self, _player: &Player, _from: u8, _to: u8) {}

    /// The player pressed a dialog button or clicked text with the `custom` action `id`.
    /// Vanilla does nothing with these.
    fn custom_click(&self, _player: &Player, _id: &Identifier) {}
}

static PLAYER_INPUT_OBSERVER: OnceLock<&'static dyn PlayerInputObserver> = OnceLock::new();

/// Installs the player input observer. Returns `false` if one is already installed.
pub fn set_player_input_observer(observer: &'static dyn PlayerInputObserver) -> bool {
    PLAYER_INPUT_OBSERVER.set(observer).is_ok()
}

/// The installed player input observer, if any.
pub(crate) fn player_input_observer() -> Option<&'static dyn PlayerInputObserver> {
    PLAYER_INPUT_OBSERVER.get().copied()
}
