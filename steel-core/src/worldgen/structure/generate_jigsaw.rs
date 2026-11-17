//! Vanilla `JigsawPlacement.generateJigsaw`: grows a jigsaw structure in a
//! live world, for `/place jigsaw` and the jigsaw block's "Generate" button.

use std::sync::Arc;

use glam::IVec3;
use rustc_hash::FxHashMap;
use steel_registry::REGISTRY;
use steel_registry::structure::{DimensionPadding, JigsawConfig, LiquidSettingsData, StartHeight};
use steel_registry::template_pool::{TemplateData, TemplatePoolData};
use steel_utils::random::RandomSource;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::random::worldgen_random::WorldgenRandom;
use steel_utils::{BlockPos, BoundingBox, ChunkPos, Identifier};
use steel_worldgen::biomes::obfuscate_biome_seed;
use steel_worldgen::structure::{StructurePiecePayload, jigsaw, vanilla_template_assets};

use crate::world::World;
use crate::worldgen::generator::ChunkGenerator as _;
use crate::worldgen::region::WorldGenRegion;
use crate::worldgen::structure::piece_placer::StructurePiecePlacer;

/// The template pools and templates jigsaw assembly reads: the world
/// generator's, or the bundled vanilla ones for a generator without structures.
pub(crate) fn jigsaw_assets(
    world: &World,
) -> (
    &FxHashMap<Identifier, TemplatePoolData>,
    &FxHashMap<Identifier, TemplateData>,
) {
    match world
        .chunk_map
        .world_gen_context
        .generator
        .structure_generator()
    {
        Some(generator) => (generator.template_pools(), generator.templates()),
        None => vanilla_template_assets(),
    }
}

/// Vanilla `generateJigsaw`: assembles up to `max_depth` pieces from `pool`,
/// starting at the jigsaw named `target`, and places them around `position`.
/// Jigsaw blocks in the pieces stay when `keep_jigsaws` is set. Returns
/// whether the assembly found a start piece.
pub(crate) fn generate_jigsaw(
    world: &Arc<World>,
    pool: &Identifier,
    target: &Identifier,
    max_depth: i32,
    position: BlockPos,
    keep_jigsaws: bool,
) -> bool {
    let (pools, templates) = jigsaw_assets(world);
    // Vanilla looks the pool up with `getOrThrow`; an unknown pool generates nothing here.
    if !pools.contains_key(pool) {
        return false;
    }
    let chunk = ChunkPos::new(position.x() >> 4, position.z() >> 4);
    // `addPieces` with `generateJigsaw`'s fixed arguments: no expansion hack, no
    // heightmap projection, distance 128, no aliases, the default padding and
    // liquid settings. A constant start height takes nothing from the random.
    let config = JigsawConfig {
        start_pool: pool.clone(),
        max_depth,
        use_expansion_hack: false,
        project_start_to_heightmap: None,
        start_height: StartHeight::Constant(position.y()),
        max_distance_from_center: 128,
        start_jigsaw_name: Some(target.clone()),
        dimension_padding: DimensionPadding { bottom: 0, top: 0 },
        pool_aliases: Vec::new(),
        liquid_settings: LiquidSettingsData::ApplyWaterlogging,
    };
    // The generation context's random: `setLargeFeatureSeed` at the start chunk.
    let mut rng = LegacyRandom::from_seed(0);
    rng.set_large_feature_seed(world.seed(), chunk.0.x, chunk.0.y);
    let min_y = world.get_min_y();
    let Some(assembly) = jigsaw::assemble(
        &config,
        &mut rng,
        position.x(),
        position.z(),
        pools,
        templates,
        &FxHashMap::default(),
        // Only read for heightmap projection, which `generateJigsaw` never asks for.
        &mut |_, _| min_y,
        min_y,
        min_y + world.get_height(),
    ) else {
        return false;
    };

    // Vanilla places every piece with an infinite box. Its `ServerLevel` loads
    // any chunk a piece reaches; Steel's live region writes loaded chunks only.
    let infinite = BoundingBox::new(IVec3::splat(i32::MIN), IVec3::splat(i32::MAX));
    let mut region = WorldGenRegion::new_live(
        &world.chunk_map.world_gen_context,
        Arc::clone(world),
        chunk,
        RandomSource::Legacy(LegacyRandom::from_seed(rand::random())),
    );
    let mut random = WorldgenRandom::from_seed(rand::random());
    let biome_zoom_seed = obfuscate_biome_seed(world.seed());
    for piece in assembly.into_structure_pieces(config.liquid_settings) {
        if let StructurePiecePayload::Jigsaw(data) = &piece.payload {
            StructurePiecePlacer::place_jigsaw_piece(
                &mut region,
                &REGISTRY,
                data,
                position,
                infinite,
                &mut random,
                biome_zoom_seed,
                keep_jigsaws,
            );
        }
    }
    true
}
