//! Vanilla `FunctionCommand`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use simdnbt::owned::{NbtCompound, NbtTag};
use steel_utils::{Identifier, translations};
use text_components::{Modifier as _, TextComponent, format::Color};

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        ChainModifiers, CommandExecutionContext, CommandResultCallback, CommandSource,
        CustomCommandExecutor, EntryAction, ExecutionCommandSource, ExecutionControl, Frame,
        SteelArgumentType, SteelCommandContext, SteelCommandRuntime, SteelContextChain, argument,
        join_components, literal,
    },
    function::CommandFunction,
    registration::CommandRegistration,
};
use super::data::{
    accessor::{AccessorKind, DataAccessor},
    single_tag,
};

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;
type Context = SteelCommandContext<CommandSource>;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("function"), |_| command())
}

fn command() -> Builder {
    let mut sources = literal("with");
    for kind in AccessorKind::ALL {
        sources = kind.wrap(sources, "source", move |source| {
            source
                .executes_custom(FunctionExecutor(Arguments::Source(kind)))
                .then(
                    argument("path", SteelArgumentType::nbt_path())
                        .executes_custom(FunctionExecutor(Arguments::SourcePath(kind))),
                )
        });
    }
    literal("function").then(
        argument("name", SteelArgumentType::function())
            .executes_custom(FunctionExecutor(Arguments::None))
            .then(
                argument("arguments", SteelArgumentType::compound_tag())
                    .executes_custom(FunctionExecutor(Arguments::Inline)),
            )
            .then(sources),
    )
}

/// Where a `FunctionCustomExecutor` takes its macro arguments from.
#[derive(Clone, Copy)]
enum Arguments {
    None,
    Inline,
    Source(AccessorKind),
    SourcePath(AccessorKind),
}

impl Arguments {
    /// Vanilla `FunctionCustomExecutor.arguments`.
    fn read(self, context: &Context) -> Result<Option<NbtCompound>, CommandSyntaxError> {
        Ok(match self {
            Self::None => None,
            Self::Inline => Some(context.compound_tag("arguments")?.clone()),
            Self::Source(kind) => Some(DataAccessor::access(context, kind, "source")?.data()),
            Self::SourcePath(kind) => {
                let accessor = DataAccessor::access(context, kind, "source")?;
                let path = context.nbt_path("path")?;
                Some(argument_tag(single_tag(&accessor, path)?)?)
            }
        })
    }
}

/// Vanilla `getArgumentTag`: the tag at the path must be a compound.
fn argument_tag(tag: NbtTag) -> Result<NbtCompound, CommandSyntaxError> {
    match tag {
        NbtTag::Compound(compound) => Ok(compound),
        tag => Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_FUNCTION_ERROR_ARGUMENT_NOT_COMPOUND
                .message([tag_type_name(&tag)])
                .component(),
        )),
    }
}

/// Vanilla `TagType.getName`.
const fn tag_type_name(tag: &NbtTag) -> &'static str {
    match tag {
        NbtTag::Byte(_) => "BYTE",
        NbtTag::Short(_) => "SHORT",
        NbtTag::Int(_) => "INT",
        NbtTag::Long(_) => "LONG",
        NbtTag::Float(_) => "FLOAT",
        NbtTag::Double(_) => "DOUBLE",
        NbtTag::ByteArray(_) => "BYTE[]",
        NbtTag::String(_) => "STRING",
        NbtTag::List(_) => "LIST",
        NbtTag::Compound(_) => "COMPOUND",
        NbtTag::IntArray(_) => "INT[]",
        NbtTag::LongArray(_) => "LONG[]",
    }
}

/// Vanilla `FunctionCommand.FunctionCustomExecutor`, with `WithErrorHandling`.
struct FunctionExecutor(Arguments);

