//! Vanilla `TestBlockEntity`: the start, log, fail and accept markers game
//! tests are built from.

use std::sync::{Arc, Weak};

use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, PropertyEnum as _, TestBlockMode};
use steel_registry::{vanilla_block_entity_types, vanilla_blocks};
use steel_utils::locks::SyncMutex;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;

/// Every test block mode, in vanilla's id order.
pub const TEST_BLOCK_MODES: [TestBlockMode; 4] = [
    TestBlockMode::Start,
    TestBlockMode::Log,
    TestBlockMode::Fail,
    TestBlockMode::Accept,
];

/// Vanilla `TestBlockMode.CODEC`: the mode's lowercase name.
#[must_use]
pub fn test_block_mode_by_name(name: &str) -> Option<TestBlockMode> {
    TEST_BLOCK_MODES
        .into_iter()
        .find(|mode| mode.as_str() == name)
}

/// The mode a test block's state shows.
#[must_use]
pub fn test_block_mode(state: BlockStateId) -> TestBlockMode {
    state
        .try_get_value(&BlockStateProperties::TEST_BLOCK_MODE)
        .unwrap_or(TestBlockMode::Start)
}

struct TestBlockData {
    mode: TestBlockMode,
    message: String,
    powered: bool,
    triggered: bool,
}

/// Vanilla `TestBlockEntity`.
pub struct TestBlockEntity {
    base: BlockEntityBase,
    data: SyncMutex<TestBlockData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `TestBlockEntity`.
unsafe impl DowncastType for TestBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/test_block");
}

impl TestBlockEntity {
    /// Vanilla `TestBlockEntity(pos, state)`: the mode comes from the state.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(&vanilla_block_entity_types::TEST_BLOCK, level, pos, state),
            data: SyncMutex::new(TestBlockData {
                mode: test_block_mode(state),
                message: String::new(),
                powered: false,
                triggered: false,
            }),
        }
    }

    /// Vanilla `getMode`.
    #[must_use]
    pub fn mode(&self) -> TestBlockMode {
        self.data.lock().mode
    }

    /// Vanilla `setMode`: also turns the block to show the mode.
    pub fn set_mode(&self, world: &Arc<World>, mode: TestBlockMode) {
        self.data.lock().mode = mode;
        let pos = self.get_block_pos();
        let state = world.get_block_state(pos);
        if state.get_block() == &vanilla_blocks::TEST_BLOCK {
            world.set_block(
                pos,
                state.set_value(&BlockStateProperties::TEST_BLOCK_MODE, mode),
                UpdateFlags::UPDATE_CLIENTS,
            );
        }
    }

    /// Vanilla `getMessage`.
    #[must_use]
    pub fn message(&self) -> String {
        self.data.lock().message.clone()
    }

    /// Vanilla `setMessage`.
    pub fn set_message(&self, message: String) {
        self.data.lock().message = message;
    }

    /// Vanilla `isPowered`.
    #[must_use]
    pub fn is_powered(&self) -> bool {
        self.data.lock().powered
    }

    /// Vanilla `setPowered`.
    pub fn set_powered(&self, powered: bool) {
        self.data.lock().powered = powered;
    }

    /// Vanilla `hasTriggered`.
    #[must_use]
    pub fn has_triggered(&self) -> bool {
        self.data.lock().triggered
    }

    /// Vanilla `reset`: a start block stops powering its neighbors.
    pub fn reset(&self, world: &Arc<World>) {
        let mut data = self.data.lock();
        data.triggered = false;
        if data.mode != TestBlockMode::Start {
            return;
        }
        data.powered = false;
        drop(data);
        world.update_neighbors_at(self.get_block_pos(), self.get_block_state().get_block());
    }

    /// Vanilla `trigger`: a start block powers its neighbors; the others
    /// remember they were reached, and log blocks log their message.
    pub fn trigger(&self, world: &Arc<World>) {
        let mode = self.mode();
        if mode == TestBlockMode::Start {
            self.set_powered(true);
            world.update_neighbors_at(self.get_block_pos(), self.get_block_state().get_block());
            self.log();
        } else {
            if mode == TestBlockMode::Log {
                self.log();
            }
            self.data.lock().triggered = true;
        }
    }

    /// Vanilla `log`.
    pub fn log(&self) {
        let pos = self.get_block_pos();
        let data = self.data.lock();
        if !data.message.trim().is_empty() {
            log::info!(
                "Test {} (at BlockPos{{x={}, y={}, z={}}}): {}",
                data.mode.as_str(),
                pos.x(),
                pos.y(),
                pos.z(),
                data.message
            );
        }
    }
}

impl BlockEntity for TestBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt: NbtCompoundView<'_, '_> = nbt.into();
        let mut data = self.data.lock();
        data.mode = nbt
            .string("mode")
            .and_then(|name| test_block_mode_by_name(&name.to_str()))
            .unwrap_or(TestBlockMode::Fail);
        data.message = nbt
            .string("message")
            .map_or_else(String::new, |message| message.to_str().into_owned());
        data.powered = nbt.byte("powered").is_some_and(|powered| powered != 0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let data = self.data.lock();
        nbt.insert("mode", data.mode.as_str());
        nbt.insert("message", data.message.clone());
        nbt.insert("powered", i8::from(data.powered));
    }

    /// Vanilla `getUpdateTag`.
    fn get_update_tag(&self) -> Option<NbtCompound> {
        Some(self.save_custom_only())
    }
}
