//! Vanilla `MinecraftServer.saveEverything` and its periodic autosave.
//!
//! A save captures players and worlds while no world is ticking, then writes
//! them in the background, so the tick never waits for the disk. The periodic
//! autosave spreads its chunk capture over several ticks, like Paper's
//! incremental autosave, so a world full of changed chunks does not stall one
//! tick for the whole capture.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::oneshot;
use uuid::Uuid;

use super::Server;
use super::tick_rate_manager::TickRateManager;
use crate::chunk::chunk_holder::ChunkHolder;
use crate::chunk::chunk_map::AutosaveChunk;
use crate::player::player_data::PersistentPlayerData;
use crate::world::World;
use crate::world::autosave::WorldSave;

/// Vanilla's intended autosave period, in seconds.
const AUTOSAVE_INTERVAL_SECONDS: f32 = 300.0;
/// Vanilla's shortest autosave period, in ticks.
const MIN_AUTOSAVE_INTERVAL_TICKS: i32 = 100;

/// Time per tick an autosave may spend capturing chunks.
const AUTOSAVE_CAPTURE_BUDGET: Duration = Duration::from_millis(10);
/// Chunk holders captured per parallel batch.
const AUTOSAVE_CAPTURE_BATCH: usize = 256;

/// An autosave whose chunks are still being captured, a batch per tick.
pub(crate) struct PendingAutosave {
    worlds: Vec<PendingWorldAutosave>,
}

struct PendingWorldAutosave {
    world: Arc<World>,
    remaining: Vec<Arc<ChunkHolder>>,
    captured: Vec<AutosaveChunk>,
}

impl PendingAutosave {
    /// Starts an autosave of every world `/save-off` has not turned off.
    pub(super) fn begin(server: &Server) -> Self {
        let worlds = server
            .worlds
            .values()
            .filter(|world| !world.no_save())
            .map(|world| PendingWorldAutosave {
                world: Arc::clone(world),
                remaining: world.autosave_chunk_holders(),
                captured: Vec::new(),
            })
            .collect();
        Self { worlds }
    }

    /// Captures chunks until `budget` is spent. Returns whether every chunk
    /// has been captured. Must run while no world is ticking.
    pub(super) fn capture_some(&mut self, budget: Duration) -> bool {
        let start = Instant::now();
        for world in &mut self.worlds {
            while !world.remaining.is_empty() {
                if start.elapsed() >= budget {
                    return false;
                }
                let batch_start = world.remaining.len().saturating_sub(AUTOSAVE_CAPTURE_BATCH);
                let batch = world.remaining.split_off(batch_start);
                world
                    .captured
                    .extend(world.world.capture_autosave_chunks(batch));
            }
        }
        true
    }

    /// Drops the autosave, marking every captured chunk unsaved again.
    pub(super) fn discard(self) {
        for world in self.worlds {
            for chunk in world.captured {
                chunk.discard();
            }
        }
    }
}

/// Everything one save captured, from [`Server::prepare_save`].
pub(crate) struct ServerSave {
    players: Vec<(String, Uuid, PersistentPlayerData)>,
    worlds: Vec<(Arc<World>, WorldSave)>,
}

/// What a [`ServerSave`] wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ServerSaveResult {
    /// Players written.
    pub players: usize,
    /// Chunks written.
    pub chunks: usize,
    /// Whether every part was written.
    pub success: bool,
}

impl Server {
    /// The capture half of vanilla `saveEverything`: every online player
    /// (vanilla `PlayerList.saveAll`, which `/save-off` does not stop) and
    /// every world, skipping worlds `/save-off` turned off unless `force`.
    ///
    /// Must run while no world is ticking, like vanilla's main-thread save.
    pub(crate) fn prepare_save(&self, force: bool) -> ServerSave {
        // This save captures every changed chunk now, including those an
        // autosave in progress already captured, so that autosave is dropped
        // instead of landing its older captures after this one.
        self.discard_pending_autosave();
        let worlds = self
            .worlds
            .values()
            .filter_map(|world| {
                world
                    .prepare_save(force)
                    .map(|save| (Arc::clone(world), save))
            })
            .collect();
        ServerSave {
            players: self.capture_players(),
            worlds,
        }
    }

