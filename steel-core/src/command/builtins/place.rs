//! Vanilla `/place` (`PlaceCommand`): place a configured feature, a jigsaw
//! assembly, a structure or a structure template into the loaded world.
//!
//! Vanilla hands all of them to the `ServerLevel` itself; Steel gives them a
//! live [`WorldGenRegion`], which reads and writes the world the same way.

use std::cell::RefCell;
use std::sync::{Arc, LazyLock};

use glam::IVec3;
use steel_registry::feature::ConfiguredFeatureRef;
use steel_registry::structure::LiquidSettingsData;
use steel_registry::structure_processor::StructureProcessorKind;
use steel_registry::vanilla_template_pools::vanilla_templates;
use steel_registry::{REGISTRY, RegistryExt as _};
use steel_utils::random::RandomSource;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::random::worldgen_random::WorldgenRandom;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BoundingBox, ChunkPos, Identifier, Rotation, translations};
use steel_worldgen::biomes::obfuscate_biome_seed;
use steel_worldgen::structure::{StructureBlockIgnore, StructureMirror};
use text_components::TextComponent;

use super::execute::loaded_block_position;
use crate::command::brigadier::{
    ArgumentType, CommandNodeBuilder, CommandSyntaxError, SuggestionsBuilder,
};
use crate::command::execution::{
    CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime,
    SteelSuggestionContext, argument, literal, suggest_resources,
};
use crate::command::registration::CommandRegistration;
use crate::world::World;
use crate::worldgen::feature::FeatureDecorationRunner;
use crate::worldgen::generator::{ChunkGenerator as _, StructureStartLevel};
use crate::worldgen::region::WorldGenRegion;
use crate::worldgen::structure::piece_placer::StructurePiecePlacer;
use crate::worldgen::structure::{generate_jigsaw, jigsaw_assets};
use crate::worldgen::template::{
    StructurePlaceSettings, StructureProcessorRandom, StructureTemplate,
};

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;
type Context = SteelCommandContext<CommandSource>;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("place"), |_| command())
}

/// Vanilla `PlaceCommand.register`.
fn command() -> Builder {
    literal("place")
        .then(
            literal("feature").then(
                argument("feature", SteelArgumentType::feature())
                    .executes(|context| place_feature(context, source_block_pos(context)))
                    .then(
                        argument("pos", SteelArgumentType::block_pos()).executes(|context| {
                            place_feature(context, loaded_block_position(context, "pos")?)
                        }),
                    ),
            ),
        )
        .then(literal("jigsaw").then(jigsaw_arguments()))
        .then(
            literal("structure").then(
                argument("structure", SteelArgumentType::structure_key())
                    .executes(|context| place_structure(context, source_block_pos(context)))
                    .then(
                        argument("pos", SteelArgumentType::block_pos()).executes(|context| {
                            place_structure(context, loaded_block_position(context, "pos")?)
                        }),
                    ),
            ),
        )
        .then(literal("template").then(template_arguments()))
}

/// The `jigsaw` branch: `<pool> <target> <max_depth> [position]`.
fn jigsaw_arguments() -> Builder {
    let max_depth = argument("max_depth", ArgumentType::integer(1, 20))
        .executes(|context| place_jigsaw(context, source_block_pos(context)))
        .then(
            argument("position", SteelArgumentType::block_pos()).executes(|context| {
                place_jigsaw(context, loaded_block_position(context, "position")?)
            }),
        );
    argument("pool", SteelArgumentType::template_pool_key())
        .then(argument("target", SteelArgumentType::identifier()).then(max_depth))
}

