//! A hook for what a client does to things the server itself does not know: entities sent
//! to one player only, and the hotbar wheel while such a thing is shown.

use std::sync::{Arc, OnceLock};

use steel_registry::items::item::BlockHitResult;
use steel_utils::Identifier;
use steel_utils::types::InteractionHand;

use super::Player;
use crate::{behavior::InteractionResult, world::World};

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

    /// The player (not a spectator) used the item in `hand` on the block at `hit`, before
    /// the block or the item is asked. `Some` if the observer took the click, with what
    /// came of it; vanilla then does nothing with it.
    fn use_on_block(
        &self,
        _player: &Arc<Player>,
        _world: &Arc<World>,
        _hand: InteractionHand,
        _hit: &BlockHitResult,
    ) -> Option<InteractionResult> {
        None
    }

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
