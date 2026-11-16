//! Vanilla `ReloadCommand`.

use steel_utils::{Identifier, translations};

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{CommandSource, SteelCommandContext, SteelCommandRuntime, literal},
    registration::CommandRegistration,
};
use crate::datapack::PackRepository;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("reload"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("reload").executes(reload)
}

/// Vanilla `ReloadCommand`: keeps the source domain's packs, adds the ones
/// found since that were not disabled on purpose, then reloads them.
#[expect(
    clippy::unnecessary_wraps,
    reason = "Command executors use a shared fallible callback signature."
)]
fn reload(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let server = source.server();
    let domain = source.world().domain();
    let disabled = server
        .worlds
        .default_world(domain)
        .map_or_else(Vec::new, |world| {
            world.level_data.read().data().data_packs.disabled.clone()
        });
    let selected = server
        .with_pack_repository(domain, |repository| {
            let current = repository.selected_ids();
            discover_new_packs(repository, &disabled, current)
        })
        .unwrap_or_default();
    source.send_success(
        &translations::COMMANDS_RELOAD_SUCCESS.msg().component(),
        true,
    );
    server.reload_data_packs(domain, &selected);
    Ok(0)
}

/// Vanilla `discoverNewPacks`: `current`, then every available pack neither
/// in it nor `disabled`.
fn discover_new_packs(
    repository: &mut PackRepository,
    disabled: &[String],
    mut current: Vec<String>,
) -> Vec<String> {
    repository.reload();
    let new = repository
        .available_packs()
        .map(|pack| pack.id().to_owned())
        .filter(|id| !disabled.contains(id) && !current.contains(id))
        .collect::<Vec<_>>();
    current.extend(new);
    current
}
