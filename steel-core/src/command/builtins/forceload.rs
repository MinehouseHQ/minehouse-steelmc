//! Vanilla `ForceLoadCommand`.

use steel_utils::{BlockPos, ChunkPos, Identifier, SectionPos, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};

/// Vanilla `MAX_CHUNK_LIMIT`.
const MAX_CHUNK_LIMIT: i64 = 256;
/// Vanilla's ±30,000,000 block bound on the selected columns.
const WORLD_BOUND: i32 = 30_000_000;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("forceload"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("forceload")
        .then(literal("add").then(change_arguments(true)))
        .then(
            literal("remove")
                .then(change_arguments(false))
                .then(literal("all").executes(|context| Ok(remove_all(context)))),
        )
        .then(
            literal("query")
                .executes(|context| Ok(list_force_load(context)))
                .then(argument("pos", SteelArgumentType::column_pos()).executes(query_force_load)),
        )
}

fn change_arguments(add: bool) -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    argument("from", SteelArgumentType::column_pos())
        .executes(move |context| {
            let from = column(context, "from")?;
            change_force_load(context, from, from, add)
        })
        .then(
            argument("to", SteelArgumentType::column_pos()).executes(move |context| {
                change_force_load(
                    context,
                    column(context, "from")?,
                    column(context, "to")?,
                    add,
                )
            }),
        )
}

/// Vanilla `ColumnPosArgument.getColumnPos`: the `(x, z)` block column.
fn column(
    context: &SteelCommandContext<CommandSource>,
    name: &str,
) -> Result<(i32, i32), CommandSyntaxError> {
    let position = context.coordinates(name)?.block_pos(context.source());
    Ok((position.x(), position.z()))
}

/// Vanilla `ChunkPos.toString`.
fn chunk_text(pos: ChunkPos) -> String {
    format!("[{}, {}]", pos.0.x, pos.0.y)
}

/// Vanilla `ChunkPos.pack`, which orders the `query` listing.
fn packed(pos: ChunkPos) -> i64 {
    (i64::from(pos.0.y) << 32) | i64::from(pos.0.x.cast_unsigned())
}

/// The world's force-loaded chunks in vanilla's listing order.
fn sorted_force_loaded_chunks(context: &SteelCommandContext<CommandSource>) -> Vec<ChunkPos> {
    let mut chunks = context.source().world().force_loaded_chunks();
    chunks.sort_unstable_by_key(|pos| packed(*pos));
    chunks
}

fn query_force_load(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let (x, z) = column(context, "pos")?;
    let chunk = ChunkPos::from_block_pos(BlockPos::new(x, 0, z));
    let source = context.source();
    let dimension = source.world().key.to_string();
    let args = [
        TextComponent::plain(chunk_text(chunk)),
        TextComponent::plain(dimension),
    ];
    if source.world().force_loaded_chunks().contains(&chunk) {
        let message = translations::COMMANDS_FORCELOAD_QUERY_SUCCESS
            .message(args)
            .component();
        source.send_success(&message, false);
        Ok(1)
    } else {
        Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_FORCELOAD_QUERY_FAILURE
                .message(args)
                .component(),
        ))
    }
}

fn list_force_load(context: &SteelCommandContext<CommandSource>) -> i32 {
    let source = context.source();
    let dimension = TextComponent::plain(source.world().key.to_string());
    let chunks = sorted_force_loaded_chunks(context);
    let chunk_count = chunks.len();
    if chunk_count == 0 {
        source.send_failure(
            translations::COMMANDS_FORCELOAD_ADDED_NONE
                .message([dimension])
                .component(),
        );
        return 0;
    }

    let chunk_list = TextComponent::plain(
        chunks
            .into_iter()
            .map(chunk_text)
            .collect::<Vec<_>>()
            .join(", "),
    );
    let message = if chunk_count == 1 {
        translations::COMMANDS_FORCELOAD_LIST_SINGLE
            .message([dimension, chunk_list])
            .component()
    } else {
        translations::COMMANDS_FORCELOAD_LIST_MULTIPLE
            .message([
                TextComponent::plain(chunk_count.to_string()),
                dimension,
                chunk_list,
            ])
            .component()
    };
    source.send_success(&message, false);
    i32::try_from(chunk_count).unwrap_or(i32::MAX)
}