/// The `template` branch: every trailing argument is optional, in order.
fn template_arguments() -> Builder {
    let strict = argument("strict", ArgumentType::bool())
        .executes(|context| place_template(context, TemplateOptions::read(context, 6)?));
    let seed = argument("seed", ArgumentType::integer(i32::MIN, i32::MAX))
        .executes(|context| place_template(context, TemplateOptions::read(context, 5)?))
        .then(strict);
    let integrity = argument("integrity", ArgumentType::float(0.0, 1.0))
        .executes(|context| place_template(context, TemplateOptions::read(context, 4)?))
        .then(seed);
    let mirror = argument("mirror", SteelArgumentType::template_mirror())
        .executes(|context| place_template(context, TemplateOptions::read(context, 3)?))
        .then(integrity);
    let rotation = argument("rotation", SteelArgumentType::template_rotation())
        .executes(|context| place_template(context, TemplateOptions::read(context, 2)?))
        .then(mirror);
    let pos = argument("pos", SteelArgumentType::block_pos())
        .executes(|context| place_template(context, TemplateOptions::read(context, 1)?))
        .then(rotation);
    argument("template", SteelArgumentType::identifier())
        .suggests(suggest_templates)
        .executes(|context| place_template(context, TemplateOptions::read(context, 0)?))
        .then(pos)
}

/// Vanilla `BlockPos.containing(source.getPosition())`.
fn source_block_pos(context: &Context) -> BlockPos {
    BlockPos::from(context.source().position())
}

/// Vanilla `PlaceCommand.placeFeature`.
fn place_feature(context: &Context, pos: BlockPos) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let world = source.world();
    let feature = context.feature("feature")?;
    let chunk = chunk_containing(pos);
    check_loaded(
        world,
        ChunkPos::new(chunk.0.x - 1, chunk.0.y - 1),
        ChunkPos::new(chunk.0.x + 1, chunk.0.y + 1),
    )?;

    let mut region = live_region(world, chunk);
    // Vanilla passes `level.getRandom()`, an unseeded random.
    let mut random = WorldgenRandom::from_seed(rand::random());
    let placed = FeatureDecorationRunner::place_feature_in_world(
        &mut region,
        &ConfiguredFeatureRef::Reference(feature),
        &mut random,
        pos,
    );
    if !placed {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_PLACE_FEATURE_FAILED,
        )));
    }
    source.send_success(
        &translations::COMMANDS_PLACE_FEATURE_SUCCESS
            .message([
                TextComponent::plain(feature.key.to_string()),
                TextComponent::plain(pos.x().to_string()),
                TextComponent::plain(pos.y().to_string()),
                TextComponent::plain(pos.z().to_string()),
            ])
            .component(),
        true,
    );
    Ok(1)
}

/// Vanilla `PlaceCommand.placeJigsaw`.
fn place_jigsaw(context: &Context, pos: BlockPos) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let world = source.world();
    let pool = context.identifier("pool")?;
    let (pools, _) = jigsaw_assets(world);
    if !pools.contains_key(pool) {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_PLACE_JIGSAW_INVALID
                .message([pool.to_string()])
                .component(),
        ));
    }
    let target = context.identifier("target")?;
    let max_depth = context.integer("max_depth")?;
    let chunk = chunk_containing(pos);
    check_loaded(world, chunk, chunk)?;
    if !generate_jigsaw(world, pool, target, max_depth, pos, false) {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_PLACE_JIGSAW_FAILED,
        )));
    }
    source.send_success(
        &translations::COMMANDS_PLACE_JIGSAW_SUCCESS
            .message([
                TextComponent::plain(pos.x().to_string()),
                TextComponent::plain(pos.y().to_string()),
                TextComponent::plain(pos.z().to_string()),
            ])
            .component(),
        true,
    );
    Ok(1)
}

