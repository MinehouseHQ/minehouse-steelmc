//! Vanilla `SculkSensorBlockEntity` and `CalibratedSculkSensorBlockEntity`:
//! the vibration listener of a sculk sensor.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::game_events::GameEventRef;
use steel_registry::position_source::{BlockPositionSource, PositionSource};
use steel_registry::{
    vanilla_block_entity_types, vanilla_game_events, vanilla_position_source_types,
};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::behavior::blocks::SculkSensorBlock;
use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::entity::SharedEntity;
use crate::world::game_event::vibrations::{
    VIBRATION_LISTENER_TAG, VibrationData, VibrationListener, VibrationSystem, VibrationUser,
    game_event_frequency, redstone_strength_for_distance, tick_vibrations,
};
use crate::world::game_event::{GameEventContext, SharedGameEventListener};
use crate::world::{SignalGetter as _, World};

const LAST_VIBRATION_FREQUENCY_TAG: &str = "last_vibration_frequency";

/// Vanilla `SculkSensorBlockEntity.VibrationUser.LISTENER_RANGE`.
const LISTENER_RANGE: i32 = 8;
/// Vanilla `CalibratedSculkSensorBlockEntity.VibrationUser.getListenerRadius`.
const CALIBRATED_LISTENER_RANGE: i32 = 16;

/// Vanilla `SculkSensorBlockEntity.VibrationUser` with the vibration data the
/// block entity owns.
struct SensorVibrations {
    pos: BlockPos,
    calibrated: bool,
    data: SyncMutex<VibrationData>,
    last_vibration_frequency: AtomicI32,
    /// Vanilla `onDataChanged` → `setChanged`, applied by the block entity tick.
    data_changed: AtomicBool,
}

impl SensorVibrations {
    /// Vanilla `CalibratedSculkSensorBlockEntity.VibrationUser.getBackSignal`.
    fn back_signal(&self, world: &World, state: BlockStateId) -> i32 {
        let direction = state
            .get_value(&BlockStateProperties::HORIZONTAL_FACING)
            .opposite();
        world.get_signal(self.pos.relative(direction), direction)
    }
}

impl VibrationUser for SensorVibrations {
    fn vibration_listener_radius(&self) -> i32 {
        if self.calibrated {
            CALIBRATED_LISTENER_RANGE
        } else {
            LISTENER_RANGE
        }
    }

    fn vibration_position_source(&self) -> PositionSource {
        PositionSource::new(
            &vanilla_position_source_types::BLOCK,
            BlockPositionSource::new(self.pos),
        )
    }

    fn vibration_listener_pos(&self) -> Option<DVec3> {
        let (x, y, z) = self.pos.get_center();
        Some(DVec3::new(x, y, z))
    }

    fn can_trigger_avoid_vibration(&self) -> bool {
        true
    }

    fn can_receive_vibration(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        event: GameEventRef,
        _context: &GameEventContext<'_>,
    ) -> bool {
        let state = world.get_block_state(self.pos);
        if self.calibrated {
            let comparison = self.back_signal(world, state);
            if comparison != 0 && game_event_frequency(event) != comparison {
                return false;
            }
        }
        if pos == self.pos
            && (event.key == vanilla_game_events::BLOCK_DESTROY.key
                || event.key == vanilla_game_events::BLOCK_PLACE.key)
        {
            return false;
        }
        game_event_frequency(event) != 0 && SculkSensorBlock::can_activate(state)
    }

    fn on_receive_vibration(
        &self,
        world: &Arc<World>,
        _pos: BlockPos,
        event: GameEventRef,
        source_entity: Option<&SharedEntity>,
        _projectile_owner: Option<&SharedEntity>,
        receiving_distance: f32,
    ) {
        let state = world.get_block_state(self.pos);
        if !SculkSensorBlock::can_activate(state) {
            return;
        }
        let frequency = game_event_frequency(event);
        self.last_vibration_frequency
            .store(frequency, Ordering::Relaxed);
        let power =
            redstone_strength_for_distance(receiving_distance, self.vibration_listener_radius());
        SculkSensorBlock::activate(source_entity, world, self.pos, state, power, frequency);
    }

