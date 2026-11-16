//! Regionized ticking: the parallel island phases of a world tick.
//!
//! Runs only with [`regionized_ticking_enabled`]. A world tick has two island
//! phases, each splitting the world's ticking chunks into [`Islands`] that run
//! on the tick pool, in Vanilla order for their own chunks:
//!
//! 1. Scheduled ticks, before the chunk-source phase: the world selects this
//!    tick's block ticks in Vanilla order, each island runs its share, then the
//!    same for fluid ticks.
//! 2. After the chunk-source phase: natural spawning, random ticks, block events
//!    and entities.
//!
//! Islands are far enough apart that none can reach another within a tick, so
//! only the interleaving between islands differs from the sequential tick, and
//! that interleaving is not observable.
//!
//! Work that is not local to an island stays sequential. Block events wait in the
//! world queue between the phases and are handed to the island that owns their
//! position, ahead of its own events, as in Vanilla's single queue; events outside
//! every island run before the islands. Entities outside every island tick after
//! them.

use std::time::{Duration, Instant};

use rayon::iter::{IndexedParallelIterator as _, IntoParallelIterator as _, ParallelIterator as _};
use rustc_hash::{FxHashMap, FxHashSet};

use super::{
    Arc, BlockRandomPositionGenerator, ChunkMap, ChunkPos, GameplayChunkLookupCacheScope,
    TickingChunkSnapshot,
};
use crate::entity::Entity as _;
use crate::world::World;
use crate::world::region::{DeferredWork, Islands, RegionQueues, RegionScope};
use crate::world::tick_pool::install_on_tick_pool;
use crate::world::tick_scheduler::{ScheduledTick, TickKey};
use crate::world::ticking_world::TickingWorldScope;

pub(crate) use crate::world::region::regionized_ticking_enabled;

/// Per-island work collected before the parallel phase.
#[derive(Default)]
struct IslandWork {
    /// Indices into the snapshot's block list for random ticks, in snapshot order.
    random_chunks: Vec<usize>,
    /// Chunks a player is close enough to spawn mobs in.
    spawning_chunks: Vec<ChunkPos>,
}

/// Merges each island's dirty chunks and runs the work it deferred, island by island.
fn finish_islands(
    dirty_chunks: &mut FxHashSet<ChunkPos>,
    results: Vec<(FxHashSet<ChunkPos>, Vec<DeferredWork>)>,
) {
    for (island_dirty, deferred) in results {
        dirty_chunks.extend(island_dirty);
        for work in deferred {
            work();
        }
    }
}

/// `(entity id, position)` of every player, before any island moves them.
fn player_snapshot(world: &World) -> Vec<(i32, glam::DVec3)> {
    let mut players = Vec::new();
    world.players.iter_players(|_, player| {
        players.push((player.id(), player.position()));
        true
    });
    players
}

/// Chunks whose entities tick: the random-tick set of the snapshot.
fn entity_ticking_chunks(tickable_chunks: &TickingChunkSnapshot) -> FxHashSet<ChunkPos> {
    tickable_chunks
        .random_chunk_indices
        .iter()
        .map(|&index| tickable_chunks.block[index].pos)
        .collect()
}

/// Splits `ticks` by island, keeping their order inside each island.
///
/// Returns the ticks of each island and the ticks outside every island.
fn split_ticks_by_island<T: TickKey>(
    islands: &Islands,
    ticks: Vec<ScheduledTick<T>>,
) -> (Vec<Vec<ScheduledTick<T>>>, Vec<ScheduledTick<T>>) {
    let mut per_island: Vec<Vec<ScheduledTick<T>>> =
        (0..islands.len()).map(|_| Vec::new()).collect();
    let mut outside = Vec::new();
    for tick in ticks {
        match islands.island_of(ChunkPos::from_block_pos(tick.pos)) {
            Some(island) => per_island[island].push(tick),
            None => outside.push(tick),
        }
    }
    (per_island, outside)
}

