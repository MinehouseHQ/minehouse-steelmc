//! Lets an extension keep a log of what players change in the world: blocks broken and
//! placed, and what went in and out of containers.

use std::sync::Arc;

use steel_utils::{BlockPos, BlockStateId};

use crate::{inventory::menu::Menu, player::Player, server::Server, world::World};

/// Hears about the world changes of players. Installed once with [`Server::set_world_log`].
///
/// Every method runs on the server tick, so an implementation must only queue the event and
/// return: storage and lookups belong on other threads.
pub trait WorldLog: Send + Sync {
    /// A player broke the block `state` at `pos`.
    fn block_broken(&self, player: &Player, world: &Arc<World>, pos: BlockPos, state: BlockStateId);

    /// A player placed `placed` at `pos`, where `replaced` was.
    fn block_placed(
        &self,
        player: &Player,
        world: &Arc<World>,
        pos: BlockPos,
        replaced: BlockStateId,
        placed: BlockStateId,
    );

    /// A player hit (`hit`) or used the block at `pos`. Returning `true` swallows the click:
    /// nothing is broken, placed or opened.
    fn block_clicked(
        &self,
        player: &Arc<Player>,
        world: &Arc<World>,
        pos: BlockPos,
        hit: bool,
    ) -> bool;

    /// A menu was opened for a player. [`Menu::block_containers`] has its contents.
    fn menu_opened(&self, player: &Player, menu: &Menu);

    /// The menu of a player is closing, after its items went back to the player.
    fn menu_closed(&self, player: &Player, menu: &Menu);
}

impl Server {
    /// Installs the world log. Returns `false` if one is already installed.
    pub fn set_world_log(&self, log: Arc<dyn WorldLog>) -> bool {
        self.world_log.set(log).is_ok()
    }

    /// The installed world log.
    #[must_use]
    pub fn world_log(&self) -> Option<&Arc<dyn WorldLog>> {
        self.world_log.get()
    }
}
