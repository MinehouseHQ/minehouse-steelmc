//! Vanilla `ServerFunctionManager` on the server: loading each domain's data
//! pack functions and running `#minecraft:load` and `#minecraft:tick`.

use std::sync::Arc;

use super::Server;
use crate::command::PendingCommandExecutionQueue;
use crate::command::execution::{
    CommandExecutionContext, CommandResultCallback, CommandSource, ExecutionStop,
};
use crate::command::function::{CommandFunction, FunctionLibrary};
use crate::command::sender::{CommandExecutionOwner, CommandSender};
use crate::datapack::{DataPack, find_packs};
use crate::world::World;

impl Server {
    /// Vanilla `ServerFunctionLibrary.reload` for every domain: compiles the
    /// functions in its default world's `datapacks` folder.
    ///
    /// Lines compile as the domain's server console, as vanilla compiles them
    /// with a gamemaster source; Steel has no permission levels to lower it to.
    pub(crate) fn reload_functions(self: &Arc<Self>) {
        for domain in self.worlds.domain_names() {
            let Some(world) = self.worlds.default_world(domain) else {
                continue;
            };
            let packs = world
                .level_path
                .as_ref()
                .map_or_else(Vec::new, |path| find_packs(&path.join("datapacks")));
            self.load_functions(domain, world, &packs);
        }
    }

    /// Compiles `packs` into `domain`'s function library, replacing the last.
    pub(crate) fn load_functions(
        self: &Arc<Self>,
        domain: &str,
        world: &Arc<World>,
        packs: &[DataPack],
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
