//! Placing a template into the loaded world as vanilla `/place template` and
//! structure blocks do: `StructureTemplate.placeInWorld` on the `ServerLevel`.

use std::cell::RefCell;
use std::sync::Arc;

use glam::IVec3;
use steel_registry::REGISTRY;
use steel_registry::structure::LiquidSettingsData;
use steel_registry::structure_processor::StructureProcessorKind;
use steel_utils::random::RandomSource;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BoundingBox, ChunkPos, Rotation};
use steel_worldgen::structure::{StructureBlockIgnore, StructureMirror};

use super::{StructurePlaceSettings, StructureProcessorRandom, StructureTemplate};
use crate::world::World;
use crate::worldgen::region::WorldGenRegion;

/// The settings vanilla `/place template` and structure blocks give
/// `placeInWorld`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LivePlacement {
    /// Vanilla `setRotation`.
    pub(crate) rotation: Rotation,
    /// Vanilla `setMirror`.
    pub(crate) mirror: StructureMirror,
    /// Below 1, vanilla adds a `BlockRotProcessor` with this integrity.
    pub(crate) integrity: f32,
    /// The seed of vanilla `StructureBlockEntity.createRandom`.
    pub(crate) seed: u64,
    /// Vanilla `setKnownShape(strict)` and flags `2 | (strict ? 816 : 0)`.
    pub(crate) strict: bool,
}

/// The seed of vanilla `StructureBlockEntity.createRandom(seed)`: seed 0
/// means the clock.
pub(crate) fn template_seed(seed: i64) -> u64 {
    if seed == 0 {
        rand::random()
    } else {
        seed.cast_unsigned()
    }
}

impl StructureTemplate {
    /// Vanilla `setIgnoreEntities(true)`: the template places no entities.
    pub(crate) fn clear_entities(&mut self) {
        self.entities.clear();
    }
}

/// The chunk holding `pos`.
pub(crate) const fn chunk_containing(pos: BlockPos) -> ChunkPos {
    ChunkPos::new(pos.x() >> 4, pos.z() >> 4)
}

/// A live region over `world`, with vanilla's unseeded level random.
pub(crate) fn live_region(world: &Arc<World>, center: ChunkPos) -> WorldGenRegion<'_> {
    WorldGenRegion::new_live(
        &world.chunk_map.world_gen_context,
        Arc::clone(world),
        center,
        RandomSource::Legacy(LegacyRandom::from_seed(rand::random())),
    )
}

/// Vanilla `template.placeInWorld(level, pos, pos, settings, random, flags)`
/// with the settings of `placement`. Returns whether anything was placed.
pub(crate) fn place_live(
    world: &Arc<World>,
    template: &StructureTemplate,
    pos: BlockPos,
    placement: LivePlacement,
) -> bool {
    let processors = [StructureProcessorKind::BlockRot {
        rottable_blocks: None,
        integrity: placement.integrity.clamp(0.0, 1.0),
    }];
    let processor_random = RefCell::new(LegacyRandom::from_seed(placement.seed));
    let rotting = placement.integrity < 1.0;
    let settings = StructurePlaceSettings {
        mirror: placement.mirror,
        rotation: placement.rotation,
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
    // Vanilla's `setKnownShape(strict)` needs no field here: Steel's placement
    // reads the shape choice from `UPDATE_KNOWN_SHAPE`, which 816 carries.
    let flags = if placement.strict {
        UpdateFlags::UPDATE_CLIENTS
            | UpdateFlags::UPDATE_KNOWN_SHAPE
            | UpdateFlags::UPDATE_SUPPRESS_DROPS
            | UpdateFlags::UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS
            | UpdateFlags::UPDATE_SKIP_ON_PLACE
    } else {
        UpdateFlags::UPDATE_CLIENTS
    };
    let mut region = live_region(world, chunk_containing(pos));
    template.place_in_world(
        &mut region,
        &REGISTRY,
        pos,
        pos,
        &settings,
        &mut LegacyRandom::from_seed(placement.seed),
        flags,
    )
}