    fn on_vibration_data_changed(&self) {
        self.data_changed.store(true, Ordering::Relaxed);
    }
}

impl VibrationSystem for SensorVibrations {
    fn vibration_data(&self) -> &SyncMutex<VibrationData> {
        &self.data
    }
}

/// Vanilla `SculkSensorBlockEntity`, also the calibrated sensor's.
pub struct SculkSensorBlockEntity {
    base: BlockEntityBase,
    vibrations: Arc<SensorVibrations>,
    listener: Arc<VibrationListener>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `SculkSensorBlockEntity`.
unsafe impl DowncastType for SculkSensorBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/sculk_sensor");
}

impl SculkSensorBlockEntity {
    /// Creates a sculk sensor.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self::of_type(&vanilla_block_entity_types::SCULK_SENSOR, level, pos, state)
    }

    /// Creates a calibrated sculk sensor.
    #[must_use]
    pub fn new_calibrated(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self::of_type(
            &vanilla_block_entity_types::CALIBRATED_SCULK_SENSOR,
            level,
            pos,
            state,
        )
    }

    fn of_type(
        block_entity_type: BlockEntityTypeRef,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> Self {
        let calibrated =
            block_entity_type.key == vanilla_block_entity_types::CALIBRATED_SCULK_SENSOR.key;
        let vibrations = Arc::new(SensorVibrations {
            pos,
            calibrated,
            data: SyncMutex::new(VibrationData::default()),
            last_vibration_frequency: AtomicI32::new(0),
            data_changed: AtomicBool::new(false),
        });
        let system: Arc<dyn VibrationSystem> = Arc::clone(&vibrations) as _;
        Self {
            base: BlockEntityBase::new(block_entity_type, level, pos, state),
            listener: Arc::new(VibrationListener::for_system(&system)),
            vibrations,
        }
    }

    /// Vanilla `getLastVibrationFrequency`.
    #[must_use]
    pub fn last_vibration_frequency(&self) -> i32 {
        self.vibrations
            .last_vibration_frequency
            .load(Ordering::Relaxed)
    }

    /// Vanilla `getVibrationUser().canReceiveVibration`.
    #[must_use]
    pub fn can_receive_vibration(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        event: GameEventRef,
        context: &GameEventContext<'_>,
    ) -> bool {
        self.vibrations
            .can_receive_vibration(world, pos, event, context)
    }

    /// Vanilla `getListener().forceScheduleVibration`.
    pub fn force_schedule_vibration(
        &self,
        world: &World,
        event: GameEventRef,
        context: &GameEventContext<'_>,
        origin: DVec3,
    ) {
        self.listener
            .force_schedule_vibration(world, event, context, origin);
    }

    /// The vibration on its way to the sensor, if any.
    #[must_use]
    pub fn has_current_vibration(&self) -> bool {
        self.vibrations.data.lock().current_vibration().is_some()
    }
}

