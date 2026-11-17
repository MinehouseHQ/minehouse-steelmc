//! Vanilla `BiomeSource.findClosestBiome3d`, spread over several polls.
//!
//! Vanilla samples the whole spiral in one call. Steel samples for a bounded
//! time per poll so a long `/locate biome` does not stall the tick, and
//! visits the columns in vanilla's order, so the result is the same.

use std::sync::Arc;
use std::time::{Duration, Instant};

use steel_registry::biome::BiomeRef;
use steel_utils::types::SpiralAround;
use steel_utils::{BlockPos, Direction};

use crate::world::{LevelReader as _, World};
use crate::worldgen::generator::ChunkGenerator as _;

/// Time one poll may spend sampling before it yields to the tick.
const POLL_BUDGET: Duration = Duration::from_millis(10);

/// What one poll of a [`BiomeSearch`] produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BiomeSearchPoll {
    /// The search has columns left to sample.
    Pending,
    /// The closest allowed biome and the sample position it was found at.
    Found(BlockPos, BiomeRef),
    /// No allowed biome lies within the search radius.
    NotFound,
}

/// A running vanilla `findClosestBiome3d`.
pub struct BiomeSearch {
    world: Arc<World>,
    candidates: Vec<BiomeRef>,
    origin: BlockPos,
    sample_ys: Vec<i32>,
    columns: SpiralAround,
    horizontal_resolution: i32,
}

impl BiomeSearch {
    /// Starts a search around `origin`, or returns `None` when the world's
    /// generator can produce none of the allowed biomes.
    #[must_use]
    pub fn new(
        world: Arc<World>,
        origin: BlockPos,
        search_radius: i32,
        horizontal_resolution: i32,
        vertical_resolution: i32,
        allowed: impl Fn(BiomeRef) -> bool,
    ) -> Option<Self> {
        let candidates: Vec<BiomeRef> = world
            .chunk_map
            .world_gen_context
            .generator
            .possible_biomes()
            .into_iter()
            .filter(|biome| allowed(biome))
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let sample_radius = search_radius.div_euclid(horizontal_resolution);
        let sample_ys = out_from_origin(
            origin.y(),
            world.min_y() + 1,
            world.min_y() + world.height(),
            vertical_resolution,
        );
        let columns = BlockPos::spiral_around(
            BlockPos::new(0, 0, 0),
            sample_radius,
            Direction::East,
            Direction::South,
        );
        Some(Self {
            world,
            candidates,
            origin,
            sample_ys,
            columns,
            horizontal_resolution,
        })
    }

    /// Samples the next columns of the spiral.
    pub fn poll(&mut self) -> BiomeSearchPoll {
        let generator = &self.world.chunk_map.world_gen_context.generator;
        let started = Instant::now();
        while started.elapsed() < POLL_BUDGET {
            let Some(column) = self.columns.next() else {
                return BiomeSearchPoll::NotFound;
            };
            let block_x = self.origin.x() + column.x() * self.horizontal_resolution;
            let block_z = self.origin.z() + column.z() * self.horizontal_resolution;
            for &block_y in &self.sample_ys {
                let biome = generator.noise_biome(block_x >> 2, block_y >> 2, block_z >> 2);
                if self.candidates.contains(&biome) {
                    return BiomeSearchPoll::Found(BlockPos::new(block_x, block_y, block_z), biome);
                }
            }
        }
        BiomeSearchPoll::Pending
    }
}

/// Vanilla `Mth.outFromOrigin`: the values from `lower` to `upper` in steps of
/// `step`, starting at `origin` and alternating outwards, above first.
#[must_use]
pub fn out_from_origin(origin: i32, lower: i32, upper: i32, step: i32) -> Vec<i32> {
    let clamped = origin.clamp(lower, upper);
    let mut values = Vec::new();
    let mut cursor = clamped;
    loop {
        let distance = (clamped - cursor).abs();
        if clamped - distance < lower && clamped + distance > upper {
            return values;
        }
        values.push(cursor);
        let previous_was_negative = cursor <= clamped;
        let can_move_positive = clamped + distance + step <= upper;
        if !previous_was_negative || !can_move_positive {
            let attempted = clamped - distance - if previous_was_negative { step } else { 0 };
            if attempted >= lower {
                cursor = attempted;
                continue;
            }
        }
        cursor = clamped + distance + step;
    }
}

#[cfg(test)]
mod tests {
    use super::out_from_origin;

    #[test]
    fn out_from_origin_alternates_like_vanilla() {
        assert_eq!(out_from_origin(5, 0, 10, 2), [5, 7, 3, 9, 1]);
        assert_eq!(
            out_from_origin(64, -63, 320, 64),
            [64, 128, 0, 192, 256, 320]
        );
        assert_eq!(
            out_from_origin(-100, -63, 320, 64),
            [-63, 1, 65, 129, 193, 257]
        );
    }
}
