//! Vanilla `SculkSensorBlock` and `CalibratedSculkSensorBlock`: blocks that
//! hear vibrations and turn them into a redstone signal.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty, IntProperty, SculkSensorPhase,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::{
    sound_events, vanilla_block_entity_types, vanilla_blocks, vanilla_entities, vanilla_game_events,
};
use steel_utils::types::UpdateFlags;
use steel_utils::value_providers::IntProvider;
use steel_utils::{BlockPos, BlockStateId, Downcast as _};

use crate::behavior::block::{
    BlockBehavior, BlockEntityCreation, schedule_water_tick_if_waterlogged, try_drop_experience,
};
use crate::behavior::blocks::NoteBlock;
use crate::behavior::context::BlockPlaceContext;
use crate::block_entity::entities::SculkSensorBlockEntity;
use crate::block_entity::{BLOCK_ENTITIES, BlockEntityTicker};
use crate::entity::ai::path::PathComputationType;
use crate::entity::{Entity, SharedEntity};
use crate::world::game_event::GameEventContext;
use crate::world::game_event::vibrations::resonance_event_by_frequency;
use crate::world::{LevelReader, ScheduledTickAccess, SignalQueryContext, World};

const PHASE: &EnumProperty<SculkSensorPhase> = &BlockStateProperties::SCULK_SENSOR_PHASE;
const POWER: &IntProperty = &BlockStateProperties::POWER;
const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;
const FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;

/// Vanilla `SculkSensorBlock.ACTIVE_TICKS`.
const ACTIVE_TICKS: i32 = 30;
/// Vanilla `CalibratedSculkSensorBlock.getActiveTicks`.
const CALIBRATED_ACTIVE_TICKS: i32 = 10;
/// Vanilla `SculkSensorBlock.COOLDOWN_TICKS`.
const COOLDOWN_TICKS: i32 = 10;
/// Vanilla `SculkSensorBlock.RESONANCE_PITCH_BEND` notes.
const RESONANCE_NOTES: [u8; 16] = [0, 0, 2, 4, 6, 7, 9, 10, 12, 14, 15, 18, 19, 21, 22, 24];
/// Vanilla `SculkSensorBlock.spawnAfterBreak` experience.
const EXPERIENCE: i32 = 5;

/// Vanilla `SculkSensorBlock`.
#[block_behavior]
pub struct SculkSensorBlock {
    block: BlockRef,
}

/// Vanilla `CalibratedSculkSensorBlock`: hears only the frequency fed into
/// its back, from twice as far.
#[block_behavior]
pub struct CalibratedSculkSensorBlock {
    block: BlockRef,
}

impl SculkSensorBlock {
    /// Creates sculk sensor behavior for `block`.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `canActivate`.
    #[must_use]
    pub fn can_activate(state: BlockStateId) -> bool {
        is_sensor(state) && state.get_value(PHASE) == SculkSensorPhase::Inactive
    }

    /// Vanilla `activate`: powers the sensor for its active ticks and lets
    /// adjacent resonators repeat the frequency.
    pub fn activate(
        source_entity: Option<&SharedEntity>,
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        power: i32,
        frequency: i32,
    ) {
        let block = state.get_block();
        world.set_block(
            pos,
            state
                .set_value(PHASE, SculkSensorPhase::Active)
                .set_value(POWER, u8::try_from(power).unwrap_or(0)),
            UpdateFlags::UPDATE_ALL,
        );
        world.schedule_block_tick_default(pos, block, active_ticks(block));
        update_neighbors(world, pos, block);
        try_resonate_vibration(source_entity, world, pos, frequency);
        world.game_event(
            &vanilla_game_events::SCULK_SENSOR_TENDRILS_CLICKING,
            pos,
            &GameEventContext::new(
                source_entity.map(|entity| entity.as_ref() as &dyn Entity),
                None,
            ),
        );
        if !state.get_value(WATERLOGGED) {
            world.play_sound(
                &sound_events::BLOCK_SCULK_SENSOR_CLICKING,
                SoundSource::Blocks,
                pos,
                1.0,
                rand::random::<f32>() * 0.2 + 0.8,
                None,
            );
        }
    }
}

