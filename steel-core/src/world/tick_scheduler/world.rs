use super::super::*;
use steel_utils::BoundingBox;

impl World {
    /// Schedules a block tick at the given position.
    ///
    /// The tick will fire after `delay` world game ticks with the given priority.
    /// Its deadline continues to age while the loaded chunk is outside simulation distance.
    /// Only one tick per `(pos, block)` pair can be active at a time — duplicates
    /// are silently ignored.
    pub fn schedule_block_tick(
        &self,
        pos: BlockPos,
        block: BlockRef,
        delay: i32,
        priority: super::TickPriority,
    ) {
        let trigger_tick = self.game_time().wrapping_add(i64::from(delay));
        let order = self.scheduled_ticks.next_sub_tick_order();
        let chunk_pos = Self::chunk_pos_for_block(pos);
        self.chunk_map.with_full_chunk(chunk_pos, |chunk| {
            chunk.schedule_block_tick(pos, block, trigger_tick, priority, order);
        });
    }

    /// Schedules a block tick with `Normal` priority.
    pub fn schedule_block_tick_default(&self, pos: BlockPos, block: BlockRef, delay: i32) {
        self.schedule_block_tick(pos, block, delay, super::TickPriority::Normal);
    }

    /// Schedules a fluid tick at the given position.
    ///
    /// The tick will fire after `delay` world game ticks with the given priority.
    /// Its deadline continues to age while the loaded chunk is outside simulation distance.
    /// Only one tick per `(pos, fluid)` pair can be active at a time.
    pub fn schedule_fluid_tick(
        &self,
        pos: BlockPos,
        fluid: FluidRef,
        delay: i32,
        priority: super::TickPriority,
    ) {
        let trigger_tick = self.game_time().wrapping_add(i64::from(delay));
        let order = self.scheduled_ticks.next_sub_tick_order();
        let chunk_pos = Self::chunk_pos_for_block(pos);
        self.chunk_map.with_full_chunk(chunk_pos, |chunk| {
            chunk.schedule_fluid_tick(pos, fluid, trigger_tick, priority, order);
        });
    }

    /// Schedules a fluid tick with `Normal` priority.
    pub fn schedule_fluid_tick_default(&self, pos: BlockPos, fluid: FluidRef, delay: i32) {
        self.schedule_fluid_tick(pos, fluid, delay, super::TickPriority::Normal);
    }

    /// Returns `true` if a block tick is already scheduled for the given `(pos, block)`.
    ///
    /// # Panics
    ///
    /// Panics if a published Full chunk's scheduled-tick container was finalized,
    /// which violates the chunk publication invariant.
    pub fn has_scheduled_block_tick(&self, pos: BlockPos, block: BlockRef) -> bool {
        let chunk_pos = Self::chunk_pos_for_block(pos);
        self.chunk_map
            .with_full_chunk(chunk_pos, |chunk| {
                chunk
                    .has_scheduled_block_tick(pos, block)
                    .expect("Full chunk scheduled-tick ownership invariant failed")
            })
            .unwrap_or(false)
    }

    /// Returns `true` if a fluid tick is already scheduled for the given `(pos, fluid)`.
    ///
    /// # Panics
    ///
    /// Panics if a published Full chunk's scheduled-tick container was finalized,
    /// which violates the chunk publication invariant.
    pub fn has_scheduled_fluid_tick(&self, pos: BlockPos, fluid: FluidRef) -> bool {
        let chunk_pos = Self::chunk_pos_for_block(pos);
        self.chunk_map
            .with_full_chunk(chunk_pos, |chunk| {
                chunk
                    .has_scheduled_fluid_tick(pos, fluid)
                    .expect("Full chunk scheduled-tick ownership invariant failed")
            })
            .unwrap_or(false)
    }

    pub(crate) fn register_full_chunk_ticks(
        &self,
        chunk: FullChunkRef<'_>,
    ) -> Result<(), super::TickSchedulerError> {
        self.scheduled_ticks.register_chunk(chunk)
    }

