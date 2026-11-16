use super::{
    Arc, BlockPos, ChunkHolder, ChunkMap, ChunkPos, ChunkStatus, InFlightLightUpdates,
    LightCacheLayout, LightCacheSetupRadius, LightLayer, LightSectionEmptinessChange,
    LightSectionRange, LightUpdateState, LightWorkset, PendingChunkLightUpdates, SectionPos,
    propagate_block_light_changes_with_empty_sections,
    propagate_sky_light_changes_with_empty_sections,
};
use rayon::iter::{IntoParallelIterator as _, ParallelIterator as _};

use crate::chunk::light::LightWorkWindowGate;
use crate::world::region::regionized_ticking_enabled;
use crate::world::tick_pool::install_on_tick_pool;

/// Light sections a queued light change updated: sky first, then block.
type UpdatedLightSections = (Vec<SectionPos>, Vec<SectionPos>);

/// Queued light tasks keyed by their light window center.
type LightTasks<T> = Vec<(ChunkPos, T)>;

/// Splits queued light tasks into the next round to run and the rest.
///
/// A round holds tasks whose light cache windows are pairwise disjoint, so they can
/// propagate in parallel. A task that overlaps a task already in the round, or one
/// already deferred, is deferred too: two overlapping tasks keep their queue order.
pub(super) fn split_light_round<T>(tasks: LightTasks<T>) -> (LightTasks<T>, LightTasks<T>) {
    let mut round: LightTasks<T> = Vec::new();
    let mut deferred: LightTasks<T> = Vec::new();
    for (center, task) in tasks {
        let waits = round
            .iter()
            .chain(deferred.iter())
            .any(|&(other, _)| LightWorkWindowGate::windows_overlap(center, other));
        if waits {
            deferred.push((center, task));
        } else {
            round.push((center, task));
        }
    }
    (round, deferred)
}

impl ChunkMap {
    /// Records a block change at the given position.
    /// This marks the chunk as having pending changes to broadcast.
    pub fn block_changed(&self, pos: BlockPos) {
        let chunk_pos = ChunkPos::new(
            SectionPos::block_to_section_coord(pos.0.x),
            SectionPos::block_to_section_coord(pos.0.z),
        );

        if let Some(holder) = self.lookup_active_holder(chunk_pos)
            && holder.block_changed(pos)
        {
            // First change for this chunk - add to broadcast list
            self.chunks_to_broadcast.lock().push(holder);
        }
    }

    /// Marks client-visible chunk packet content as changed.
    pub fn packet_content_changed(&self, chunk_pos: ChunkPos) {
        if let Some(holder) = self.lookup_active_holder(chunk_pos) {
            holder.mark_packet_content_changed();
        }
    }

    /// Records a light-section change at the given position.
    pub fn light_changed(&self, layer: LightLayer, section_pos: SectionPos) {
        let chunk_pos = ChunkPos::new(section_pos.x(), section_pos.z());

        if let Some(holder) = self.lookup_active_holder(chunk_pos) {
            if holder.light_changed(layer, section_pos) {
                self.chunks_to_broadcast.lock().push(holder);
            }
            return;
        }

        if let Some(holder) = self
            .unloading_chunks
            .read_sync(&chunk_pos, |_, h| Arc::clone(h))
        {
            holder.mark_light_section_dirty(section_pos);
        }
    }

    /// Queues a block or section light change for the next light propagation drain.
    pub fn queue_light_change(
        &self,
        pos: BlockPos,
        check_block: bool,
        empty_section_change: Option<LightSectionEmptinessChange>,
    ) {
        if !check_block && empty_section_change.is_none() {
            return;
        }

        let chunk_pos = ChunkPos::new(
            SectionPos::block_to_section_coord(pos.0.x),
            SectionPos::block_to_section_coord(pos.0.z),
        );

        let mut light_updates = self.light_updates.lock();
        if !self.light_update_center_is_available(chunk_pos) {
            return;
        }

        light_updates
            .pending
            .queue_change(chunk_pos, pos, check_block, empty_section_change);
    }

