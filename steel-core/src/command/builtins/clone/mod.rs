//! Vanilla `CloneCommands`.

use std::collections::VecDeque;
use std::sync::Arc;

use simdnbt::owned::NbtCompound;
use steel_registry::{
    blocks::block_state_ext::BlockStateExt as _, vanilla_blocks,
    vanilla_game_rules::MAX_BLOCK_MODIFICATIONS,
};
use steel_utils::{
    BlockPos, BlockStateId, BoundingBox, Identifier, translations, types::UpdateFlags,
};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        BlockPredicate, CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime,
        argument, literal,
    },
    registration::CommandRegistration,
};
use super::execute::{ensure_region_chunks_block_ticking, loaded_block_position_in};
use super::fill::block_region_volume;
use crate::world::World;

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;

/// Vanilla's `816`: known shape, suppressed drops, no block-entity side
/// effects and no `onPlace`.
const STRICT_FLAGS: UpdateFlags = UpdateFlags::UPDATE_KNOWN_SHAPE
    .union(UpdateFlags::UPDATE_SUPPRESS_DROPS)
    .union(UpdateFlags::UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS)
    .union(UpdateFlags::UPDATE_SKIP_ON_PLACE);

/// Vanilla `CloneCommands.Mode`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CloneMode {
    Force,
    Move,
    Normal,
}

impl CloneMode {
    const fn can_overlap(self) -> bool {
        matches!(self, Self::Force | Self::Move)
    }
}

/// Which blocks of the source region are cloned.
#[derive(Clone, Copy)]
enum CloneFilter {
    /// `replace` and the bare form.
    All,
    /// Vanilla `FILTER_AIR`.
    Masked,
    /// `filtered <filter>`.
    Predicate,
}

/// Where a dimension comes from: the source's own world or an argument.
#[derive(Clone, Copy)]
enum DimensionChoice {
    Source,
    Argument(&'static str),
}

#[derive(Clone, Copy)]
struct Dimensions {
    from: DimensionChoice,
    to: DimensionChoice,
}

#[derive(Clone, Copy)]
struct CloneOptions {
    dimensions: Dimensions,
    filter: CloneFilter,
    mode: CloneMode,
    strict: bool,
}

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("clone"), |_| command())
}

fn command() -> Builder {
    literal("clone")
        .then(begin_end_destination(DimensionChoice::Source))
        .then(literal("from").then(
            argument("sourceDimension", SteelArgumentType::world()).then(begin_end_destination(
                DimensionChoice::Argument("sourceDimension"),
            )),
        ))
}

fn begin_end_destination(from: DimensionChoice) -> Builder {
    argument("begin", SteelArgumentType::block_pos()).then(
        argument("end", SteelArgumentType::block_pos())
            .then(destination_and_strict(Dimensions {
                from,
                to: DimensionChoice::Source,
            }))
            .then(literal("to").then(
                argument("targetDimension", SteelArgumentType::world()).then(
                    destination_and_strict(Dimensions {
                        from,
                        to: DimensionChoice::Argument("targetDimension"),
                    }),
                ),
            )),
    )
}

fn destination_and_strict(dimensions: Dimensions) -> Builder {
    mode_suffix(
        argument("destination", SteelArgumentType::block_pos()),
        dimensions,
        false,
    )
    .then(mode_suffix(literal("strict"), dimensions, true))
}

fn mode_suffix(builder: Builder, dimensions: Dimensions, strict: bool) -> Builder {
    let options = CloneOptions {
        dimensions,
        filter: CloneFilter::All,
        mode: CloneMode::Normal,
        strict,
    };
    builder
        .executes(move |context| execute_clone(context, options))
        .then(with_clone_modes(literal("replace"), options))
        .then(with_clone_modes(
            literal("masked"),
            CloneOptions {
                filter: CloneFilter::Masked,
                ..options
            },
        ))
        .then(literal("filtered").then(with_clone_modes(
            argument("filter", SteelArgumentType::block_predicate()),
            CloneOptions {
                filter: CloneFilter::Predicate,
                ..options
            },
        )))
}

fn with_clone_modes(builder: Builder, options: CloneOptions) -> Builder {
    let with_mode = move |mode| CloneOptions { mode, ..options };
    builder
        .executes(move |context| execute_clone(context, with_mode(CloneMode::Normal)))
        .then(
            literal("force")
                .executes(move |context| execute_clone(context, with_mode(CloneMode::Force))),
        )
        .then(
            literal("move")
                .executes(move |context| execute_clone(context, with_mode(CloneMode::Move))),
        )
        .then(
            literal("normal")
                .executes(move |context| execute_clone(context, with_mode(CloneMode::Normal))),
        )
}