impl ChunkMap {
    /// Runs `run` once per island on the tick pool, each with its island bound.
    ///
    /// Returns, island by island, what `run` returned and the work the island
    /// deferred, plus each island's time.
    fn run_on_islands<W: Send, R: Send>(
        &self,
        world: &Arc<World>,
        queues: &[Arc<RegionQueues>],
        work: Vec<W>,
        run: impl Fn(&RegionQueues, W) -> R + Sync,
    ) -> Vec<(R, Vec<DeferredWork>, Duration)> {
        install_on_tick_pool(|| {
            work.into_par_iter()
                .enumerate()
                .with_max_len(1)
                .map(|(island, work)| {
                    let island_start = Instant::now();
                    let queues = &queues[island];
                    let _region = RegionScope::enter(world, Arc::clone(queues));
                    let _ticking = TickingWorldScope::enter(world);
                    let lookup_cache = GameplayChunkLookupCacheScope::enter(self);
                    let result = run(queues, work);
                    let _ = lookup_cache.finish();
                    (result, queues.take_deferred(), island_start.elapsed())
                })
                .collect()
        })
    }

    /// Vanilla `LevelTicks.tick` for block ticks, then fluid ticks, island by island.
    ///
    /// The world selects each batch in Vanilla order with Vanilla's cap; islands
    /// then run their share in that order. Block events they queue wait in the
    /// world queue for the second island phase.
    pub(super) fn tick_scheduled_ticks_in_islands(
        &self,
        world: &Arc<World>,
        tickable_chunks: &TickingChunkSnapshot,
        current_tick: i64,
    ) {
        let _span = tracing::trace_span!("scheduled_ticks_in_islands").entered();
        let islands = Islands::build(tickable_chunks.block.iter().map(|chunk| chunk.pos));
        let players = player_snapshot(world);
        let queues: Vec<Arc<RegionQueues>> = (0..islands.len())
            .map(|island| islands.queues(island, &players))
            .collect();

        let ready_block_ticks =
            Self::collect_scheduled_block_ticks(world, tickable_chunks, current_tick);
        let (per_island, outside) = split_ticks_by_island(&islands, ready_block_ticks);
        let results = self.run_on_islands(world, &queues, per_island, |queues, ticks| {
            queues.set_handling_tick(true);
            Self::execute_scheduled_block_ticks(world, ticks);
        });
        Self::execute_scheduled_block_ticks(world, outside);
        Self::finish_scheduled_islands(results);

        let ready_fluid_ticks =
            Self::collect_scheduled_fluid_ticks(world, tickable_chunks, current_tick);
        let (per_island, outside) = split_ticks_by_island(&islands, ready_fluid_ticks);
        let results = self.run_on_islands(world, &queues, per_island, |queues, ticks| {
            Self::execute_scheduled_fluid_ticks(world, ticks);
            queues.set_handling_tick(false);
        });
        Self::execute_scheduled_fluid_ticks(world, outside);
        Self::finish_scheduled_islands(results);

        for queues in &queues {
            world.requeue_island_block_events(queues);
        }
    }

    fn finish_scheduled_islands(results: Vec<((), Vec<DeferredWork>, Duration)>) {
        for ((), deferred, _) in results {
            for work in deferred {
                work();
            }
        }
    }

