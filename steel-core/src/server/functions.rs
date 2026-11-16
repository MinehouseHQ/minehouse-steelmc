//! Vanilla `ServerFunctionManager` on the server: loading each domain's data
//! pack functions and running `#minecraft:load` and `#minecraft:tick`.

use std::sync::Arc;

use super::Server;
use crate::command::PendingCommandExecutionQueue;
use crate::command::execution::{
    CommandExecutionContext, CommandResultCallback, CommandSource, ExecutionStop,
};
use crate::command::function::{CommandFunction, FunctionLibrary, TimerCallback};
use crate::command::sender::{CommandExecutionOwner, CommandSender};
use crate::datapack::{DataPack, FeatureFlagSet, PackRepository};
use crate::world::World;

impl Server {
    /// Vanilla `MinecraftServer.configurePackRepository` and the first
    /// resource load, for every domain: selects its packs from its default
    /// world's saved selection and compiles their functions.
    ///
    /// Lines compile as the domain's server console, as vanilla compiles them
    /// with a gamemaster source; Steel has no permission levels to lower it to.
    pub(crate) fn reload_functions(self: &Arc<Self>) {
        for domain in self.worlds.domain_names() {
            self.configure_data_packs(domain);
        }
    }

    /// Runs `f` on `domain`'s pack repository, configuring the domain first
    /// if it has none yet. `None` for an unknown domain.
    pub(crate) fn with_pack_repository<R>(
        self: &Arc<Self>,
        domain: &str,
        f: impl FnOnce(&mut PackRepository) -> R,
    ) -> Option<R> {
        if self.pack_repositories.with(domain, |_| ()).is_none() {
            self.configure_data_packs(domain);
        }
        self.pack_repositories.with(domain, f)
    }

    /// [`Self::reload_functions`] for one domain.
    fn configure_data_packs(self: &Arc<Self>, domain: &str) {
        let Some(world) = self.worlds.default_world(domain) else {
            return;
        };
        let mut repository =
            PackRepository::new(world.level_path.as_ref().map(|path| path.join("datapacks")));
        let saved = world.level_data.read().data().data_packs.clone();
        let selection = repository.configure(&saved, &FeatureFlagSet::vanilla());
        let packs = repository.selected_packs().to_vec();
        self.pack_repositories.insert(domain, repository);
        if selection != saved {
            world.level_data.write().data_mut().data_packs = selection;
        }
        self.load_functions(domain, world, packs.iter().filter_map(|pack| pack.files()));
    }

    /// Vanilla `ReloadCommand.reloadPacks` and `MinecraftServer.reloadResources`:
    /// makes `selected` the domain's packs, saves the selection and compiles
    /// their functions.
    ///
    /// Steel's packs carry only functions, so the functions are what reloads.
    /// Vanilla reloads in the background and swaps them in a few ticks later;
    /// Steel swaps them before returning. Either way `#minecraft:load` runs on
    /// the next tick.
    pub(crate) fn reload_data_packs(self: &Arc<Self>, domain: &str, selected: &[String]) {
        let Some(world) = self.worlds.default_world(domain) else {
            return;
        };
        let Some((selection, packs)) = self.pack_repositories.with(domain, |repository| {
            repository.set_selected(selected);
            (
                repository.selection_config(),
                repository.selected_packs().to_vec(),
            )
        }) else {
            return;
        };
        world.level_data.write().data_mut().data_packs = selection;
        self.load_functions(domain, world, packs.iter().filter_map(|pack| pack.files()));
    }

    /// Compiles `packs`, bottom first, into `domain`'s function library,
    /// replacing the last.
    pub(crate) fn load_functions<'pack>(
        self: &Arc<Self>,
        domain: &str,
        world: &Arc<World>,
        packs: impl IntoIterator<Item = &'pack DataPack>,
    ) {
        let compilation_source =
            CommandSource::server_in(Arc::clone(self), Arc::clone(world)).with_suppressed_output();
        let library = {
            let dispatcher = self.command_dispatcher.read();
            FunctionLibrary::load(packs, &dispatcher, &compilation_source)
        };
        self.functions.replace(domain, library);
    }

    /// Vanilla `ServerFunctionManager.tick` for every domain.
    pub(crate) fn tick_functions(
        self: &Arc<Self>,
        pending: &mut PendingCommandExecutionQueue<CommandSource>,
        runs_normally: bool,
    ) {
        for (domain, functions) in self.functions.take_tick_functions(runs_normally) {
            let Some(world) = self.worlds.default_world(&domain) else {
                continue;
            };
            // Vanilla `getGameLoopSender`.
            let source = CommandSource::server_in(Arc::clone(self), Arc::clone(world))
                .with_suppressed_output();
            for function in functions {
                self.execute_function(pending, &function, &source);
            }
        }
    }

    /// Vanilla `TimerQueue.tick` from `ServerLevel.tickTime`: runs each
    /// domain's due `/schedule` events once its game time has advanced.
    pub(crate) fn tick_scheduled_functions(
        self: &Arc<Self>,
        pending: &mut PendingCommandExecutionQueue<CommandSource>,
        runs_normally: bool,
    ) {
        if !runs_normally {
            return;
        }
        for domain in self.worlds.domain_names() {
            let (Some(queue), Some(world)) = (
                self.scheduled_events.get(domain),
                self.worlds.default_world(domain),
            ) else {
                continue;
            };
            let current_tick = world.game_time();
            // An event's function may schedule or clear others, so each one is
            // taken from the queue before it runs.
            while let Some(callback) = queue.pop_due(current_tick) {
                let library = self.functions.library(domain).unwrap_or_default();
                let functions = match &callback {
                    TimerCallback::Function(id) => library.function(id).into_iter().collect(),
                    TimerCallback::FunctionTag(id) => library.tag(id).iter().collect::<Vec<_>>(),
                };
                // Vanilla `getGameLoopSender`.
                let source = CommandSource::server_in(Arc::clone(self), Arc::clone(world))
                    .with_suppressed_output();
                for function in functions {
                    self.execute_function(pending, function, &source);
                }
            }
        }
    }

    /// Vanilla `ServerFunctionManager.execute` from the game loop: each
    /// function runs in its own execution context.
    fn execute_function(
        self: &Arc<Self>,
        pending: &mut PendingCommandExecutionQueue<CommandSource>,
        function: &CommandFunction,
        source: &CommandSource,
    ) {
        let instantiated = {
            let dispatcher = self.command_dispatcher.read();
            function.instantiate(None, &dispatcher)
        };
        // Vanilla drops an instantiation failure without a word.
        let Ok(instantiated) = instantiated else {
            return;
        };
        let mut execution = CommandExecutionContext::for_source(source);
        execution.queue_initial_function_call(
            instantiated.entries(),
            source.clone(),
            CommandResultCallback::empty(),
        );
        if execution.run() == ExecutionStop::Suspended {
            let owner = CommandExecutionOwner::capture(CommandSender::Console, self);
            if !pending.push_suspended(owner, execution) {
                tracing::error!("suspended function execution could not be retained");
            }
        }
    }
}
