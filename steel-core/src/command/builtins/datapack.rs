//! Vanilla `DataPackCommand`.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use steel_utils::{Identifier, translations, version};
use text_components::format::Color;
use text_components::translation::Translation;
use text_components::{Modifier as _, TextComponent};

use super::super::{
    brigadier::{
        ArgumentType, CommandNodeBuilder, CommandRequirement, CommandSyntaxError,
        SuggestionsBuilder, escape_if_required,
    },
    execution::{
        CommandPermissionSource, CommandSource, CommandTextResolver, SteelArgumentType,
        SteelCommandContext, SteelCommandRuntime, SteelSuggestionContext, argument,
        join_components, literal, suggest_list,
    },
    registration::CommandRegistration,
};
use crate::datapack::{FeatureFlagSet, Pack, PackRepository, PackSource};
use crate::permission::{PermissionExpr, PermissionKey, PermissionState};

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;
type Context = SteelCommandContext<CommandSource>;

/// The permission `/datapack create` needs granted, standing in for vanilla's
/// `LEVEL_OWNERS` above the command's own `LEVEL_GAMEMASTERS`.
const CREATE_PERMISSION: &str = "minecraft.command.datapack.create";

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("datapack"), |_| command())
}

fn command() -> Builder {
    literal("datapack")
        .then(literal("enable").then(enable_tree()))
        .then(
            literal("disable").then(
                argument("name", ArgumentType::string())
                    .suggests(suggest_selected_packs)
                    .executes(disable_pack),
            ),
        )
        .then(
            literal("list")
                .executes(|context| Ok(list_packs(context.source())))
                .then(
                    literal("available")
                        .executes(|context| Ok(list_available_packs(context.source()))),
                )
                .then(
                    literal("enabled").executes(|context| Ok(list_enabled_packs(context.source()))),
                ),
        )
        .then(literal("create").requires(owners()).then(
            argument("id", ArgumentType::string()).then(
                argument("description", SteelArgumentType::component()).executes(create_pack),
            ),
        ))
}

/// `/datapack enable <name> [first|last|before <existing>|after <existing>]`.
fn enable_tree() -> Builder {
    argument("name", ArgumentType::string())
        .suggests(suggest_unselected_packs)
        .executes(|context| enable_pack(context, Placement::Default))
        .then(
            literal("after").then(
                argument("existing", ArgumentType::string())
                    .suggests(suggest_selected_packs)
                    .executes(|context| enable_pack(context, Placement::After)),
            ),
        )
        .then(
            literal("before").then(
                argument("existing", ArgumentType::string())
                    .suggests(suggest_selected_packs)
                    .executes(|context| enable_pack(context, Placement::Before)),
            ),
        )
        .then(literal("last").executes(|context| enable_pack(context, Placement::Last)))
        .then(literal("first").executes(|context| enable_pack(context, Placement::First)))
}

/// Vanilla `Commands.hasPermission(LEVEL_OWNERS)` for `create`.
fn owners() -> CommandRequirement<CommandSource> {
    let permission = PermissionKey::parse(CREATE_PERMISSION).map(PermissionExpr::key);
    CommandRequirement::authorization(move |source: &CommandSource| {
        permission.as_ref().is_ok_and(|permission| {
            source.permission_state(permission) == Some(PermissionState::Allow)
        })
    })
}

/// Vanilla `CommandSourceStack.enabledFeatures`: Steel worlds enable only the
/// vanilla flag.
fn enabled_features() -> FeatureFlagSet {
    FeatureFlagSet::vanilla()
}

/// Runs `f` on the source domain's pack repository.
fn with_repository<R>(
    source: &CommandSource,
    f: impl FnOnce(&mut PackRepository) -> R,
) -> Option<R> {
    source
        .server()
        .with_pack_repository(source.world().domain(), f)
}