/// Vanilla `PlaceCommand.placeStructure`.
fn place_structure(context: &Context, pos: BlockPos) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let world = source.world();
    let id = context.identifier("structure")?;
    let Some(structure) = REGISTRY.structures.by_key(id) else {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_PLACE_STRUCTURE_INVALID
                .message([id.to_string()])
                .component(),
        ));
    };
    let min_y = world.get_min_y();
    let height = world.get_height();
    let failed = || {
        CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_PLACE_STRUCTURE_FAILED,
        ))
    };
    let mut start = world
        .chunk_map
        .world_gen_context
        .generator
        .generate_structure_start(
            id,
            chunk_containing(pos),
            StructureStartLevel {
                seed: world.seed(),
                min_y,
                height,
                sea_level: world.sea_level,
            },
        )
        .ok_or_else(failed)?;
    let bounds = start.bounding_box.ok_or_else(failed)?;
    let min_chunk = ChunkPos::new(bounds.min_x() >> 4, bounds.min_z() >> 4);
    let max_chunk = ChunkPos::new(bounds.max_x() >> 4, bounds.max_z() >> 4);
    check_loaded(world, min_chunk, max_chunk)?;

    let mut region = live_region(world, min_chunk);
    // Vanilla passes `level.getRandom()` to every chunk.
    let mut random = WorldgenRandom::from_seed(rand::random());
    let biome_zoom_seed = obfuscate_biome_seed(world.seed());
    for chunk_x in min_chunk.0.x..=max_chunk.0.x {
        for chunk_z in min_chunk.0.y..=max_chunk.0.y {
            let clip = BoundingBox::new(
                IVec3::new(chunk_x << 4, min_y, chunk_z << 4),
                IVec3::new((chunk_x << 4) + 15, min_y + height, (chunk_z << 4) + 15),
            );
            StructurePiecePlacer::place_start_in_chunk(
                &mut region,
                &REGISTRY,
                structure,
                &mut start,
                clip,
                &mut random,
                biome_zoom_seed,
            );
        }
    }
    source.send_success(
        &translations::COMMANDS_PLACE_STRUCTURE_SUCCESS
            .message([
                TextComponent::plain(id.to_string()),
                TextComponent::plain(pos.x().to_string()),
                TextComponent::plain(pos.y().to_string()),
                TextComponent::plain(pos.z().to_string()),
            ])
            .component(),
        true,
    );
    Ok(1)
}

/// The arguments of `place template` past the template id, with vanilla's
/// defaults for the ones left out.
struct TemplateOptions {
    pos: BlockPos,
    rotation: Rotation,
    mirror: StructureMirror,
    integrity: f32,
    seed: i32,
    strict: bool,
}

impl TemplateOptions {
    /// Reads the first `given` optional arguments; the rest keep vanilla's defaults.
    fn read(context: &Context, given: usize) -> Result<Self, CommandSyntaxError> {
        Ok(Self {
            pos: if given >= 1 {
                loaded_block_position(context, "pos")?
            } else {
                source_block_pos(context)
            },
            rotation: if given >= 2 {
                context.template_rotation("rotation")?
            } else {
                Rotation::None
            },
            mirror: if given >= 3 {
                context.template_mirror("mirror")?
            } else {
                StructureMirror::None
            },
            integrity: if given >= 4 {
                context.float("integrity")?
            } else {
                1.0
            },
            seed: if given >= 5 {
                context.integer("seed")?
            } else {
                0
            },
            strict: given >= 6 && context.boolean("strict")?,
        })
    }
}

