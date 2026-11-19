//! Spectators look like everyone else in the tab list (the Fabric `TabSpectators`). The
//! vanilla client greys them out, puts them in italics and sorts them to the bottom by the
//! game mode in the player info packet, so players who are not spectators themselves are
//! told survival for everyone else in spectator.
//!
//! Safe because the server never sends a spectator's body to non-spectators. A player's own
//! entry is never touched, and spectators get the real modes, so they still see each other
//! as see-through heads.

use steel_core::{player::Player, server::tab_list::TabModeObserver};
use steel_utils::types::GameType;

/// The observer installed in the core.
pub(super) static TAB_SPECTATORS: TabSpectators = TabSpectators;

/// See the module.
pub(super) struct TabSpectators;

impl TabModeObserver for TabSpectators {
    fn shown(&self, viewer: &Player, _subject: &Player, real: GameType) -> GameType {
        shown(viewer.game_mode(), real)
    }
}

/// The mode a viewer in `viewer` is told for another player in `real`.
const fn shown(viewer: GameType, real: GameType) -> GameType {
    match (viewer, real) {
        (GameType::Spectator, _) => real,
        (_, GameType::Spectator) => GameType::Survival,
        _ => real,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_spectators_are_told_who_spectates() {
        assert_eq!(
            shown(GameType::Survival, GameType::Spectator),
            GameType::Survival
        );
        assert_eq!(
            shown(GameType::Creative, GameType::Spectator),
            GameType::Survival
        );
        assert_eq!(
            shown(GameType::Spectator, GameType::Spectator),
            GameType::Spectator
        );
        assert_eq!(
            shown(GameType::Survival, GameType::Creative),
            GameType::Creative
        );
    }
}