/// Vanilla `SELECTED_PACKS`.
fn suggest_selected_packs(
    context: &SteelSuggestionContext<'_, CommandSource>,
    builder: &mut SuggestionsBuilder<'_>,
) {
    let ids = with_repository(context.source(), |repository| {
        repository
            .selected_packs()
            .iter()
            .map(|pack| escape_if_required(pack.id()))
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    suggest_list(builder, ids);
}

/// Vanilla `UNSELECTED_PACKS`: the packs the world's features allow that are
/// not enabled.
fn suggest_unselected_packs(
    context: &SteelSuggestionContext<'_, CommandSource>,
    builder: &mut SuggestionsBuilder<'_>,
) {
    let features = enabled_features();
    let ids = with_repository(context.source(), |repository| {
        repository
            .available_packs()
            .filter(|pack| {
                pack.requested_features().is_subset_of(&features) && !repository.is_selected(pack)
            })
            .map(|pack| escape_if_required(pack.id()))
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    suggest_list(builder, ids);
}

fn pack_error(translation: &Translation<1>, id: &str) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(translation.message([id.to_owned()]).component())
}

/// Vanilla `getPack`: the named pack, which must be disabled to enable it,
/// enabled to disable it, and need no feature the world lacks.
fn get_pack(
    context: &Context,
    name: &str,
    enabling: bool,
) -> Result<Arc<Pack>, CommandSyntaxError> {
    let id = context.string(name)?;
    let found = with_repository(context.source(), |repository| {
        repository
            .pack(id)
            .map(|pack| (repository.is_selected(&pack), pack))
    })
    .flatten();
    let Some((enabled, pack)) = found else {
        return Err(pack_error(&translations::COMMANDS_DATAPACK_UNKNOWN, id));
    };
    if enabling && enabled {
        return Err(pack_error(
            &translations::COMMANDS_DATAPACK_ENABLE_FAILED,
            id,
        ));
    }
    if !enabling && !enabled {
        return Err(pack_error(
            &translations::COMMANDS_DATAPACK_DISABLE_FAILED,
            id,
        ));
    }
    let available = enabled_features();
    let requested = pack.requested_features();
    if !enabling && !requested.is_empty() && pack.source() == PackSource::Feature {
        return Err(pack_error(
            &translations::COMMANDS_DATAPACK_DISABLE_FAILED_FEATURE,
            id,
        ));
    }
    if !requested.is_subset_of(&available) {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_DATAPACK_ENABLE_FAILED_NO_FLAGS
                .message([id.to_owned(), requested.print_missing(&available)])
                .component(),
        ));
    }
    Ok(pack)
}

/// Where `/datapack enable` puts the pack in the selection.
#[derive(Clone, Copy)]
enum Placement {
    /// The pack's default position.
    Default,
    After,
    Before,
    Last,
    First,
}

/// The selection, bottom first.
fn selected_packs(source: &CommandSource) -> Vec<Arc<Pack>> {
    with_repository(source, |repository| repository.selected_packs().to_vec()).unwrap_or_default()
}

fn position_of(selected: &[Arc<Pack>], pack: &Pack) -> usize {
    selected
        .iter()
        .position(|selected| selected.id() == pack.id())
        .unwrap_or_default()
}

/// Vanilla `enablePack`.
fn enable_pack(context: &Context, placement: Placement) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let pack = get_pack(context, "name", true)?;
    let mut selected = selected_packs(source);
    match placement {
        Placement::Default => pack.insert_at_default_position(&mut selected),
        Placement::After => {
            let existing = get_pack(context, "existing", false)?;
            let index = position_of(&selected, &existing) + 1;
            selected.insert(index, Arc::clone(&pack));
        }
        Placement::Before => {
            let existing = get_pack(context, "existing", false)?;
            let index = position_of(&selected, &existing);
            selected.insert(index, Arc::clone(&pack));
        }
        Placement::Last => selected.push(Arc::clone(&pack)),
        Placement::First => selected.insert(0, Arc::clone(&pack)),
    }
    source.send_success(
        &translations::COMMANDS_DATAPACK_MODIFY_ENABLE
            .message([pack.chat_link(true)])
            .component(),
        true,
    );
    reload_packs(source, &selected);
    Ok(count(selected.len()))
}

