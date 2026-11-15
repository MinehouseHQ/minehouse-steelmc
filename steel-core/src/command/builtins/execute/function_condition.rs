//! Vanilla `/execute if function` and `/execute unless function`.

use std::mem;
use std::sync::Arc;

use steel_utils::locks::SyncMutex;
use steel_utils::translations;
use text_components::TextComponent;

use super::super::super::{
    brigadier::{CommandNodeBuilder, CommandRedirectTarget, CommandSyntaxError},
    execution::{
        ChainModifiers, CommandResultCallback, CommandSource, CustomModifierExecutor,
        ExecutionCommandSource as _, ExecutionControl, SteelArgumentType, SteelCommandRuntime,
        SteelContextChain, argument, literal,
    },
    function::InstantiatedFunction,
};
use super::super::function::function_collection;

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;

/// Vanilla's `function <name>` conditional, which only forks: it has no
/// result of its own to report.
pub(super) fn function_condition(expected: bool) -> Builder {
    literal("function").then(
        argument("name", SteelArgumentType::function()).redirects_custom(
            CommandRedirectTarget::CommandRoot,
            IfFunction { expected },
            true,
        ),
    )
}

/// Vanilla `ExecuteIfFunctionCustomModifier`.
struct IfFunction {
    expected: bool,
}

impl IfFunction {
    /// `if` passes a source whose functions return non-zero, `unless` one
    /// whose functions return zero.
    const fn check(&self, value: i32) -> bool {
        (value != 0) == self.expected
    }
}

impl CustomModifierExecutor<CommandSource> for IfFunction {
    /// Vanilla `ExecuteCommand.scheduleFunctionConditionsAndTest`.
    fn apply(
        &self,
        original_source: Arc<CommandSource>,
        sources: Vec<Arc<CommandSource>>,
        chain: &SteelContextChain<CommandSource>,
        modifiers: ChainModifiers,
        control: &mut ExecutionControl<'_, CommandSource>,
    ) {
        let context = chain.top_context().copy_for(Arc::clone(&original_source));
        let functions = match function_collection(&context, "name") {
            Ok((_, functions)) => functions,
            Err(error) => {
                original_source.handle_error(&error, modifiers.is_forked());
                return;
            }
        };
        if functions.is_empty() {
            return;
        }

        // Like vanilla, a function that fails to instantiate reports the
        // error and leaves the rest out, yet the condition still runs.
        let mut instantiated = Vec::with_capacity(functions.len());
        {
            let dispatcher = original_source.server().command_dispatcher().read();
            for function in &functions {
                match function.instantiate(None, &dispatcher) {
                    Ok(function) => instantiated.push(function),
                    Err(reason) => {
                        let error = CommandSyntaxError::dynamic(
                            translations::COMMANDS_EXECUTE_FUNCTION_INSTANTIATION_FAILURE
                                .message([TextComponent::plain(function.id().to_string()), *reason])
                                .component(),
                        );
                        original_source.handle_error(&error, modifiers.is_forked());
                        break;
                    }
                }
            }
        }
        let instantiated: Arc<[Arc<InstantiatedFunction>]> = instantiated.into();

        let passed = Arc::new(SyncMutex::new(Vec::with_capacity(sources.len())));
        for source in sources {
            // Vanilla `clearCallbacks`, then `FunctionCommand.modifySenderForExecution`.
            let function_source = Arc::new(
                source
                    .with_callback(CommandResultCallback::empty())
                    .with_suppressed_output(),
            );
            let output = {
                let passed = Arc::clone(&passed);
                let expected = self.expected;
                CommandResultCallback::new(move |_, result| {
                    if (IfFunction { expected }).check(result) {
                        passed.lock().push(Arc::clone(&source));
                    }
                })
            };
            let instantiated = Arc::clone(&instantiated);
            control.queue_isolated_call(output, move |control| {
                for function in instantiated.iter() {
                    let consumer = control.current_frame().return_value_consumer();
                    control.queue_function_call(
                        function.entries(),
                        Arc::clone(&function_source),
                        consumer,
                        true,
                    );
                }
                control.queue_fallthrough();
            });
        }

        let Some(next_stage) = chain.next_stage() else {
            unreachable!("function conditions fork to the execute root")
        };
        control.queue_continuation(
            next_stage,
            original_source,
            move || mem::take(&mut *passed.lock()),
            modifiers,
        );
    }
}
