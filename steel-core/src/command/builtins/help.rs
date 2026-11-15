//! Vanilla `/help`: usage lines for every command, or for one command's branches.

use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use crate::command::brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError, NodeId};
use crate::command::execution::{
    CommandSource, SteelCommandContext, SteelCommandRuntime, argument, literal,
};
use crate::command::registration::CommandRegistration;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("help"), |_| command()).default_access()
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("help")
        .executes(help_all)
        .then(argument("command", ArgumentType::greedy_string()).executes(help_command))
}

/// Vanilla `HelpCommand`: one line for each command the source can use.
#[expect(
    clippy::unnecessary_wraps,
    reason = "Command executors use a shared fallible callback signature."
)]
fn help_all(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let dispatcher = source.server().command_dispatcher().read();
    let usage = dispatcher.smart_usage(dispatcher.root(), source);
    drop(dispatcher);
    Ok(send_usage(source, "", &usage))
}

/// Vanilla `HelpCommand` with a command: the branches after the last node
/// the given text parses to.
fn help_command(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let input = context.string("command")?;
    let dispatcher = source.server().command_dispatcher().read();
    let parse = dispatcher.parse(input, source.clone());
    let Some(last) = parse.context().nodes().last() else {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_HELP_FAILED,
        )));
    };
    let usage = dispatcher.smart_usage(last.node(), source);
    let prefix = format!("{} ", parse.reader().input());
    drop(parse);
    drop(dispatcher);
    Ok(send_usage(source, &prefix, &usage))
}

fn send_usage(source: &CommandSource, prefix: &str, usage: &[(NodeId, String)]) -> i32 {
    for (_, line) in usage {
        source.send_success(&TextComponent::plain(format!("/{prefix}{line}")), false);
    }
    i32::try_from(usage.len()).unwrap_or(i32::MAX)
}