fn resolve_dimension(
    context: &SteelCommandContext<CommandSource>,
    choice: DimensionChoice,
) -> Result<Arc<World>, CommandSyntaxError> {
    match choice {
        DimensionChoice::Source => Ok(Arc::clone(context.source().world())),
        DimensionChoice::Argument(name) => context.world_argument(name)?.resolve(context.source()),
    }
}

fn execute_clone(
    context: &SteelCommandContext<CommandSource>,
    options: CloneOptions,
) -> Result<i32, CommandSyntaxError> {
    let from_world = resolve_dimension(context, options.dimensions.from)?;
    let begin = loaded_block_position_in(context, &from_world, "begin")?;
    let end = loaded_block_position_in(context, &from_world, "end")?;
    let to_world = resolve_dimension(context, options.dimensions.to)?;
    let destination = loaded_block_position_in(context, &to_world, "destination")?;
    let predicate = match options.filter {
        CloneFilter::Predicate => Some(context.block_predicate("filter")?),
        CloneFilter::All | CloneFilter::Masked => None,
    };
    let selection = Selection {
        filter: options.filter,
        predicate,
    };
    let regions = CloneRegions::new(begin, end, destination);
    let source = context.source();
    let limit = source.world().get_game_rule(&MAX_BLOCK_MODIFICATIONS);
    check_clone(&from_world, &to_world, regions, options.mode, limit)?;
    let count = clone_blocks(&from_world, &to_world, regions, &selection, options);
    if count == 0 {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_CLONE_FAILED,
        )));
    }
    let message = translations::COMMANDS_CLONE_SUCCESS
        .message([TextComponent::plain(count.to_string())])
        .component();
    source.send_success(&message, true);
    Ok(count)
}

struct Selection<'predicate> {
    filter: CloneFilter,
    predicate: Option<&'predicate BlockPredicate>,
}

impl Selection<'_> {
    fn matches(&self, world: &World, pos: BlockPos) -> bool {
        match (self.filter, self.predicate) {
            (CloneFilter::Masked, _) => !world.get_block_state(pos).is_air(),
            (CloneFilter::Predicate, Some(predicate)) => predicate.matches(world, pos),
            (CloneFilter::All | CloneFilter::Predicate, _) => true,
        }
    }
}

/// The source region, its same-sized destination and the offset between them.
#[derive(Clone, Copy)]
struct CloneRegions {
    from: BoundingBox,
    to: BoundingBox,
    offset: (i32, i32, i32),
}

impl CloneRegions {
    const fn new(begin: BlockPos, end: BlockPos, destination: BlockPos) -> Self {
        let from = BoundingBox::from_corners(begin, end);
        let destination_end = destination.offset(
            from.max_x() - from.min_x(),
            from.max_y() - from.min_y(),
            from.max_z() - from.min_z(),
        );
        let to = BoundingBox::from_corners(destination, destination_end);
        let offset = (
            to.min_x() - from.min_x(),
            to.min_y() - from.min_y(),
            to.min_z() - from.min_z(),
        );
        Self { from, to, offset }
    }
}

/// Vanilla's checks before anything is touched: overlap, size and loaded chunks.
///
/// Steel requires block-ticking chunks rather than vanilla's merely loaded ones, like
/// `/fill`, so neighbor updates always find their radius-one halo. Steel has no debug
/// worlds, so vanilla's `isDebug` failure has no counterpart.
/// `limit` is the source world's `max_block_modifications`, as in vanilla.
fn check_clone(
    from_world: &Arc<World>,
    to_world: &Arc<World>,
    regions: CloneRegions,
    mode: CloneMode,
    limit: i32,
) -> Result<(), CommandSyntaxError> {
    if !mode.can_overlap()
        && Arc::ptr_eq(from_world, to_world)
        && regions.to.intersects(regions.from)
    {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_CLONE_OVERLAP,
        )));
    }
    let area = block_region_volume(regions.from);
    if area > i64::from(limit) {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_CLONE_TOOBIG
                .message([
                    TextComponent::plain(limit.to_string()),
                    TextComponent::plain(area.to_string()),
                ])
                .component(),
        ));
    }
    ensure_region_chunks_block_ticking(from_world, &regions.from)?;
    ensure_region_chunks_block_ticking(to_world, &regions.to)
}