    pub(crate) fn unregister_full_chunk_ticks(&self, pos: ChunkPos) {
        self.scheduled_ticks.unregister_chunk(pos);
    }

    #[cfg(test)]
    pub(crate) fn has_registered_full_chunk_ticks(&self, pos: ChunkPos) -> bool {
        self.scheduled_ticks.has_registered_chunk(pos)
    }

    #[cfg(test)]
    pub(crate) fn has_indexed_scheduled_tick_head(&self, pos: ChunkPos) -> bool {
        self.scheduled_ticks.has_indexed_head(pos)
    }

    pub(crate) fn schedule_block_tick_for_chunk(
        &self,
        chunk: FullChunkRef<'_>,
        pos: BlockPos,
        block: BlockRef,
        trigger_tick: i64,
        priority: super::TickPriority,
        sub_tick_order: i64,
    ) -> Result<bool, super::TickSchedulerError> {
        self.scheduled_ticks.schedule_block(
            chunk,
            block,
            pos,
            trigger_tick,
            priority,
            sub_tick_order,
        )
    }

    pub(crate) fn schedule_fluid_tick_for_chunk(
        &self,
        chunk: FullChunkRef<'_>,
        pos: BlockPos,
        fluid: FluidRef,
        trigger_tick: i64,
        priority: super::TickPriority,
        sub_tick_order: i64,
    ) -> Result<bool, super::TickSchedulerError> {
        self.scheduled_ticks.schedule_fluid(
            chunk,
            fluid,
            pos,
            trigger_tick,
            priority,
            sub_tick_order,
        )
    }

    pub(crate) fn unpack_scheduled_ticks(
        &self,
        pos: ChunkPos,
    ) -> Result<(), super::TickSchedulerError> {
        self.scheduled_ticks.unpack_chunk(pos, self.game_time())
    }

    pub(crate) fn reconcile_active_scheduled_tick_chunks<I>(
        &self,
        active_chunks: I,
    ) -> Result<(), super::TickSchedulerError>
    where
        I: Iterator<Item = ChunkPos> + Clone,
    {
        self.scheduled_ticks.reconcile_active_chunks(active_chunks)
    }

    pub(crate) fn begin_scheduled_tick_phase(
        &self,
        current_tick: i64,
        max_ticks: usize,
    ) -> super::ScheduledTickBatch<BlockRef> {
        self.scheduled_ticks.begin_tick(current_tick, max_ticks)
    }

    pub(crate) fn collect_scheduled_fluid_tick_batch(
        &self,
        current_tick: i64,
        max_ticks: usize,
    ) -> super::ScheduledTickBatch<FluidRef> {
        self.scheduled_ticks
            .collect_fluid_ticks(current_tick, max_ticks)
    }

    /// Vanilla `LevelTicks.copyAreaFrom` for block ticks: every block tick inside
    /// `area` of `source` (this tick's selected batch, already run or not, then
    /// each loaded chunk's live queue) is scheduled here shifted by `offset`,
    /// keeping its deadline and priority. Sub-tick orders are moved past the
    /// copied range's maximum so the copies keep their relative order.
    pub(crate) fn copy_block_ticks_from(
        &self,
        source: &Self,
        area: BoundingBox,
        (dx, dy, dz): (i32, i32, i32),
    ) {
        let mut ticks: Vec<super::BlockTick> = source
            .region_queues
            .block_ticks_this_tick
            .lock()
            .as_ref()
            .map_or_else(Vec::new, |batch| {
                batch
                    .ticks()
                    .iter()
                    .filter(|tick| area.contains_blockpos(tick.pos))
                    .copied()
                    .collect()
            });
        for chunk_x in SectionPos::block_to_section_coord(area.min_x())
            ..=SectionPos::block_to_section_coord(area.max_x())
        {
            for chunk_z in SectionPos::block_to_section_coord(area.min_z())
                ..=SectionPos::block_to_section_coord(area.max_z())
            {
                let in_chunk = source
                    .chunk_map
                    .with_full_chunk(ChunkPos::new(chunk_x, chunk_z), |chunk| {
                        chunk.scheduled_tick_container().live_block_ticks_in(area)
                    })
                    .flatten();
                ticks.extend(in_chunk.into_iter().flatten());
            }
        }
        let (Some(min), Some(max)) = (
            ticks.iter().map(|tick| tick.sub_tick_order).min(),
            ticks.iter().map(|tick| tick.sub_tick_order).max(),
        ) else {
            return;
        };
        for tick in ticks {
            let pos = tick.pos.offset(dx, dy, dz);
            self.chunk_map
                .with_full_chunk(Self::chunk_pos_for_block(pos), |chunk| {
                    chunk.schedule_block_tick(
                        pos,
                        tick.tick_type,
                        tick.trigger_tick,
                        tick.priority,
                        // Java long arithmetic.
                        tick.sub_tick_order
                            .wrapping_sub(min)
                            .wrapping_add(max)
                            .wrapping_add(1),
                    );
                });
        }
    }