    /// Captures the next batch of a periodic autosave, starting one when
    /// `start` is set and none is in progress. Returns the save once its last
    /// chunk is captured, together with players and world data captured now.
    ///
    /// Must run while no world is ticking.
    pub(super) fn capture_autosave_step(&self, start: bool) -> Option<ServerSave> {
        let mut pending = self.pending_autosave.lock().take();
        if pending.is_none() && start {
            pending = Some(PendingAutosave::begin(self));
        }
        let mut pending = pending?;
        if !pending.capture_some(AUTOSAVE_CAPTURE_BUDGET) {
            *self.pending_autosave.lock() = Some(pending);
            return None;
        }
        let mut worlds = Vec::with_capacity(pending.worlds.len());
        for PendingWorldAutosave {
            world, captured, ..
        } in pending.worlds
        {
            match world.prepare_save_without_chunks(false) {
                Some(mut save) => {
                    save.set_chunks(captured);
                    worlds.push((world, save));
                }
                // `/save-off` ran while the capture was in progress.
                None => captured.into_iter().for_each(AutosaveChunk::discard),
            }
        }
        Some(ServerSave {
            players: self.capture_players(),
            worlds,
        })
    }

    /// Whether a periodic autosave is still capturing chunks.
    pub(super) fn is_autosave_capturing(&self) -> bool {
        self.pending_autosave.lock().is_some()
    }

    /// Drops an autosave still capturing chunks, marking what it captured
    /// unsaved again so the next save writes it.
    pub(super) fn discard_pending_autosave(&self) {
        let pending = self.pending_autosave.lock().take();
        if let Some(pending) = pending {
            pending.discard();
        }
    }

    fn capture_players(&self) -> Vec<(String, Uuid, PersistentPlayerData)> {
        self.get_players()
            .iter()
            .map(|player| {
                (
                    player.get_world().domain().to_owned(),
                    player.gameprofile.id(),
                    PersistentPlayerData::from_player(player),
                )
            })
            .collect()
    }

    /// The write half of vanilla `saveEverything`, together with the
    /// server-wide saved data vanilla `saveAllChunks` stores: scoreboards,
    /// command storage, `/bossbar` bars and maps.
    pub(crate) async fn write_save(&self, save: ServerSave) -> ServerSaveResult {
        let mut result = ServerSaveResult {
            success: true,
            ..ServerSaveResult::default()
        };
        for (domain, uuid, data) in save.players {
            match self
                .player_data_storage
                .save_domain_data(&domain, uuid, &data)
                .await
            {
                Ok(()) => result.players += 1,
                Err(error) => {
                    log::error!("Failed to save player {uuid} in domain {domain}: {error}");
                    result.success = false;
                }
            }
        }
        for (world, world_save) in save.worlds {
            let written = world.write_save(world_save).await;
            result.chunks += written.chunks;
            result.success &= written.success;
        }

        let command_data = self.save_command_data().await;
        if let Err(error) = command_data.scoreboards {
            log::error!("Failed to save domain scoreboards: {error}");
            result.success = false;
        }
        if let Err(error) = command_data.storage {
            log::error!("Failed to save domain command storage: {error}");
            result.success = false;
        }
        if let Err(error) = command_data.boss_events {
            log::error!("Failed to save domain boss bars: {error}");
            result.success = false;
        }
        if let Err(error) = command_data.random_sequences {
            log::error!("Failed to save domain random sequences: {error}");
            result.success = false;
        }
        if let Err(error) = command_data.scheduled_events {
            log::error!("Failed to save domain scheduled events: {error}");
            result.success = false;
        }
        if let Err(error) = command_data.stopwatches {
            log::error!("Failed to save domain stopwatches: {error}");
            result.success = false;
        }
        if let Err(error) = self.worlds.save_maps() {
            log::error!("Failed to save maps: {error}");
            result.success = false;
        }
        result
    }

    /// Writes a captured save in the background, then hands its result to
    /// `on_written`. Writes run one after another in capture order, so an
    /// older capture never lands over a newer one; shutdown waits for them
    /// before its own final save.
    pub(crate) fn spawn_save_write(
        self: &Arc<Self>,
        save: ServerSave,
        on_written: impl FnOnce(ServerSaveResult) + Send + 'static,
    ) {
        let (written, next) = oneshot::channel::<()>();
        let previous = self.last_save_write.lock().replace(next);
        let server = Arc::clone(self);
        self.save_writes.spawn(async move {
            if let Some(previous) = previous {
                // Resolves once the previous write finished, sent or dropped.
                let _ = previous.await;
            }
            on_written(server.write_save(save).await);
            // Vanilla `saveEverything` ends with `warnOnLowDiskSpace`.
            server.warn_on_low_disk_space();
            drop(written);
        });
    }

