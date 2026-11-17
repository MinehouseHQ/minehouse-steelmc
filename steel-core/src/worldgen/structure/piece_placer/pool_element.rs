use steel_registry::structure::LiquidSettingsData;
use steel_registry::structure_processor::StructureProcessorKind;
use steel_registry::template_pool::{PoolElement, ProcessorList, Projection};
use steel_registry::{Registry, RegistryExt};
use steel_utils::random::worldgen_random::WorldgenRandom;
use steel_utils::{BlockPos, BoundingBox, Identifier, Rotation};

use crate::worldgen::feature::FeatureDecorationRunner;
use crate::worldgen::region::WorldGenRegion;
use crate::worldgen::template::{
    StructurePlaceSettings, StructureProcessorRandom, StructureTemplate,
};
use steel_worldgen::structure::jigsaw::JigsawPieceData;
use steel_worldgen::structure::{StructureBlockIgnore, StructureMirror};

use super::StructurePiecePlacer;

impl StructurePiecePlacer {
    /// Vanilla `PoolElementStructurePiece.place`, keeping the pool element's
    /// jigsaw blocks when `keep_jigsaws` is set.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors PoolElementStructurePiece.place inputs"
    )]
    pub(crate) fn place_jigsaw_piece(
        region: &mut WorldGenRegion<'_>,
        registry: &Registry,
        data: &JigsawPieceData,
        reference_pos: BlockPos,
        clip: BoundingBox,
        random: &mut WorldgenRandom,
        biome_zoom_seed: i64,
        keep_jigsaws: bool,
    ) -> bool {
        Self::place_pool_element(
            region,
            registry,
            &data.pool_element,
            BlockPos::new(data.position.x, data.position.y, data.position.z),
            reference_pos,
            data.rotation,
            clip,
            random,
            data.liquid_settings,
            biome_zoom_seed,
            keep_jigsaws,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors StructurePoolElement.place inputs"
    )]
    pub(super) fn place_pool_element(
        region: &mut WorldGenRegion<'_>,
        registry: &Registry,
        element: &PoolElement,
        position: BlockPos,
        reference_pos: BlockPos,
        rotation: Rotation,
        clip: BoundingBox,
        random: &mut WorldgenRandom,
        liquid_settings: LiquidSettingsData,
        biome_zoom_seed: i64,
        keep_jigsaws: bool,
    ) -> bool {
        match element {
            PoolElement::Single {
                location,
                processors,
                projection,
            } => Self::place_single_pool_element(
                region,
                registry,
                location,
                processors,
                *projection,
                StructureBlockIgnore::StructureBlock,
                StructureBlockIgnore::None,
                position,
                reference_pos,
                rotation,
                clip,
                random,
                liquid_settings,
                keep_jigsaws,
            ),
            PoolElement::LegacySingle {
                location,
                processors,
                projection,
            } => Self::place_single_pool_element(
                region,
                registry,
                location,
                processors,
                *projection,
                StructureBlockIgnore::None,
                StructureBlockIgnore::StructureAndAir,
                position,
                reference_pos,
                rotation,
                clip,
                random,
                liquid_settings,
                keep_jigsaws,
            ),
            PoolElement::Empty => true,
            PoolElement::Feature { feature, .. } => {
                FeatureDecorationRunner::place_structure_pool_feature(
                    region,
                    registry,
                    random,
                    position,
                    feature,
                    biome_zoom_seed,
                )
            }
            PoolElement::List { elements, .. } => {
                for element in elements {
                    if !Self::place_pool_element(
                        region,
                        registry,
                        element,
                        position,
                        reference_pos,
                        rotation,
                        clip,
                        random,
                        liquid_settings,
                        biome_zoom_seed,
                        keep_jigsaws,
                    ) {
                        return false;
                    }
                }
                true
            }
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors SinglePoolElement.place and StructureTemplate.placeInWorld"
    )]
    fn place_single_pool_element(
        region: &mut WorldGenRegion<'_>,
        registry: &Registry,
        location: &Identifier,
        processors: &ProcessorList,
        projection: Projection,
        block_ignore: StructureBlockIgnore,
        late_block_ignore: StructureBlockIgnore,
        position: BlockPos,
        reference_pos: BlockPos,
        rotation: Rotation,
        clip: BoundingBox,
        random: &mut WorldgenRandom,
        liquid_settings: LiquidSettingsData,
        keep_jigsaws: bool,
    ) -> bool {
        let template = match StructureTemplate::load_vanilla(registry, location) {
            Ok(template) => template,
            Err(err) => panic!("{err}"),
        };
        let processor_list = Self::pool_processors(registry, processors);
        let settings = StructurePlaceSettings {
            mirror: StructureMirror::None,
            rotation,
            rotation_pivot: BlockPos::ZERO,
            bounding_box: clip,
            processors: processor_list,
            block_ignore,
            late_block_ignore,
            replace_jigsaws: !keep_jigsaws,
            projection: Some(projection),
            processor_random: StructureProcessorRandom::Positional,
            liquid_settings,
        };

        template.place_in_world(
            region,
            registry,
            position,
            reference_pos,
            &settings,
            random,
            Self::JIGSAW_UPDATE_FLAGS,
        )
    }

    fn pool_processors<'a>(
        registry: &'a Registry,
        processors: &'a ProcessorList,
    ) -> &'a [StructureProcessorKind] {
        match processors {
            ProcessorList::Empty => &[],
            ProcessorList::Registry(key) => {
                let Some(processor_list) = registry.structure_processors.by_key(key) else {
                    panic!("template pool references unknown processor list {key}");
                };
                &processor_list.data.processors
            }
        }
    }
}