    /// Returns whether a selected block tick at `(pos, block)` has not started yet.
    ///
    /// This mirrors `LevelTickAccess.willTickThisTick` and is distinct from
    /// [`Self::has_scheduled_block_tick`], because selected ticks have already
    /// been removed from their owning chunk queue.
    pub fn will_tick_block_this_tick(&self, pos: BlockPos, block: BlockRef) -> bool {
        let batch = self.with_region_queues(|queues| queues.block_ticks_this_tick.lock().clone());
        batch.is_some_and(|batch| batch.contains(pos, block))
    }

    /// Returns whether a selected fluid tick at `(pos, fluid)` has not started yet.
    pub fn will_tick_fluid_this_tick(&self, pos: BlockPos, fluid: FluidRef) -> bool {
        let batch = self.with_region_queues(|queues| queues.fluid_ticks_this_tick.lock().clone());
        batch.is_some_and(|batch| batch.contains(pos, fluid))
    }

    pub(crate) fn begin_scheduled_block_tick_batch(
        &self,
        ticks: Vec<super::BlockTick>,
    ) -> Arc<super::ScheduledTickRunBatch<BlockRef>> {
        let batch = Arc::new(super::ScheduledTickRunBatch::new(ticks));
        self.with_region_queues(|queues| {
            let mut current = queues.block_ticks_this_tick.lock();
            assert!(
                current.is_none(),
                "scheduled block-tick batch was already active"
            );
            *current = Some(Arc::clone(&batch));
        });
        batch
    }

    pub(crate) fn end_scheduled_block_tick_batch(
        &self,
        batch: &Arc<super::ScheduledTickRunBatch<BlockRef>>,
    ) {
        let removed = self.with_region_queues(|queues| {
            let mut current = queues.block_ticks_this_tick.lock();
            assert!(
                current
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, batch)),
                "scheduled block-tick batch identity changed during execution"
            );
            current.take()
        });
        drop(removed);
    }

    pub(crate) fn begin_scheduled_fluid_tick_batch(
        &self,
        ticks: Vec<super::FluidTick>,
    ) -> Arc<super::ScheduledTickRunBatch<FluidRef>> {
        let batch = Arc::new(super::ScheduledTickRunBatch::new(ticks));
        self.with_region_queues(|queues| {
            let mut current = queues.fluid_ticks_this_tick.lock();
            assert!(
                current.is_none(),
                "scheduled fluid-tick batch was already active"
            );
            *current = Some(Arc::clone(&batch));
        });
        batch
    }

    pub(crate) fn end_scheduled_fluid_tick_batch(
        &self,
        batch: &Arc<super::ScheduledTickRunBatch<FluidRef>>,
    ) {
        let removed = self.with_region_queues(|queues| {
            let mut current = queues.fluid_ticks_this_tick.lock();
            assert!(
                current
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, batch)),
                "scheduled fluid-tick batch identity changed during execution"
            );
            current.take()
        });
        drop(removed);
    }
}