    /// Whether an autosave or `/save-all` is still writing.
    pub(crate) fn is_save_writing(&self) -> bool {
        !self.save_writes.is_empty()
    }

    /// Waits for every save still writing. New writes are refused afterwards.
    pub(super) async fn finish_save_writes(&self) {
        self.save_writes.close();
        self.save_writes.wait().await;
    }

    /// Vanilla `MinecraftServer.setAutoSave`: turns saving of every world on
    /// or off, returning whether any world changed.
    pub fn set_auto_save(&self, enable: bool) -> bool {
        let mut changed = false;
        for world in self.worlds.values() {
            changed |= world.set_no_save(!enable);
        }
        changed
    }
}

/// Vanilla's autosave countdown: `ticksUntilAutosave`,
/// `computeNextAutosaveInterval` and `onTickRateChanged`.
pub(super) struct AutosaveSchedule {
    ticks_until_autosave: i32,
    tick_rate: f32,
    sprinting: bool,
}

impl AutosaveSchedule {
    /// Vanilla starts counting down from 6000 ticks.
    pub(super) const fn new() -> Self {
        Self {
            ticks_until_autosave: 6000,
            tick_rate: 20.0,
            sprinting: false,
        }
    }

    /// Counts one server tick down and reports whether an autosave is due,
    /// restarting the countdown when it is.
    pub(super) fn tick(&mut self, tick_rate_manager: &TickRateManager) -> bool {
        self.track_tick_rate(tick_rate_manager);
        self.ticks_until_autosave -= 1;
        if self.ticks_until_autosave > 0 {
            return false;
        }
        self.ticks_until_autosave = next_autosave_interval(tick_rate_manager);
        true
    }

    /// Vanilla calls `onTickRateChanged` when the tick rate is set and when a
    /// sprint finishes: a shorter interval cuts the current countdown.
    fn track_tick_rate(&mut self, tick_rate_manager: &TickRateManager) {
        let sprinting = tick_rate_manager.is_sprinting();
        #[expect(
            clippy::float_cmp,
            reason = "any change of the configured tick rate counts, like vanilla's setter call"
        )]
        let rate_changed = tick_rate_manager.tick_rate() != self.tick_rate;
        let sprint_finished = self.sprinting && !sprinting;
        self.tick_rate = tick_rate_manager.tick_rate();
        self.sprinting = sprinting;
        if rate_changed || sprint_finished {
            self.ticks_until_autosave = self
                .ticks_until_autosave
                .min(next_autosave_interval(tick_rate_manager));
        }
    }
}

/// Vanilla `computeNextAutosaveInterval`: five minutes at the current tick
/// rate, or at the measured speed while sprinting.
fn next_autosave_interval(tick_rate_manager: &TickRateManager) -> i32 {
    let ticks_per_second = if tick_rate_manager.is_sprinting() {
        let estimated_tick_time_nanos = tick_rate_manager.get_average_tick_time_nanos() + 1;
        1_000_000_000.0 / estimated_tick_time_nanos as f32
    } else {
        tick_rate_manager.tick_rate()
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "vanilla casts the float interval to int"
    )]
    let interval = (ticks_per_second * AUTOSAVE_INTERVAL_SECONDS) as i32;
    interval.max(MIN_AUTOSAVE_INTERVAL_TICKS)
}

#[cfg(test)]
mod tests {
    use super::{AutosaveSchedule, next_autosave_interval};
    use crate::server::tick_rate_manager::TickRateManager;

    #[test]
    fn autosave_runs_every_five_minutes_of_ticks() {
        let manager = TickRateManager::new();
        assert_eq!(next_autosave_interval(&manager), 6000);
        let mut schedule = AutosaveSchedule::new();
        for _ in 0..5999 {
            assert!(!schedule.tick(&manager));
        }
        assert!(schedule.tick(&manager));
        for _ in 0..5999 {
            assert!(!schedule.tick(&manager));
        }
        assert!(schedule.tick(&manager));
    }

    #[test]
    fn faster_tick_rate_cuts_the_countdown() {
        let mut manager = TickRateManager::new();
        let mut schedule = AutosaveSchedule::new();
        assert!(!schedule.tick(&manager));
        manager.set_tick_rate(1.0);
        // 1 tick per second gives 300 ticks; the countdown drops to it.
        for _ in 0..299 {
            assert!(!schedule.tick(&manager));
        }
        assert!(schedule.tick(&manager));
        assert_eq!(next_autosave_interval(&manager), 300);
    }
}