impl CalibratedSculkSensorBlock {
    /// Creates calibrated sculk sensor behavior for `block`.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

fn is_sensor(state: BlockStateId) -> bool {
    let block = state.get_block();
    block.key == vanilla_blocks::SCULK_SENSOR.key
        || block.key == vanilla_blocks::CALIBRATED_SCULK_SENSOR.key
}

fn active_ticks(block: BlockRef) -> i32 {
    if block.key == vanilla_blocks::CALIBRATED_SCULK_SENSOR.key {
        CALIBRATED_ACTIVE_TICKS
    } else {
        ACTIVE_TICKS
    }
}

/// Vanilla `updateNeighbours`: the sensor and the block under it.
fn update_neighbors(world: &Arc<World>, pos: BlockPos, block: BlockRef) {
    world.update_neighbors_at(pos, block);
    world.update_neighbors_at(pos.below(), block);
}

/// Vanilla `deactivate`.
fn deactivate(world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
    let block = state.get_block();
    world.set_block(
        pos,
        state
            .set_value(PHASE, SculkSensorPhase::Cooldown)
            .set_value(POWER, 0),
        UpdateFlags::UPDATE_ALL,
    );
    world.schedule_block_tick_default(pos, block, COOLDOWN_TICKS);
    update_neighbors(world, pos, block);
}

/// Vanilla `tryResonateVibration`: every adjacent amethyst block repeats the
/// vibration as a resonate event.
fn try_resonate_vibration(
    source_entity: Option<&SharedEntity>,
    world: &Arc<World>,
    pos: BlockPos,
    frequency: i32,
) {
    for direction in Direction::ALL {
        let relative = pos.relative(direction);
        let state = world.get_block_state(relative);
        if !state.get_block().has_tag(&BlockTag::VIBRATION_RESONATORS) {
            continue;
        }
        world.game_event(
            resonance_event_by_frequency(frequency),
            relative,
            &GameEventContext::new(
                source_entity.map(|entity| entity.as_ref() as &dyn Entity),
                Some(state),
            ),
        );
        let note = RESONANCE_NOTES[usize::try_from(frequency).unwrap_or(0)];
        world.play_sound(
            &sound_events::BLOCK_AMETHYST_BLOCK_RESONATE,
            SoundSource::Blocks,
            relative,
            1.0,
            NoteBlock::pitch_from_note(note),
            None,
        );
    }
}

/// The behavior both sensors share, with the calibrated differences passed in.
macro_rules! sculk_sensor_behavior {
    () => {
        fn update_shape(
            &self,
            state: BlockStateId,
            world: &dyn ScheduledTickAccess,
            pos: BlockPos,
            _direction: Direction,
            _neighbor_pos: BlockPos,
            _neighbor_state: BlockStateId,
        ) -> BlockStateId {
            schedule_water_tick_if_waterlogged(state, world, pos);
            state
        }

        /// Vanilla `tick`: an active sensor cools down, a cooling one goes
        /// back to listening.
        fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
            match state.get_value(PHASE) {
                SculkSensorPhase::Active => deactivate(world, pos, state),
                SculkSensorPhase::Cooldown => {
                    world.set_block(
                        pos,
                        state.set_value(PHASE, SculkSensorPhase::Inactive),
                        UpdateFlags::UPDATE_ALL,
                    );
                    if !state.get_value(WATERLOGGED) {
                        world.play_sound(
                            &sound_events::BLOCK_SCULK_SENSOR_CLICKING_STOP,
                            SoundSource::Blocks,
                            pos,
                            1.0,
                            rand::random::<f32>() * 0.2 + 0.8,
                            None,
                        );
                    }
                }
                SculkSensorPhase::Inactive => {}
            }
        }

        /// Vanilla `stepOn`: anything but a warden stepping on the sensor
        /// makes a step vibration right away.
        fn step_on(
            &self,
            state: BlockStateId,
            world: &Arc<World>,
            pos: BlockPos,
            entity: &dyn Entity,
        ) {
            if SculkSensorBlock::can_activate(state)
                && entity.entity_type().key != vanilla_entities::WARDEN.key
                && let Some(block_entity) = world.get_block_entity(pos)
                && let Some(sensor) = block_entity.downcast_ref::<SculkSensorBlockEntity>()
                && sensor.can_receive_vibration(
                    world,
                    pos,
                    &vanilla_game_events::STEP,
                    &GameEventContext::new(None, Some(state)),
                )
            {
                sensor.force_schedule_vibration(
                    world,
                    &vanilla_game_events::STEP,
                    &GameEventContext::new(Some(entity), None),
                    entity.position(),
                );
            }
            self.default_step_on(state, world, pos, entity);
        }

        /// Vanilla `onPlace`: a powered sensor with no tick to end it loses
        /// its power.
        fn on_place(
            &self,
            state: BlockStateId,
            world: &Arc<World>,
            pos: BlockPos,
            old_state: BlockStateId,
            _moved_by_piston: bool,
        ) {
            if state.get_block() != old_state.get_block()
                && state.get_value(POWER) > 0
                && !world.has_scheduled_block_tick(pos, self.block)
            {
                world.set_block(
                    pos,
                    state.set_value(POWER, 0),
                    UpdateFlags::UPDATE_CLIENTS | UpdateFlags::UPDATE_KNOWN_SHAPE,
                );
            }
        }

        fn affect_neighbors_after_removal(
            &self,
            state: BlockStateId,
            world: &Arc<World>,
            pos: BlockPos,
            _moved_by_piston: bool,
        ) {
            if state.get_value(PHASE) == SculkSensorPhase::Active {
                update_neighbors(world, pos, state.get_block());
            }
        }

        fn is_signal_source(&self, _state: BlockStateId, _context: SignalQueryContext) -> bool {
            true
        }

        fn get_own_signal(
            &self,
            state: BlockStateId,
            _world: &dyn LevelReader,
            _pos: BlockPos,
            _context: SignalQueryContext,
        ) -> i32 {
            i32::from(state.get_value(POWER))
        }

        fn get_direct_signal(
            &self,
            state: BlockStateId,
            world: &dyn LevelReader,
            pos: BlockPos,
            direction: Direction,
            context: SignalQueryContext,
        ) -> i32 {
            if direction == Direction::Up {
                self.get_signal(state, world, pos, direction, context)
            } else {
                0
            }
        }

        fn has_analog_output_signal(&self, _state: BlockStateId) -> bool {
            true
        }

        /// Vanilla `getAnalogOutputSignal`: the last frequency while active.
        fn get_analog_output_signal(
            &self,
            state: BlockStateId,
            world: &World,
            pos: BlockPos,
            _direction: Direction,
        ) -> i32 {
            let Some(block_entity) = world.get_block_entity(pos) else {
                return 0;
            };
            let Some(sensor) = block_entity.downcast_ref::<SculkSensorBlockEntity>() else {
                return 0;
            };
            if state.get_value(PHASE) == SculkSensorPhase::Active {
                sensor.last_vibration_frequency()
            } else {
                0
            }
        }

        fn is_pathfindable(
            &self,
            _state: BlockStateId,
            _computation_type: PathComputationType,
        ) -> bool {
            false
        }

        fn spawn_after_break(
            &self,
            _state: BlockStateId,
            world: &Arc<World>,
            pos: BlockPos,
            tool: &ItemStack,
            drop_experience: bool,
        ) {
            if drop_experience {
                try_drop_experience(world, pos, tool, &IntProvider::Constant(EXPERIENCE));
            }
        }

        // `animateTick` emits client-local particles only.
    };
}