impl CustomCommandExecutor<CommandSource> for FunctionExecutor {
    fn run(
        &self,
        source: Arc<CommandSource>,
        chain: &SteelContextChain<CommandSource>,
        modifiers: ChainModifiers,
        control: &mut ExecutionControl<'_, CommandSource>,
    ) {
        if let Err(error) = self.run_guarded(&source, chain, modifiers, control) {
            source.handle_error(&error, modifiers.is_forked());
            source.callback().on_result(false, 0);
        }
    }
}

impl FunctionExecutor {
    fn run_guarded(
        &self,
        source: &Arc<CommandSource>,
        chain: &SteelContextChain<CommandSource>,
        modifiers: ChainModifiers,
        control: &mut ExecutionControl<'_, CommandSource>,
    ) -> Result<(), CommandSyntaxError> {
        let context = chain.top_context().copy_for(Arc::clone(source));
        let (name, functions) = function_collection(&context, "name")?;
        if functions.is_empty() {
            return Err(CommandSyntaxError::dynamic(
                translations::COMMANDS_FUNCTION_SCHEDULED_NO_FUNCTIONS
                    .message([name.to_string()])
                    .component(),
            ));
        }
        let arguments = self.0.read(&context)?;
        // Vanilla `modifySenderForExecution` also raises the permission level to
        // gamemaster; Steel has no levels, and a function's lines were checked
        // when they compiled.
        let function_source = source.with_suppressed_output();
        let message = if let [function] = functions.as_slice() {
            translations::COMMANDS_FUNCTION_SCHEDULED_SINGLE
                .message([function.id().to_string()])
                .component()
        } else {
            translations::COMMANDS_FUNCTION_SCHEDULED_MULTIPLE
                .message([id_list(&functions)])
                .component()
        };
        source.send_success(&message, true);
        let queue = FunctionQueue {
            arguments: arguments.as_ref(),
            original_source: source,
            function_source: Arc::new(
                function_source.with_callback(CommandResultCallback::empty()),
            ),
        };
        if modifiers.is_return() {
            queue.as_return(&functions, control)
        } else {
            queue.no_return(&functions, control)
        }
    }
}

/// Vanilla `FunctionArgument.getFunctionCollection`.
pub(super) fn function_collection(
    context: &Context,
    name: &str,
) -> Result<(Identifier, Vec<Arc<CommandFunction>>), CommandSyntaxError> {
    let reference = context.function_reference(name)?;
    let library = context
        .source()
        .server()
        .functions
        .library(context.source().world().domain())
        .unwrap_or_default();
    if reference.tag {
        return Ok((reference.id.clone(), library.tag(&reference.id).to_vec()));
    }
    let function = library.function(&reference.id).ok_or_else(|| {
        CommandSyntaxError::dynamic(
            translations::ARGUMENTS_FUNCTION_UNKNOWN
                .message([reference.id.to_string()])
                .component(),
        )
    })?;
    Ok((reference.id.clone(), vec![Arc::clone(function)]))
}

/// Vanilla `ComponentUtils.formatList` of the functions' ids, with its gray
/// comma separator.
fn id_list(functions: &[Arc<CommandFunction>]) -> TextComponent {
    join_components(
        functions
            .iter()
            .map(|function| TextComponent::plain(function.id().to_string())),
        &TextComponent::plain(", ").color(Color::Gray),
    )
}

/// Vanilla `FunctionCommand.queueFunctions`' shared arguments.
struct FunctionQueue<'a> {
    arguments: Option<&'a NbtCompound>,
    original_source: &'a Arc<CommandSource>,
    function_source: Arc<CommandSource>,
}

