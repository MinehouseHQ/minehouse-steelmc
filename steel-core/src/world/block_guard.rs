//! Lets an extension keep some blocks from players and hoppers: a chest that is the stock of
//! a shop, say.

use std::sync::{Arc, OnceLock};

use steel_utils::BlockPos;

use super::World;
use crate::player::Player;

/// Asked before a player starts to break a block and before a hopper pulls from one.
/// Installed once with [`set_block_guard`]. Both run on the server tick.
pub trait BlockGuard: Send + Sync {
    /// Whether `player` may not break the block at `pos`. The guard tells the player why.
    fn keeps_from_breaking(&self, _player: &Player, _world: &Arc<World>, _pos: BlockPos) -> bool {
        false
    }

    /// Whether hoppers and hopper minecarts may not pull from the container at `pos`.
    fn keeps_from_hoppers(&self, _world: &Arc<World>, _pos: BlockPos) -> bool {
        false
    }
}

static BLOCK_GUARD: OnceLock<&'static dyn BlockGuard> = OnceLock::new();

/// Installs the block guard. Returns `false` if one is already installed.
pub fn set_block_guard(guard: &'static dyn BlockGuard) -> bool {
    BLOCK_GUARD.set(guard).is_ok()
}

/// Whether the installed guard keeps `player` from breaking the block at `pos`.
pub(crate) fn keeps_from_breaking(player: &Player, world: &Arc<World>, pos: BlockPos) -> bool {
    BLOCK_GUARD
        .get()
        .is_some_and(|guard| guard.keeps_from_breaking(player, world, pos))
}

/// Whether the installed guard keeps hoppers from the container at `pos`.
pub(crate) fn keeps_from_hoppers(world: &Arc<World>, pos: BlockPos) -> bool {
    BLOCK_GUARD
        .get()
        .is_some_and(|guard| guard.keeps_from_hoppers(world, pos))
}