impl BlockBehavior for SculkSensorBlock {
    sculk_sensor_behavior!();

    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(WATERLOGGED, context.is_water_source()),
        )
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::SCULK_SENSOR,
            level,
            pos,
            state,
        ))
    }

    fn get_block_entity_ticker(
        &self,
        _world: &Arc<World>,
        _state: BlockStateId,
        block_entity_type: BlockEntityTypeRef,
    ) -> Option<BlockEntityTicker> {
        BlockEntityTicker::for_matching_entity_tick(
            block_entity_type,
            &vanilla_block_entity_types::SCULK_SENSOR,
        )
    }
}

impl BlockBehavior for CalibratedSculkSensorBlock {
    sculk_sensor_behavior!();

    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(WATERLOGGED, context.is_water_source())
                .set_value(FACING, context.horizontal_direction()),
        )
    }

    /// Vanilla `getSignal`: no signal out of its back.
    fn get_signal(
        &self,
        state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
        direction: Direction,
        context: SignalQueryContext,
    ) -> i32 {
        if direction == state.get_value(FACING) {
            0
        } else {
            self.get_own_signal(state, world, pos, context)
        }
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::CALIBRATED_SCULK_SENSOR,
            level,
            pos,
            state,
        ))
    }

    fn get_block_entity_ticker(
        &self,
        _world: &Arc<World>,
        _state: BlockStateId,
        block_entity_type: BlockEntityTypeRef,
    ) -> Option<BlockEntityTicker> {
        BlockEntityTicker::for_matching_entity_tick(
            block_entity_type,
            &vanilla_block_entity_types::CALIBRATED_SCULK_SENSOR,
        )
    }
}
