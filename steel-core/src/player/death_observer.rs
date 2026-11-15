//! A hook for a player's death, before anything they carry is dropped.

use std::sync::OnceLock;

use super::Player;

/// Sees a player die before their items and experience are dropped.
pub trait PlayerDeathObserver: Send + Sync {
    /// `player` is dying; what is still in their inventory and experience after this call
    /// is what drops (unless `keepInventory` or spectator mode keeps it).
    fn before_drops(&self, player: &Player);
}

static PLAYER_DEATH_OBSERVER: OnceLock<&'static dyn PlayerDeathObserver> = OnceLock::new();

/// Installs the player death observer. Returns `false` if one is already installed.
pub fn set_player_death_observer(observer: &'static dyn PlayerDeathObserver) -> bool {
    PLAYER_DEATH_OBSERVER.set(observer).is_ok()
}

/// The installed player death observer, if any.
pub(crate) fn player_death_observer() -> Option<&'static dyn PlayerDeathObserver> {
    PLAYER_DEATH_OBSERVER.get().copied()
}