    /// Drains all queued light updates and runs one scoped propagation per changed chunk.
    ///
    /// With regionized ticking, tasks whose light windows do not overlap propagate in
    /// parallel on the tick pool, round by round. Their changed sections are reported in
    /// queue order once each round is done, so broadcasts come out as they did sequentially.
    pub fn propagate_queued_light_changes(&self) {
        let Some((tasks, in_flight_updates)) = self.drain_pending_light_updates() else {
            return;
        };

        let mut blocked_tasks = Vec::new();
        if regionized_ticking_enabled() {
            let mut remaining = tasks
                .into_iter()
                .filter(|(_, task)| !task.is_empty())
                .collect::<Vec<_>>();
            while !remaining.is_empty() {
                let (round, deferred) = split_light_round(remaining);
                remaining = deferred;
                let mut reserved = Vec::with_capacity(round.len());
                for (center, task) in round {
                    match self.light_work_window_gate.try_reserve_centered(center) {
                        Some(reservation) => reserved.push((center, task, reservation)),
                        None => blocked_tasks.push((center, task)),
                    }
                }
                let results = if reserved.len() > 1 {
                    install_on_tick_pool(|| {
                        reserved
                            .into_par_iter()
                            .map(|(center, task, reservation)| {
                                let updated = self.compute_queued_light_change(center, task);
                                drop(reservation);
                                updated
                            })
                            .collect::<Vec<_>>()
                    })
                } else {
                    reserved
                        .into_iter()
                        .map(|(center, task, reservation)| {
                            let updated = self.compute_queued_light_change(center, task);
                            drop(reservation);
                            updated
                        })
                        .collect()
                };
                for updated in results.into_iter().flatten() {
                    self.report_light_changes(updated);
                }
            }
        } else {
            for (center, task) in tasks {
                if task.is_empty() {
                    continue;
                }
                let Some(light_work_window_reservation) =
                    self.light_work_window_gate.try_reserve_centered(center)
                else {
                    blocked_tasks.push((center, task));
                    continue;
                };

                self.propagate_queued_light_change(center, task);
                drop(light_work_window_reservation);
            }
        }

        if !blocked_tasks.is_empty() {
            self.light_updates
                .lock()
                .pending
                .prepend_drained(blocked_tasks);
        }
        drop(in_flight_updates);
    }

    pub(super) async fn flush_queued_light_changes_for_save(&self) {
        loop {
            let Some(center) = self.next_pending_light_update_center() else {
                if !self.has_in_flight_light_updates() {
                    return;
                }
                self.wait_for_in_flight_light_updates().await;
                continue;
            };

            let light_work_window_reservation =
                self.light_work_window_gate.reserve_centered(center).await;

            let Some((task, in_flight_updates)) =
                self.drain_pending_light_update_for_center(center)
            else {
                drop(light_work_window_reservation);
                continue;
            };

            if task.is_empty() {
                drop(light_work_window_reservation);
                drop(in_flight_updates);
                continue;
            }

            self.propagate_queued_light_change(center, task);
            drop(light_work_window_reservation);
            drop(in_flight_updates);
        }
    }

    pub(super) fn drain_pending_light_updates(
        &self,
    ) -> Option<(
        Vec<(ChunkPos, PendingChunkLightUpdates)>,
        InFlightLightUpdates<'_>,
    )> {
        let mut light_updates = self.light_updates.lock();
        if light_updates.pending.is_empty() {
            return None;
        }
        let tasks = light_updates.pending.drain();
        let centers = tasks
            .iter()
            .map(|(chunk_pos, _)| *chunk_pos)
            .collect::<Vec<_>>();
        let in_flight = self.track_in_flight_light_updates(&mut light_updates, centers);
        Some((tasks, in_flight))
    }

    pub(super) fn next_pending_light_update_center(&self) -> Option<ChunkPos> {
        self.light_updates.lock().pending.next_center()
    }

    pub(super) fn next_pending_light_update_center_touching_chunk(
        &self,
        chunk_pos: ChunkPos,
    ) -> Option<ChunkPos> {
        self.light_updates
            .lock()
            .pending
            .next_center_touching_chunk(chunk_pos)
    }

    pub(super) fn drain_pending_light_update_for_center(
        &self,
        center: ChunkPos,
    ) -> Option<(PendingChunkLightUpdates, InFlightLightUpdates<'_>)> {
        let mut light_updates = self.light_updates.lock();
        let task = light_updates.pending.drain_center(center)?;
        let in_flight = self.track_in_flight_light_updates(&mut light_updates, vec![center]);
        Some((task, in_flight))
    }

