//! Periodic world saving: vanilla `ServerLevel.save` and `noSave`.
//!
//! A save is split in two. The capture runs while the world is not ticking,
//! so everything is taken at one consistent point, like vanilla's main-thread
//! save. The write then runs in the background without holding any world lock.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use steel_utils::saved_data::names as saved_data_names;

use super::World;
use super::custom_spawner::WanderingTraderData;
use super::raid::SavedRaids;
use crate::chunk::chunk_holder::ChunkHolder;
use crate::chunk::chunk_map::AutosaveChunk;
use crate::chunk::chunk_ticket_storage::PersistentChunkTickets;
use crate::level_data::PreparedLevelData;

/// One world's state captured by [`World::prepare_save`].
pub(crate) struct WorldSave {
    level_data: Option<PreparedLevelData>,
    chunk_tickets: PersistentChunkTickets,
    wandering_trader_data: Option<WanderingTraderData>,
    raids: SavedRaids,
    chunks: Vec<AutosaveChunk>,
}

impl WorldSave {
    /// Sets the chunks this save writes.
    pub(crate) fn set_chunks(&mut self, chunks: Vec<AutosaveChunk>) {
        self.chunks = chunks;
    }
}

/// What a [`WorldSave`] wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WorldSaveResult {
    /// Chunks written.
    pub chunks: usize,
    /// Whether every part was written.
    pub success: bool,
}

impl World {
    /// Vanilla `ServerLevel.noSave`: whether `/save-off` turned saving off.
    #[must_use]
    pub fn no_save(&self) -> bool {
        self.no_save.load(Ordering::Acquire)
    }

    /// Sets vanilla `ServerLevel.noSave`, returning whether it changed.
    pub fn set_no_save(&self, no_save: bool) -> bool {
        self.no_save.swap(no_save, Ordering::AcqRel) != no_save
    }

    /// The capture half of vanilla `ServerLevel.save`: level data, the
    /// world's saved data and every loaded Full chunk with unsaved changes.
    ///
    /// Returns `None` when `/save-off` turned saving off and `force` is not
    /// set, like vanilla's `noSave` check. Must run while this world is not
    /// ticking.
    pub(crate) fn prepare_save(&self, force: bool) -> Option<WorldSave> {
        let mut save = self.prepare_save_without_chunks(force)?;
        save.chunks = self.chunk_map.prepare_autosave();
        Some(save)
    }

    /// Every loaded chunk holder, for a chunk capture split over several
    /// ticks with [`Self::capture_autosave_chunks`].
    pub(crate) fn autosave_chunk_holders(&self) -> Vec<Arc<ChunkHolder>> {
        self.chunk_map.autosave_holders()
    }

    /// Captures the Full chunks among `holders` that have unsaved changes.
    /// Must run while this world is not ticking.
    pub(crate) fn capture_autosave_chunks(
        &self,
        holders: Vec<Arc<ChunkHolder>>,
    ) -> Vec<AutosaveChunk> {
        self.chunk_map.capture_autosave(holders)
    }

    /// [`Self::prepare_save`] without the chunks, for a save whose chunks
    /// were captured over several ticks.
    pub(crate) fn prepare_save_without_chunks(&self, force: bool) -> Option<WorldSave> {
        if self.no_save() && !force {
            return None;
        }
        self.sync_world_border_to_level_data();
        let prepared = self.level_data.write().prepare_save();
        let level_data = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                log::error!("Failed to serialize level data of {}: {error}", self.key);
                None
            }
        };
        let wandering_trader_data = {
            let mut data = self.wandering_trader_data.lock();
            data.is_dirty().then(|| {
                let snapshot = data.clone();
                data.mark_saved();
                snapshot
            })
        };
        Some(WorldSave {
            level_data,
            chunk_tickets: self.chunk_map.persistent_chunk_tickets(),
            wandering_trader_data,
            raids: self.raids.to_saved(),
            chunks: Vec::new(),
        })
    }

    /// The write half of vanilla `ServerLevel.save`. Parts that fail to write
    /// are marked unsaved again, so the next save retries them.
    pub(crate) async fn write_save(&self, save: WorldSave) -> WorldSaveResult {
        let WorldSave {
            level_data,
            chunk_tickets,
            wandering_trader_data,
            raids,
            chunks,
        } = save;
        let mut success = true;

        if let Some(level_data) = level_data
            && let Err(error) = level_data.write().await
        {
            log::error!("Failed to save level data of {}: {error}", self.key);
            self.level_data.write().mark_dirty();
            success = false;
        }
        if let Err(error) = self
            .saved_data
            .save(saved_data_names::CHUNK_TICKETS, &chunk_tickets)
            .await
        {
            log::error!("Failed to save chunk tickets of {}: {error}", self.key);
            success = false;
        }
        if let Some(data) = wandering_trader_data
            && let Err(error) = self
                .saved_data
                .save(saved_data_names::WANDERING_TRADER, &data)
                .await
        {
            log::error!(
                "Failed to save wandering trader data of {}: {error}",
                self.key
            );
            self.wandering_trader_data.lock().mark_dirty();
            success = false;
        }
        if let Err(error) = self.saved_data.save(saved_data_names::RAIDS, &raids).await {
            log::error!("Failed to save raids of {}: {error}", self.key);
            success = false;
        }
        let chunks = match self.chunk_map.write_autosave(chunks).await {
            Ok(saved) => saved,
            Err(error) => {
                log::error!("Failed to flush region files of {}: {error}", self.key);
                success = false;
                0
            }
        };
        WorldSaveResult { chunks, success }
    }
}
