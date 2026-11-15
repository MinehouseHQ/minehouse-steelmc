//! Vanilla `ReloadCommand`.

use steel_utils::{Identifier, translations};

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{CommandSource, SteelCommandContext, SteelCommandRuntime, literal},
    registration::CommandRegistration,
};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("reload"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("reload").executes(reload)
}

/// Vanilla `ReloadCommand`: picks up packs added since the last load, then
/// reloads them.
///
/// Steel's data packs carry only functions, and every pack found is enabled,
/// so a reload compiles the source domain's `datapacks` folder again. Vanilla
/// reloads in the background and swaps the functions in a few ticks later;
/// Steel swaps them before the command returns. Either way
/// `#minecraft:load` runs on the next tick.
#[expect(
    clippy::unnecessary_wraps,
    reason = "Command executors use a shared fallible callback signature."
)]
fn reload(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    source.send_success(
        &translations::COMMANDS_RELOAD_SUCCESS.msg().component(),
        true,
    );
    source
        .server()
        .reload_domain_functions(source.world().domain());
    Ok(0)
}