fn remove_all(context: &SteelCommandContext<CommandSource>) -> i32 {
    let source = context.source();
    let world = source.world();
    for chunk in world.force_loaded_chunks() {
        let _ = world.set_chunk_forced(chunk, false);
    }
    let message = translations::COMMANDS_FORCELOAD_REMOVED_ALL
        .message([TextComponent::plain(world.key.to_string())])
        .component();
    source.send_success(&message, true);
    0
}

/// Vanilla `changeForceLoad`, with its `CommandResponseTracker` feedback:
/// counts only the chunks whose forced state actually changed.
fn change_force_load(
    context: &SteelCommandContext<CommandSource>,
    (from_x, from_z): (i32, i32),
    (to_x, to_z): (i32, i32),
    add: bool,
) -> Result<i32, CommandSyntaxError> {
    let (min_x, max_x) = (from_x.min(to_x), from_x.max(to_x));
    let (min_z, max_z) = (from_z.min(to_z), from_z.max(to_z));
    if min_x < -WORLD_BOUND || min_z < -WORLD_BOUND || max_x >= WORLD_BOUND || max_z >= WORLD_BOUND
    {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::ARGUMENT_POS_OUTOFWORLD,
        )));
    }
    let min_chunk = ChunkPos::new(
        SectionPos::block_to_section_coord(min_x),
        SectionPos::block_to_section_coord(min_z),
    );
    let max_chunk = ChunkPos::new(
        SectionPos::block_to_section_coord(max_x),
        SectionPos::block_to_section_coord(max_z),
    );
    let chunk_count = (i64::from(max_chunk.0.x) - i64::from(min_chunk.0.x) + 1)
        * (i64::from(max_chunk.0.y) - i64::from(min_chunk.0.y) + 1);
    if chunk_count > MAX_CHUNK_LIMIT {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_FORCELOAD_TOOBIG
                .message([
                    TextComponent::plain(MAX_CHUNK_LIMIT.to_string()),
                    TextComponent::plain(chunk_count.to_string()),
                ])
                .component(),
        ));
    }

    let source = context.source();
    let world = source.world();
    let mut changed = Vec::new();
    for x in min_chunk.0.x..=max_chunk.0.x {
        for z in min_chunk.0.y..=max_chunk.0.y {
            let chunk = ChunkPos::new(x, z);
            if world.set_chunk_forced(chunk, add) {
                changed.push(chunk);
            }
        }
    }

    let dimension = TextComponent::plain(world.key.to_string());
    let message = match changed.as_slice() {
        [] => {
            let failure = if add {
                &translations::COMMANDS_FORCELOAD_ADDED_FAILURE
            } else {
                &translations::COMMANDS_FORCELOAD_REMOVED_FAILURE
            };
            return Err(CommandSyntaxError::dynamic(TextComponent::from(failure)));
        }
        [chunk] => {
            let single = if add {
                &translations::COMMANDS_FORCELOAD_ADDED_SINGLE
            } else {
                &translations::COMMANDS_FORCELOAD_REMOVED_SINGLE
            };
            single
                .message([TextComponent::plain(chunk_text(*chunk)), dimension])
                .component()
        }
        _ => {
            let multiple = if add {
                &translations::COMMANDS_FORCELOAD_ADDED_MULTIPLE
            } else {
                &translations::COMMANDS_FORCELOAD_REMOVED_MULTIPLE
            };
            multiple
                .message([
                    TextComponent::plain(changed.len().to_string()),
                    dimension,
                    TextComponent::plain(chunk_text(min_chunk)),
                    TextComponent::plain(chunk_text(max_chunk)),
                ])
                .component()
        }
    };
    source.send_success(&message, true);
    Ok(i32::try_from(changed.len()).unwrap_or(i32::MAX))
}