    /// Runs spawning, random ticks, block events and entities island by island.
    ///
    /// Marks the chunks entity ticks dirtied, as the sequential entity phase does.
    #[expect(
        clippy::too_many_lines,
        reason = "the island phase keeps Vanilla's per-island step order in one place"
    )]
    pub(super) fn tick_islands(
        &self,
        world: &Arc<World>,
        tickable_chunks: &TickingChunkSnapshot,
        random_tick_speed: u32,
        runs_normally: bool,
    ) -> IslandPhaseStats {
        let _span = tracing::trace_span!("tick_islands").entered();
        let islands = Islands::build(tickable_chunks.block.iter().map(|chunk| chunk.pos));

        let players = player_snapshot(world);
        let island_queues: Vec<Arc<RegionQueues>> = (0..islands.len())
            .map(|island| islands.queues(island, &players))
            .collect();

        // The world's queue holds events queued before this phase: by packets and
        // commands, and by scheduled ticks. Each goes to the island owning its
        // position, ahead of the island's own events, as in Vanilla's single queue.
        // The rest run now.
        if runs_normally {
            world.route_block_events_to_islands(&island_queues, |chunk| islands.island_of(chunk));
            world.run_block_events();
        }

        let mut work: Vec<IslandWork> = (0..islands.len()).map(|_| IslandWork::default()).collect();
        let mut holders = FxHashMap::default();
        if runs_normally {
            let players = world.spawning_player_positions();
            for tickable_chunk in &tickable_chunks.block {
                let Some(island) = islands.island_of(tickable_chunk.pos) else {
                    continue;
                };
                if tickable_chunk.holder.try_full_chunk().is_some()
                    && World::any_player_close_enough_for_spawning(&players, tickable_chunk.pos)
                {
                    work[island].spawning_chunks.push(tickable_chunk.pos);
                    holders.insert(tickable_chunk.pos, &tickable_chunk.holder);
                }
            }
            for &index in &tickable_chunks.random_chunk_indices {
                if let Some(island) = islands.island_of(tickable_chunks.block[index].pos) {
                    work[island].random_chunks.push(index);
                }
            }
        }
        let entity_ticking = entity_ticking_chunks(tickable_chunks);
        let spawning = runs_normally
            .then(|| world.prepare_spawning(self.scheduling.natural_spawn_chunk_count()))
            .flatten();

        let entity_manager = world.entity_manager();
        let (island_entities, remainder) = entity_manager
            .partition_ticking_entities(islands.len(), |chunk| islands.island_of(chunk));

        let island_count = islands.len();
        let parallel_start = Instant::now();
        let results: Vec<(FxHashSet<ChunkPos>, Vec<DeferredWork>, Duration)> =
            install_on_tick_pool(|| {
                work.into_par_iter()
                    .zip(island_entities)
                    .enumerate()
                    .with_max_len(1)
                    .map(|(island, (work, entities))| {
                        let island_start = Instant::now();
                        let queues = Arc::clone(&island_queues[island]);
                        let _region = RegionScope::enter(world, Arc::clone(&queues));
                        let _ticking = TickingWorldScope::enter(world);
                        let lookup_cache = GameplayChunkLookupCacheScope::enter(self);

                        queues.set_handling_tick(true);
                        if runs_normally {
                            let mut spawning_chunks = work.spawning_chunks;
                            let mut island_spawning = spawning.clone();
                            world.run_spawning_chunks(
                                &mut spawning_chunks,
                                island_spawning.as_mut(),
                                |pos| {
                                    if let Some(chunk) =
                                        holders.get(&pos).and_then(|holder| holder.try_full_chunk())
                                    {
                                        chunk.common().increment_inhabited_time();
                                    }
                                    if entity_ticking.contains(&pos) {
                                        world.tick_thunder(pos);
                                    }
                                },
                            );

                            if random_tick_speed > 0 {
                                // As in the sequential pass, positions come from Vanilla's
                                // coordinate LCG seeded per tick from runtime RNG.
                                let mut random_positions =
                                    BlockRandomPositionGenerator::from_runtime_rng();
                                for index in work.random_chunks {
                                    let tickable_chunk = &tickable_chunks.block[index];
                                    if tickable_chunk.randomly_ticking_sections.is_empty() {
                                        continue;
                                    }
                                    if let Some(chunk) = tickable_chunk.holder.try_full_chunk() {
                                        chunk.tick_random_blocks(
                                            world,
                                            random_tick_speed,
                                            &mut random_positions,
                                        );
                                    }
                                }
                            }

                            world.run_block_events();
                        }
                        // Vanilla clears this before ticking entities and block entities.
                        queues.set_handling_tick(false);

                        let dirty = entity_manager.tick_entity_list(entities, runs_normally);
                        let _ = lookup_cache.finish();
                        world.requeue_island_block_events(&queues);
                        (dirty, queues.take_deferred(), island_start.elapsed())
                    })
                    .collect()
            });
        let mut stats = IslandPhaseStats {
            count: island_count,
            parallel: parallel_start.elapsed(),
            ..IslandPhaseStats::default()
        };
        let results = results
            .into_iter()
            .map(|(dirty, deferred, elapsed)| {
                stats.slowest = stats.slowest.max(elapsed);
                stats.busy += elapsed;
                (dirty, deferred)
            })
            .collect();

        let mut dirty_chunks = entity_manager.tick_entity_list(remainder, runs_normally);
        finish_islands(&mut dirty_chunks, results);
        tracing::trace!(island_count, "Ticked islands");
        for chunk in dirty_chunks {
            world.mark_chunk_dirty(chunk);
        }
        stats
    }
}

/// How one island phase went, for slow-tick reports.
#[derive(Clone, Copy, Debug, Default)]
pub struct IslandPhaseStats {
    /// Number of islands.
    pub count: usize,
    /// Wall time of the parallel part.
    pub parallel: Duration,
    /// Time of the slowest island.
    pub slowest: Duration,
    /// Sum of all island times (CPU time spent in islands).
    pub busy: Duration,
}