impl FunctionQueue<'_> {
    /// Vanilla `queueFunctionsAsReturn`: each function's result returns from
    /// the frame that ran `return run function`.
    fn as_return(
        &self,
        functions: &[Arc<CommandFunction>],
        control: &mut ExecutionControl<'_, CommandSource>,
    ) -> Result<(), CommandSyntaxError> {
        let output = CommandResultCallback::chain(
            self.original_source.callback(),
            control.current_frame().return_value_consumer(),
        );
        for function in functions {
            let collector = self.decorate_output(function.id(), output.clone());
            self.instantiate_and_queue(function, collector, true, control)?;
        }
        control.queue_fallthrough();
        Ok(())
    }

    /// Vanilla `queueFunctionsNoReturn`: several functions report their summed
    /// results once all have run.
    fn no_return(
        &self,
        functions: &[Arc<CommandFunction>],
        control: &mut ExecutionControl<'_, CommandSource>,
    ) -> Result<(), CommandSyntaxError> {
        let original = self.original_source.callback();
        if functions.len() == 1 || original.is_empty() {
            for function in functions {
                let collector = self.decorate_output(function.id(), original.clone());
                self.instantiate_and_queue(function, collector, false, control)?;
            }
            return Ok(());
        }

        let accumulator = Arc::new(Accumulator::default());
        for function in functions {
            let partial = {
                let accumulator = Arc::clone(&accumulator);
                CommandResultCallback::new(move |_, result| accumulator.add(result))
            };
            let collector = self.decorate_output(function.id(), partial);
            self.instantiate_and_queue(function, collector, false, control)?;
        }
        control.queue_next(ReportSum {
            accumulator,
            callback: original,
        });
        Ok(())
    }

    /// Vanilla `decorateOutputIfNeeded`: a non-silent caller hears each result.
    fn decorate_output(
        &self,
        id: &Identifier,
        callback: CommandResultCallback,
    ) -> CommandResultCallback {
        if self.original_source.is_silent() {
            return callback;
        }
        let source = Arc::clone(self.original_source);
        let id = id.to_string();
        CommandResultCallback::new(move |success, result| {
            source.send_success(
                &translations::COMMANDS_FUNCTION_RESULT
                    .message([
                        TextComponent::plain(id.clone()),
                        TextComponent::plain(result.to_string()),
                    ])
                    .component(),
                true,
            );
            callback.on_result(success, result);
        })
    }

    /// Vanilla `instantiateAndQueueFunctions`.
    fn instantiate_and_queue(
        &self,
        function: &CommandFunction,
        collector: CommandResultCallback,
        return_parent_frame: bool,
        control: &mut ExecutionControl<'_, CommandSource>,
    ) -> Result<(), CommandSyntaxError> {
        let instantiated = {
            let dispatcher = self.original_source.server().command_dispatcher().read();
            function.instantiate(self.arguments, &dispatcher)
        }
        .map_err(|reason| {
            CommandSyntaxError::dynamic(
                translations::COMMANDS_FUNCTION_INSTANTIATION_FAILURE
                    .message([TextComponent::plain(function.id().to_string()), *reason])
                    .component(),
            )
        })?;
        control.queue_function_call(
            instantiated.entries(),
            Arc::clone(&self.function_source),
            collector,
            return_parent_frame,
        );
        Ok(())
    }
}

/// Vanilla `queueFunctionsNoReturn`'s local `Accumulator`.
#[derive(Default)]
struct Accumulator {
    any_result: AtomicBool,
    sum: AtomicI32,
}

impl Accumulator {
    fn add(&self, result: i32) {
        self.any_result.store(true, Ordering::Relaxed);
        // Wraps like Java's `int` addition.
        self.sum.fetch_add(result, Ordering::Relaxed);
    }
}

/// The task queued after every function: reports the summed result, if any.
struct ReportSum {
    accumulator: Arc<Accumulator>,
    callback: CommandResultCallback,
}

impl EntryAction<CommandSource> for ReportSum {
    fn execute(
        self: Box<Self>,
        _context: &mut CommandExecutionContext<CommandSource>,
        _frame: Frame,
    ) {
        if self.accumulator.any_result.load(Ordering::Relaxed) {
            self.callback
                .on_result(true, self.accumulator.sum.load(Ordering::Relaxed));
        }
    }
}