    pub(super) fn track_in_flight_light_updates(
        &self,
        light_updates: &mut LightUpdateState,
        centers: Vec<ChunkPos>,
    ) -> InFlightLightUpdates<'_> {
        light_updates.track_in_flight(&centers);
        InFlightLightUpdates {
            centers,
            light_updates: &self.light_updates,
            progress_notify: &self.light_updates_progress_notify,
        }
    }

    pub(super) fn has_in_flight_light_updates(&self) -> bool {
        self.light_updates.lock().has_in_flight_updates()
    }

    pub(super) fn has_in_flight_light_update_touching_chunk(&self, chunk_pos: ChunkPos) -> bool {
        self.light_updates
            .lock()
            .has_in_flight_update_touching_chunk(chunk_pos)
    }

    pub(super) async fn wait_for_in_flight_light_updates(&self) {
        loop {
            if !self.has_in_flight_light_updates() {
                return;
            }

            let progress = self.light_updates_progress_notify.notified();
            if !self.has_in_flight_light_updates() {
                return;
            }
            progress.await;
        }
    }

    pub(super) async fn wait_for_in_flight_light_update_touching_chunk(&self, chunk_pos: ChunkPos) {
        loop {
            if !self.has_in_flight_light_update_touching_chunk(chunk_pos) {
                return;
            }

            let progress = self.light_updates_progress_notify.notified();
            if !self.has_in_flight_light_update_touching_chunk(chunk_pos) {
                return;
            }
            progress.await;
        }
    }

    pub(super) async fn flush_queued_light_changes_touching_chunk_for_save(
        &self,
        chunk_pos: ChunkPos,
    ) {
        loop {
            let Some(center) = self.next_pending_light_update_center_touching_chunk(chunk_pos)
            else {
                if !self.has_in_flight_light_update_touching_chunk(chunk_pos) {
                    return;
                }
                self.wait_for_in_flight_light_update_touching_chunk(chunk_pos)
                    .await;
                continue;
            };

            let light_work_window_reservation =
                self.light_work_window_gate.reserve_centered(center).await;

            let Some((task, in_flight_updates)) =
                self.drain_pending_light_update_for_center(center)
            else {
                drop(light_work_window_reservation);
                continue;
            };

            if task.is_empty() {
                drop(light_work_window_reservation);
                drop(in_flight_updates);
                continue;
            }

            self.propagate_queued_light_change(center, task);
            drop(light_work_window_reservation);
            drop(in_flight_updates);
        }
    }

    #[cfg(test)]
    pub(super) fn has_pending_light_updates(&self) -> bool {
        !self.light_updates.lock().is_idle()
    }

    #[cfg(test)]
    pub(super) fn light_update_touches_chunk(&self, chunk_pos: ChunkPos) -> bool {
        self.light_updates.lock().touches_chunk(chunk_pos)
    }

    pub(super) fn light_update_center_is_available(&self, center: ChunkPos) -> bool {
        self.light_update_holder(center)
            .is_some_and(|holder| holder.try_chunk(ChunkStatus::Light).is_some())
    }

    pub(super) fn light_update_holder(&self, chunk_pos: ChunkPos) -> Option<Arc<ChunkHolder>> {
        self.chunks
            .read_sync(&chunk_pos, |_, holder| Arc::clone(holder))
            .or_else(|| {
                self.unloading_chunks
                    .read_sync(&chunk_pos, |_, holder| Arc::clone(holder))
            })
    }

    pub(super) fn propagate_queued_light_change(
        &self,
        center: ChunkPos,
        task: PendingChunkLightUpdates,
    ) {
        if let Some(updated) = self.compute_queued_light_change(center, task) {
            self.report_light_changes(updated);
        }
    }

    /// Marks the sections a queued light change updated for broadcast.
    fn report_light_changes(&self, (sky, block): UpdatedLightSections) {
        for section_pos in sky {
            self.light_changed(LightLayer::Sky, section_pos);
        }
        for section_pos in block {
            self.light_changed(LightLayer::Block, section_pos);
        }
    }

    /// Propagates one queued light change inside its light window and returns the
    /// sections it updated, without reporting them.
    ///
    /// The caller must hold the light work reservation for `center`.
    fn compute_queued_light_change(
        &self,
        center: ChunkPos,
        task: PendingChunkLightUpdates,
    ) -> Option<UpdatedLightSections> {
        let Some(workset) = self.light_workset_for_change(center) else {
            log::warn!("Failed to set up light workset for queued light update at {center:?}");
            return None;
        };

        let empty_sections = task.empty_section_changes();
        let positions = task.changed_positions.into_iter().collect::<Vec<_>>();
        let world = self.world_gen_context.world();

        let mut sky = Vec::new();
        if world.dimension_type.has_skylight {
            match propagate_sky_light_changes_with_empty_sections(
                &workset,
                positions.iter().copied(),
                empty_sections.iter().copied(),
            ) {
                Ok(result) => sky = result.updated_sections.into_iter().collect(),
                Err(error) => {
                    log::warn!(
                        "Failed to propagate queued sky-light change for {center:?}: {error:?}"
                    );
                }
            }
        }

        let Ok(result) =
            propagate_block_light_changes_with_empty_sections(&workset, positions, empty_sections)
        else {
            log::warn!("Failed to propagate queued block-light change for {center:?}");
            // The sky sections already changed; report them like the sequential path did.
            return Some((sky, Vec::new()));
        };

        Some((sky, result.updated_sections.into_iter().collect()))
    }

    pub(super) fn light_workset_for_change(&self, center: ChunkPos) -> Option<LightWorkset> {
        let Ok(range) = LightSectionRange::from_world_height(
            self.world_gen_context.min_y(),
            self.world_gen_context.height(),
        ) else {
            return None;
        };

        let layout = LightCacheLayout::new(center, range);
        LightWorkset::setup(
            layout,
            LightCacheSetupRadius::Full,
            true,
            |chunk_pos| {
                let holder = self.light_update_holder(chunk_pos)?;
                holder.try_chunk(ChunkStatus::Light)?;
                Some(holder)
            },
            |_| true,
        )
        .ok()
    }
}