impl BlockEntity for SculkSensorBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt: NbtCompoundView<'_, '_> = nbt.into();
        self.vibrations.last_vibration_frequency.store(
            nbt.int(LAST_VIBRATION_FREQUENCY_TAG).unwrap_or(0),
            Ordering::Relaxed,
        );
        *self.vibrations.data.lock() = nbt
            .compound(VIBRATION_LISTENER_TAG)
            .and_then(VibrationData::load)
            .unwrap_or_default();
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        nbt.insert(
            LAST_VIBRATION_FREQUENCY_TAG,
            self.last_vibration_frequency(),
        );
        nbt.insert(VIBRATION_LISTENER_TAG, self.vibrations.data.lock().save());
    }

    fn game_event_listener(&self) -> Option<SharedGameEventListener> {
        Some(Arc::clone(&self.listener) as SharedGameEventListener)
    }

    /// Vanilla `VibrationSystem.Ticker.tick`.
    fn tick(&self, world: &Arc<World>) {
        tick_vibrations(world, self.vibrations.as_ref());
        if self.vibrations.data_changed.swap(false, Ordering::Relaxed) {
            self.set_changed();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use simdnbt::borrow::read_compound;
    use steel_registry::blocks::properties::{Direction, EnumProperty, SculkSensorPhase};
    use steel_registry::{init_vanilla_registry, vanilla_blocks};
    use steel_utils::types::UpdateFlags;
    use steel_utils::{ChunkPos, Downcast as _};

    use super::*;
    use crate::behavior::{BLOCK_BEHAVIORS, init_behaviors};
    use crate::block_entity::{SharedBlockEntity, init_block_entities};
    use crate::test_support::{TestWorld, fresh_test_world, insert_ready_full_chunk};
    use crate::world::SignalQueryContext;

    const SENSOR: BlockPos = BlockPos::new(8, 64, 8);
    const PHASE: &EnumProperty<SculkSensorPhase> = &BlockStateProperties::SCULK_SENSOR_PHASE;

    fn arena(name: &'static str, state: BlockStateId) -> TestWorld {
        init_vanilla_registry();
        init_behaviors();
        init_block_entities();
        let fixture = fresh_test_world(name);
        insert_ready_full_chunk(&fixture.world, ChunkPos::from_block_pos(SENSOR));
        assert!(
            fixture
                .world
                .set_block(SENSOR, state, UpdateFlags::UPDATE_ALL)
        );
        fixture
    }

    fn block_entity(world: &World) -> SharedBlockEntity {
        world.get_block_entity(SENSOR).expect("a sculk sensor")
    }

    fn sensor(block_entity: &SharedBlockEntity) -> &SculkSensorBlockEntity {
        block_entity
            .downcast_ref::<SculkSensorBlockEntity>()
            .expect("a sculk sensor block entity")
    }

    fn phase(world: &World) -> SculkSensorPhase {
        world.get_block_state(SENSOR).get_value(PHASE)
    }

    fn power(world: &World) -> u8 {
        world
            .get_block_state(SENSOR)
            .get_value(&BlockStateProperties::POWER)
    }

    /// Ticks the block entity until the vibration in flight lands.
    fn tick_until_received(world: &Arc<World>) {
        let block_entity = block_entity(world);
        for _ in 0..20 {
            block_entity.tick(world);
            world.level_data.write().advance_game_time();
        }
    }

    /// Runs the sensor's scheduled block tick.
    fn scheduled_tick(world: &Arc<World>) {
        let state = world.get_block_state(SENSOR);
        BLOCK_BEHAVIORS
            .get_behavior(state.get_block())
            .tick(state, world, SENSOR);
    }

    fn place_event(world: &Arc<World>, pos: BlockPos) {
        world.game_event(
            &vanilla_game_events::BLOCK_PLACE,
            pos,
            &GameEventContext::new(None, None),
        );
    }

    #[test]
    fn a_vibration_powers_the_sensor_by_distance_then_it_cools_down() {
        init_vanilla_registry();
        let fixture = arena(
            "sculk_sensor_activation",
            vanilla_blocks::SCULK_SENSOR.default_state(),
        );
        let world = &fixture.world;
        place_event(world, BlockPos::new(8, 64, 12));
        tick_until_received(world);

        // Four blocks away out of eight: 15 - floor(15 / 8 * 4).
        assert_eq!(phase(world), SculkSensorPhase::Active);
        assert_eq!(power(world), 8);
        assert_eq!(sensor(&block_entity(world)).last_vibration_frequency(), 13);
        let state = world.get_block_state(SENSOR);
        assert_eq!(
            BLOCK_BEHAVIORS
                .get_behavior(state.get_block())
                .get_analog_output_signal(state, world, SENSOR, Direction::North),
            13
        );

        // While active it hears nothing new.
        place_event(world, BlockPos::new(8, 64, 10));
        tick_until_received(world);
        assert_eq!(power(world), 8);

        scheduled_tick(world);
        assert_eq!(phase(world), SculkSensorPhase::Cooldown);
        assert_eq!(power(world), 0);
        scheduled_tick(world);
        assert_eq!(phase(world), SculkSensorPhase::Inactive);
    }

    #[test]
    fn a_vibration_out_of_range_is_not_heard() {
        init_vanilla_registry();
        let fixture = arena(
            "sculk_sensor_range",
            vanilla_blocks::SCULK_SENSOR.default_state(),
        );
        let world = &fixture.world;
        place_event(world, BlockPos::new(8, 64, 18));
        tick_until_received(world);
        assert_eq!(phase(world), SculkSensorPhase::Inactive);
    }

    #[test]
    fn a_calibrated_sensor_hears_only_the_frequency_fed_into_its_back() {
        init_vanilla_registry();
        let calibrated = vanilla_blocks::CALIBRATED_SCULK_SENSOR
            .default_state()
            .set_value(&BlockStateProperties::HORIZONTAL_FACING, Direction::North);
        let fixture = arena("calibrated_sculk_sensor_filter", calibrated);
        let world = &fixture.world;
        let back = SENSOR.relative(Direction::South);
        assert!(world.set_block(
            back,
            vanilla_blocks::REDSTONE_BLOCK.default_state(),
            UpdateFlags::UPDATE_ALL,
        ));

        // A block placement is frequency 13, not the 15 at its back.
        place_event(world, BlockPos::new(8, 64, 2));
        tick_until_received(world);
        assert_eq!(phase(world), SculkSensorPhase::Inactive);

        assert!(world.set_block(
            back,
            vanilla_blocks::AIR.default_state(),
            UpdateFlags::UPDATE_ALL,
        ));
        place_event(world, BlockPos::new(8, 64, 2));
        tick_until_received(world);
        // Six blocks away out of sixteen: 15 - floor(15 / 16 * 6).
        assert_eq!(phase(world), SculkSensorPhase::Active);
        assert_eq!(power(world), 10);

        // No signal comes out of its back.
        let state = world.get_block_state(SENSOR);
        let behavior = BLOCK_BEHAVIORS.get_behavior(state.get_block());
        let context = SignalQueryContext::DEFAULT;
        assert_eq!(
            behavior.get_signal(state, world.as_ref(), SENSOR, Direction::North, context),
            0
        );
        assert_eq!(
            behavior.get_signal(state, world.as_ref(), SENSOR, Direction::South, context),
            10
        );
    }

    #[test]
    fn the_frequency_and_the_vibration_in_flight_survive_a_reload() {
        init_vanilla_registry();
        let fixture = arena(
            "sculk_sensor_reload",
            vanilla_blocks::SCULK_SENSOR.default_state(),
        );
        let world = &fixture.world;
        let block_entity = block_entity(world);
        let original = sensor(&block_entity);
        original
            .vibrations
            .last_vibration_frequency
            .store(7, Ordering::Relaxed);
        // The candidate becomes the vibration in flight on a later tick.
        place_event(world, BlockPos::new(8, 64, 12));
        world.level_data.write().advance_game_time();
        block_entity.tick(world);
        assert!(original.has_current_vibration());

        let mut saved = NbtCompound::new();
        original.save_additional(&mut saved);
        let mut bytes = Vec::new();
        saved.write(&mut bytes);
        let borrowed = read_compound(&mut Cursor::new(bytes.as_slice())).expect("valid nbt");
        let reloaded = SculkSensorBlockEntity::new(
            Weak::new(),
            SENSOR,
            vanilla_blocks::SCULK_SENSOR.default_state(),
        );
        reloaded.load_additional(&borrowed);

        assert_eq!(reloaded.last_vibration_frequency(), 7);
        assert!(reloaded.has_current_vibration());
        let mut resaved = NbtCompound::new();
        reloaded.save_additional(&mut resaved);
        assert_eq!(resaved, saved);
    }
}
