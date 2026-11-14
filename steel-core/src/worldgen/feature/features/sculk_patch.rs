//! Vanilla `SculkPatchFeature`: a world generation sculk spreader run from
//! one spot.

use super::super::prelude::*;
use super::super::runner::FeatureDecorationRunner;
use crate::behavior::blocks::spreader::{SculkLevel, SculkSpreader};
use steel_registry::vanilla_block_entity_types;

impl SculkLevel for WorldGenRegion<'_> {
    fn should_tick_blocks_at(&self, _pos: BlockPos) -> bool {
        true
    }

    fn mark_pos_for_postprocessing(&self, pos: BlockPos) {
        WorldGenRegion::mark_pos_for_postprocessing(self, pos);
    }

    /// Vanilla `WorldGenRegion.setBlock` leaves an empty block entity for
    /// the chunk to create later.
    fn on_growth_placed(&self, pos: BlockPos, state: BlockStateId) {
        let block_entity_type = if state.get_block() == &vanilla_blocks::SCULK_SENSOR {
            &vanilla_block_entity_types::SCULK_SENSOR
        } else if state.get_block() == &vanilla_blocks::SCULK_SHRIEKER {
            &vanilla_block_entity_types::SCULK_SHRIEKER
        } else {
            return;
        };
        FeatureDecorationRunner::set_empty_block_entity(self, pos, block_entity_type, state);
    }
}

impl FeatureDecorationRunner {
    pub(in crate::worldgen::feature) fn place_sculk_patch_feature(
        region: &mut WorldGenRegion<'_>,
        _registry: &Registry,
        random: &mut WorldgenRandom,
        config: &SculkPatchConfiguration,
        origin: BlockPos,
    ) -> bool {
        if !Self::can_sculk_spread_from(region, origin) {
            return false;
        }

        let region: &WorldGenRegion<'_> = region;
        let mut spreader = SculkSpreader::world_gen();
        let total_rounds = config.spread_rounds + config.growth_rounds;
        for round in 0..total_rounds {
            for _ in 0..config.charge_count {
                spreader.add_cursors(origin, config.amount_per_charge);
            }

            let spread_veins = round < config.spread_rounds;
            for _ in 0..config.spread_attempts {
                spreader.update_cursors(region, origin, random, spread_veins);
            }

            spreader.clear();
        }

        true
    }

    /// Vanilla `SculkPatchFeature.canSpreadFrom`.
    fn can_sculk_spread_from(region: &WorldGenRegion<'_>, origin: BlockPos) -> bool {
        let start = region.block_state(origin);
        let block = start.get_block();
        if block == &vanilla_blocks::SCULK || block == &vanilla_blocks::SCULK_VEIN {
            return true;
        }

        if !start.is_air()
            && (block != &vanilla_blocks::WATER || !get_fluid_state_from_block(start).is_source())
        {
            return false;
        }

        Self::VANILLA_DIRECTION_VALUES.iter().any(|direction| {
            let pos = origin.relative(*direction);
            let state = region.block_state(pos);
            shapes::is_offset_shape_full_block(state.get_collision_shape_at(pos))
        })
    }
}