/// Vanilla `disablePack`.
fn disable_pack(context: &Context) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let pack = get_pack(context, "name", false)?;
    let mut selected = selected_packs(source);
    selected.retain(|selected| selected.id() != pack.id());
    source.send_success(
        &translations::COMMANDS_DATAPACK_MODIFY_DISABLE
            .message([pack.chat_link(true)])
            .component(),
        true,
    );
    reload_packs(source, &selected);
    Ok(count(selected.len()))
}

/// Vanilla `ReloadCommand.reloadPacks`.
fn reload_packs(source: &CommandSource, selected: &[Arc<Pack>]) {
    let ids = selected
        .iter()
        .map(|pack| pack.id().to_owned())
        .collect::<Vec<_>>();
    source
        .server()
        .reload_data_packs(source.world().domain(), &ids);
}

fn count(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// Vanilla `listPacks`.
fn list_packs(source: &CommandSource) -> i32 {
    list_enabled_packs(source).saturating_add(list_available_packs(source))
}

/// Vanilla `listAvailablePacks`: finds the packs anew, then lists the ones the
/// world's features allow that are not enabled.
fn list_available_packs(source: &CommandSource) -> i32 {
    let features = enabled_features();
    let unselected = with_repository(source, |repository| {
        repository.reload();
        repository
            .available_packs()
            .filter(|pack| {
                !repository.is_selected(pack) && pack.requested_features().is_subset_of(&features)
            })
            .cloned()
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    let message = if unselected.is_empty() {
        translations::COMMANDS_DATAPACK_LIST_AVAILABLE_NONE
            .msg()
            .component()
    } else {
        list_message(
            &translations::COMMANDS_DATAPACK_LIST_AVAILABLE_SUCCESS,
            &unselected,
            false,
        )
    };
    source.send_success(&message, false);
    count(unselected.len())
}

/// Vanilla `listEnabledPacks`: finds the packs anew, then lists the enabled
/// ones.
fn list_enabled_packs(source: &CommandSource) -> i32 {
    let selected = with_repository(source, |repository| {
        repository.reload();
        repository.selected_packs().to_vec()
    })
    .unwrap_or_default();
    let message = if selected.is_empty() {
        translations::COMMANDS_DATAPACK_LIST_ENABLED_NONE
            .msg()
            .component()
    } else {
        list_message(
            &translations::COMMANDS_DATAPACK_LIST_ENABLED_SUCCESS,
            &selected,
            true,
        )
    };
    source.send_success(&message, false);
    count(selected.len())
}

/// A list message: the pack count and their chat links, as vanilla's
/// `ComponentUtils.formatList`.
fn list_message(translation: &Translation<2>, packs: &[Arc<Pack>], enabled: bool) -> TextComponent {
    let links = join_components(
        packs.iter().map(|pack| pack.chat_link(enabled)),
        &TextComponent::plain(", ").color(Color::Gray),
    );
    translation
        .message([TextComponent::plain(packs.len().to_string()), links])
        .component()
}

/// Vanilla `PackMetadataSection` as `/datapack create` writes it.
#[derive(Serialize)]
struct PackMcmeta<'description> {
    pack: PackSection<'description>,
}

/// Vanilla `PackMetadataSection.SERVER_TYPE` for the current data pack format:
/// every minor version of its major one, which encodes as the bare major.
#[derive(Serialize)]
struct PackSection<'description> {
    description: &'description TextComponent,
    min_format: i32,
    max_format: i32,
}

/// Vanilla `createPack`: an empty pack in the world's `datapacks` folder.
fn create_pack(context: &Context) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let id = context.string("id")?;
    let description = context
        .text_component("description")?
        .try_resolve(&CommandTextResolver::for_source(source))?;
    if !is_valid_path_segment(id) {
        return Err(pack_error(
            &translations::COMMANDS_DATAPACK_CREATE_INVALID_NAME,
            id,
        ));
    }
    if !is_path_part_portable(id) {
        return Err(pack_error(
            &translations::COMMANDS_DATAPACK_CREATE_INVALID_FULL_NAME,
            id,
        ));
    }
    let io_failure = || pack_error(&translations::COMMANDS_DATAPACK_CREATE_IO_FAILURE, id);
    let datapacks = source
        .server()
        .worlds
        .default_world(source.world().domain())
        .and_then(|world| world.level_path.as_ref().map(|path| path.join("datapacks")))
        .ok_or_else(io_failure)?;
    let pack_dir = datapacks.join(id);
    if pack_dir.exists() {
        return Err(pack_error(
            &translations::COMMANDS_DATAPACK_CREATE_ALREADY_EXISTS,
            id,
        ));
    }
    if let Err(error) = write_pack(&pack_dir, &description) {
        log::warn!("Failed to create pack at {}: {error}", datapacks.display());
        return Err(io_failure());
    }
    source.send_success(
        &translations::COMMANDS_DATAPACK_CREATE_SUCCESS
            .message([id.to_owned()])
            .component(),
        true,
    );
    Ok(1)
}