/// Vanilla `CloneBlockInfo`.
struct CloneBlock {
    pos: BlockPos,
    state: BlockStateId,
    block_entity: Option<NbtCompound>,
    previous_state: BlockStateId,
}

/// The selected blocks in vanilla's three lists and the source positions to clear.
#[derive(Default)]
struct CollectedBlocks {
    solid: Vec<CloneBlock>,
    block_entities: Vec<CloneBlock>,
    other: Vec<CloneBlock>,
    clear: VecDeque<BlockPos>,
}

fn collect_blocks(
    from_world: &World,
    to_world: &World,
    regions: CloneRegions,
    selection: &Selection<'_>,
) -> CollectedBlocks {
    let (dx, dy, dz) = regions.offset;
    let from = regions.from;
    let mut blocks = CollectedBlocks::default();
    for z in from.min_z()..=from.max_z() {
        for y in from.min_y()..=from.max_y() {
            for x in from.min_x()..=from.max_x() {
                let source_pos = BlockPos::new(x, y, z);
                if !selection.matches(from_world, source_pos) {
                    continue;
                }
                let pos = source_pos.offset(dx, dy, dz);
                let state = from_world.get_block_state(source_pos);
                let mut block = CloneBlock {
                    pos,
                    state,
                    block_entity: None,
                    previous_state: to_world.get_block_state(pos),
                };
                if let Some(block_entity) = from_world.get_block_entity(source_pos) {
                    // Vanilla keeps `saveCustomOnly` and `components()` side by side;
                    // Steel carries the stored components in the same compound.
                    block.block_entity = Some(block_entity.save_without_metadata());
                    blocks.block_entities.push(block);
                    blocks.clear.push_back(source_pos);
                } else if !state.is_solid_render()
                    && !from_world.is_collision_shape_full_block_at(source_pos, state)
                {
                    blocks.other.push(block);
                    blocks.clear.push_front(source_pos);
                } else {
                    blocks.solid.push(block);
                    blocks.clear.push_back(source_pos);
                }
            }
        }
    }
    blocks
}

/// Vanilla `clone` after its checks: returns how many destination blocks changed.
fn clone_blocks(
    from_world: &Arc<World>,
    to_world: &Arc<World>,
    regions: CloneRegions,
    selection: &Selection<'_>,
    options: CloneOptions,
) -> i32 {
    let blocks = collect_blocks(from_world, to_world, regions, selection);
    let default_flags = if options.strict {
        UpdateFlags::UPDATE_CLIENTS | STRICT_FLAGS
    } else {
        UpdateFlags::UPDATE_CLIENTS
    };
    let barrier = vanilla_blocks::BARRIER.default_state();
    if options.mode == CloneMode::Move {
        for &pos in &blocks.clear {
            from_world.set_block(pos, barrier, default_flags | STRICT_FLAGS);
        }
        let standard_flags = if options.strict {
            default_flags
        } else {
            UpdateFlags::UPDATE_ALL
        };
        let air = vanilla_blocks::AIR.default_state();
        for &pos in &blocks.clear {
            from_world.set_block(pos, air, standard_flags);
        }
    }

    let ordered: Vec<&CloneBlock> = blocks
        .solid
        .iter()
        .chain(&blocks.block_entities)
        .chain(&blocks.other)
        .collect();
    for block in ordered.iter().rev() {
        to_world.set_block(block.pos, barrier, default_flags | STRICT_FLAGS);
    }
    let mut count = 0;
    for block in &ordered {
        if to_world.set_block(block.pos, block.state, default_flags) {
            count += 1;
        }
    }
    for block in &blocks.block_entities {
        load_block_entity(to_world, block);
        to_world.set_block(block.pos, block.state, default_flags);
    }
    if !options.strict {
        for block in ordered.iter().rev() {
            to_world.update_neighbors_on_block_set(block.pos, block.previous_state);
        }
    }
    to_world.copy_block_ticks_from(from_world, regions.from, regions.offset);
    count
}

/// Vanilla's `loadCustomOnly`, `setComponents` and `setChanged` on the new entity.
/// Like vanilla's problem reporter, a load problem is logged and the clone goes on.
fn load_block_entity(world: &World, block: &CloneBlock) {
    let (Some(nbt), Some(block_entity)) = (&block.block_entity, world.get_block_entity(block.pos))
    else {
        return;
    };
    if let Err(error) = block_entity.load_with_owned_components(nbt) {
        log::warn!(
            "Failed to load cloned block entity data at {:?}: {error}",
            block.pos
        );
    }
    block_entity.set_changed();
}

#[cfg(test)]
mod tests;