/// Vanilla `PlaceCommand.placeTemplate`.
fn place_template(context: &Context, options: TemplateOptions) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let world = source.world();
    let id = context.identifier("template")?;
    // Vanilla's template manager also reads the world's `generated` folder;
    // Steel has only the bundled vanilla templates.
    let template = StructureTemplate::load_vanilla(&REGISTRY, id).map_err(|_| {
        CommandSyntaxError::dynamic(
            translations::COMMANDS_PLACE_TEMPLATE_INVALID
                .message([id.to_string()])
                .component(),
        )
    })?;
    let pos = options.pos;
    let size = template.size(Rotation::None);
    check_loaded(
        world,
        chunk_containing(pos),
        chunk_containing(BlockPos(pos.0 + size)),
    )?;

    let processors = [StructureProcessorKind::BlockRot {
        rottable_blocks: None,
        integrity: options.integrity,
    }];
    let seed = template_seed(options.seed);
    let processor_random = RefCell::new(LegacyRandom::from_seed(seed));
    let rotting = options.integrity < 1.0;
    let settings = StructurePlaceSettings {
        mirror: options.mirror,
        rotation: options.rotation,
        rotation_pivot: BlockPos::ZERO,
        // Vanilla leaves the box unset; the world bounds every write anyway.
        bounding_box: BoundingBox::new(
            IVec3::new(i32::MIN, world.get_min_y(), i32::MIN),
            IVec3::new(
                i32::MAX,
                world.get_min_y() + world.get_height() - 1,
                i32::MAX,
            ),
        ),
        processors: if rotting { &processors } else { &[] },
        block_ignore: StructureBlockIgnore::None,
        late_block_ignore: StructureBlockIgnore::None,
        replace_jigsaws: false,
        projection: None,
        processor_random: if rotting {
            StructureProcessorRandom::Separate(&processor_random)
        } else {
            StructureProcessorRandom::Positional
        },
        liquid_settings: LiquidSettingsData::ApplyWaterlogging,
    };
    // Vanilla flags `2 | (strict ? 816 : 0)`. Its `setKnownShape(strict)` needs no
    // field here: Steel's placement reads the shape choice from `UPDATE_KNOWN_SHAPE`,
    // which 816 carries.
    let flags = if options.strict {
        UpdateFlags::UPDATE_CLIENTS
            | UpdateFlags::UPDATE_KNOWN_SHAPE
            | UpdateFlags::UPDATE_SUPPRESS_DROPS
            | UpdateFlags::UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS
            | UpdateFlags::UPDATE_SKIP_ON_PLACE
    } else {
        UpdateFlags::UPDATE_CLIENTS
    };

    let mut region = live_region(world, chunk_containing(pos));
    let placed = template.place_in_world(
        &mut region,
        &REGISTRY,
        pos,
        pos,
        &settings,
        &mut LegacyRandom::from_seed(seed),
        flags,
    );
    if !placed {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_PLACE_TEMPLATE_FAILED,
        )));
    }
    source.send_success(
        &translations::COMMANDS_PLACE_TEMPLATE_SUCCESS
            .message([
                TextComponent::plain(id.to_string()),
                TextComponent::plain(pos.x().to_string()),
                TextComponent::plain(pos.y().to_string()),
                TextComponent::plain(pos.z().to_string()),
            ])
            .component(),
        true,
    );
    Ok(1)
}

/// The seed of vanilla `StructureBlockEntity.createRandom(seed)`, which `/place
/// template` calls twice: seed 0 means the clock, the same for both randoms.
fn template_seed(seed: i32) -> u64 {
    if seed == 0 {
        rand::random()
    } else {
        i64::from(seed).cast_unsigned()
    }
}

/// A live region over `world`, with vanilla's unseeded level random.
fn live_region(world: &Arc<World>, center: ChunkPos) -> WorldGenRegion<'_> {
    WorldGenRegion::new_live(
        &world.chunk_map.world_gen_context,
        Arc::clone(world),
        center,
        RandomSource::Legacy(LegacyRandom::from_seed(rand::random())),
    )
}

const fn chunk_containing(pos: BlockPos) -> ChunkPos {
    ChunkPos::new(pos.x() >> 4, pos.z() >> 4)
}

/// Vanilla `PlaceCommand.checkLoaded`: every chunk in the range must be loaded.
fn check_loaded(world: &World, min: ChunkPos, max: ChunkPos) -> Result<(), CommandSyntaxError> {
    for chunk_x in min.0.x..=max.0.x {
        for chunk_z in min.0.y..=max.0.y {
            let corner = BlockPos::new(chunk_x << 4, 0, chunk_z << 4);
            if !world.is_full_chunk_loaded_at(corner) {
                return Err(CommandSyntaxError::dynamic(TextComponent::from(
                    &translations::ARGUMENT_POS_UNLOADED,
                )));
            }
        }
    }
    Ok(())
}

/// Vanilla `PlaceCommand.SUGGEST_TEMPLATES`.
fn suggest_templates(
    _context: &SteelSuggestionContext<'_, CommandSource>,
    builder: &mut SuggestionsBuilder<'_>,
) {
    suggest_resources(template_ids().iter(), builder);
}

/// Every bundled vanilla template id.
fn template_ids() -> &'static [Identifier] {
    static IDS: LazyLock<Vec<Identifier>> =
        LazyLock::new(|| vanilla_templates().into_iter().map(|(id, _)| id).collect());
    &IDS
}