/// The pack folder, its `data` folder and its `pack.mcmeta`, written as
/// vanilla's Gson writer does with a two-space indent.
fn write_pack(pack_dir: &Path, description: &TextComponent) -> io::Result<()> {
    let mcmeta = PackMcmeta {
        pack: PackSection {
            description,
            min_format: version::DATA_PACK_MAJOR_VERSION,
            max_format: version::DATA_PACK_MAJOR_VERSION,
        },
    };
    let json = serde_json::to_string_pretty(&mcmeta).map_err(io::Error::other)?;
    fs::create_dir(pack_dir)?;
    fs::create_dir(pack_dir.join("data"))?;
    fs::write(pack_dir.join("pack.mcmeta"), json)
}

/// Vanilla `FileUtil.isValidPathSegment`.
fn is_valid_path_segment(segment: &str) -> bool {
    segment != "."
        && segment != ".."
        && !segment.is_empty()
        && segment
            .chars()
            .all(|c| matches!(c, '-' | '.' | '_' | 'a'..='z' | '0'..='9'))
}

/// Vanilla `FileUtil.isPathPartPortable`: not a name Windows reserves, nor
/// one ending in a dot.
fn is_path_part_portable(name: &str) -> bool {
    if name.ends_with('.') {
        return false;
    }
    let stem = name.split_once('.').map_or(name, |(stem, _)| stem);
    let stem = stem.to_ascii_uppercase();
    let reserved = matches!(
        stem.as_str(),
        "CLOCK$" | "CON" | "PRN" | "AUX" | "NUL" | "COM"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|digit| {
            matches!(digit, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
        })
    });
    !reserved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datapack::read_pack_metadata;

    #[test]
    fn pack_names_must_be_portable_path_segments() {
        assert!(is_valid_path_segment("my-pack_1.0"));
        for invalid in ["", ".", "..", "My", "a b", "a/b"] {
            assert!(!is_valid_path_segment(invalid), "{invalid}");
        }
        assert!(is_path_part_portable("console"));
        assert!(is_path_part_portable("com10"));
        for reserved in ["con", "com", "nul.txt", "lpt3", "clock$", "pack."] {
            assert!(!is_path_part_portable(reserved), "{reserved}");
        }
    }

    #[test]
    fn created_metadata_reads_back() {
        let json = serde_json::to_string_pretty(&PackMcmeta {
            pack: PackSection {
                description: &TextComponent::plain("Hi"),
                min_format: version::DATA_PACK_MAJOR_VERSION,
                max_format: version::DATA_PACK_MAJOR_VERSION,
            },
        })
        .expect("metadata should encode");
        assert!(json.starts_with("{\n  \"pack\": {\n    \"description\": "));
        assert!(json.contains("\"min_format\": 121,\n    \"max_format\": 121\n"));
        let metadata =
            read_pack_metadata("file/hi", json.as_bytes()).expect("created metadata should read");
        assert_eq!(metadata.description, TextComponent::plain("Hi"));
    }
}
