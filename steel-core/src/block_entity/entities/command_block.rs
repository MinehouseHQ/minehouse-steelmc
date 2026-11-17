//! Vanilla `CommandBlockEntity`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::data_components::{DataComponentMap, vanilla_components::CUSTOM_NAME};
use steel_registry::{vanilla_block_entity_types, vanilla_blocks};
use steel_utils::{BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey};

use crate::block_entity::{BlockEntity, BlockEntityBase, ImplicitComponentGetter};
use crate::command::command_block::{BaseCommandBlock, CommandBlockOrigin};
use crate::world::World;

/// Vanilla `CommandBlockEntity.Mode`, chosen by which command block holds the entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandBlockMode {
    /// A chain command block: runs when the block before it in a chain runs.
    Sequence,
    /// A repeating command block: runs every tick while active.
    Auto,
    /// An impulse command block: runs once when powered.
    Redstone,
}

impl CommandBlockMode {
    /// The mode of a command block state: impulse unless it is a repeating or
    /// chain command block, vanilla `CommandBlockEntity.getMode`.
    #[must_use]
    pub fn of(state: BlockStateId) -> Self {
        let block = state.get_block();
        if block == &vanilla_blocks::REPEATING_COMMAND_BLOCK {
            Self::Auto
        } else if block == &vanilla_blocks::CHAIN_COMMAND_BLOCK {
            Self::Sequence
        } else {
            Self::Redstone
        }
    }
}

/// Vanilla `CommandBlockEntity`: the command a command block runs and its
/// redstone state.
pub struct CommandBlockEntity {
    base: BlockEntityBase,
    command_block: Arc<BaseCommandBlock>,
    powered: AtomicBool,
    auto: AtomicBool,
    condition_met: AtomicBool,
}

// SAFETY: This key is owned by Steel and uniquely identifies `CommandBlockEntity`.
unsafe impl DowncastType for CommandBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/command_block");
}

impl CommandBlockEntity {
    /// Creates an unpowered command block entity with no command.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::COMMAND_BLOCK,
                level,
                pos,
                state,
            ),
            command_block: Arc::new(BaseCommandBlock::new()),
            powered: AtomicBool::new(false),
            auto: AtomicBool::new(false),
            condition_met: AtomicBool::new(false),
        }
    }

    /// Sets whether the block is always active without scheduling anything,
    /// as vanilla's `setAutomatic` does before the entity has a level.
    pub fn load_automatic(&self, auto: bool) {
        self.auto.store(auto, Ordering::Relaxed);
    }

    /// Vanilla `getCommandBlock`.
    #[must_use]
    pub const fn command_block(&self) -> &Arc<BaseCommandBlock> {
        &self.command_block
    }

    /// Vanilla `CommandBlockEntity.commandBlock.setCommand`, which also marks
    /// the block entity changed.
    pub fn set_command(&self, command: String) {
        self.command_block.set_command(command);
        self.set_changed();
    }

    /// Vanilla `isPowered`.
    #[must_use]
    pub fn is_powered(&self) -> bool {
        self.powered.load(Ordering::Relaxed)
    }

    /// Vanilla `setPowered`.
    pub fn set_powered(&self, powered: bool) {
        self.powered.store(powered, Ordering::Relaxed);
    }

    /// Vanilla `isAutomatic`: whether the block runs without redstone ("Always Active").
    #[must_use]
    pub fn is_automatic(&self) -> bool {
        self.auto.load(Ordering::Relaxed)
    }

    /// Vanilla `setAutomatic`: switching a non-chain block to always active
    /// while unpowered schedules its first run.
    pub fn set_automatic(&self, world: &World, auto: bool) {
        let previous = self.auto.swap(auto, Ordering::Relaxed);
        if !previous && auto && !self.is_powered() && self.mode() != CommandBlockMode::Sequence {
            self.schedule_tick(world);
        }
    }

    /// Vanilla `onModeSwitch`: a block that became repeating while active
    /// starts running.
    pub fn on_mode_switch(&self, world: &World) {
        if self.mode() == CommandBlockMode::Auto && (self.is_powered() || self.is_automatic()) {
            self.schedule_tick(world);
        }
    }

    fn schedule_tick(&self, world: &World) {
        let block = self.get_block_state().get_block();
        if is_command_block(self.get_block_state()) {
            self.mark_condition_met(world);
            world.schedule_block_tick_default(self.get_block_pos(), block, 1);
        }
    }

    /// Vanilla `wasConditionMet`.
    #[must_use]
    pub fn was_condition_met(&self) -> bool {
        self.condition_met.load(Ordering::Relaxed)
    }

    /// Vanilla `markConditionMet`: a conditional block's condition is met when
    /// the command block behind it last succeeded.
    pub fn mark_condition_met(&self, world: &World) -> bool {
        let mut condition_met = true;
        if self.is_conditional(world) {
            let pos = self.get_block_pos();
            let facing = world
                .get_block_state(pos)
                .get_value(&BlockStateProperties::FACING);
            let behind = pos.relative(facing.opposite());
            condition_met = is_command_block(world.get_block_state(behind))
                && world.get_block_entity(behind).is_some_and(|entity| {
                    entity
                        .downcast_ref::<Self>()
                        .is_some_and(|entity| entity.command_block.success_count() > 0)
                });
        }
        self.condition_met.store(condition_met, Ordering::Relaxed);
        condition_met
    }

    /// Vanilla `getMode`.
    #[must_use]
    pub fn mode(&self) -> CommandBlockMode {
        CommandBlockMode::of(self.get_block_state())
    }

    /// Vanilla `isConditional`, read from the block in the world.
    #[must_use]
    pub fn is_conditional(&self, world: &World) -> bool {
        let state = world.get_block_state(self.get_block_pos());
        is_command_block(state) && state.get_value(&BlockStateProperties::CONDITIONAL)
    }

    /// Where this block's command runs: the block center, facing the way the
    /// block faces, vanilla `createCommandSourceStack`.
    pub(crate) fn origin(&self) -> CommandBlockOrigin {
        let pos = self.get_block_pos();
        let facing = self
            .get_block_state()
            .get_value(&BlockStateProperties::FACING);
        CommandBlockOrigin {
            position: DVec3::new(
                f64::from(pos.x()) + 0.5,
                f64::from(pos.y()) + 0.5,
                f64::from(pos.z()) + 0.5,
            ),
            rotation: (facing.to_yaw(), 0.0),
            entity: None,
            // Vanilla `onUpdated`: resend the block so watching clients refresh.
            on_updated: Arc::new(move |world: &World| world.send_block_updated(pos)),
        }
    }
}

/// Whether `state` is one of the three command blocks.
#[must_use]
pub fn is_command_block(state: BlockStateId) -> bool {
    let block = state.get_block();
    block == &vanilla_blocks::COMMAND_BLOCK
        || block == &vanilla_blocks::REPEATING_COMMAND_BLOCK
        || block == &vanilla_blocks::CHAIN_COMMAND_BLOCK
}

impl BlockEntity for CommandBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt: NbtCompoundView<'_, '_> = nbt.into();
        self.command_block.load(&nbt);
        let boolean = |key: &str| nbt.byte(key).is_some_and(|value| value != 0);
        self.powered.store(boolean("powered"), Ordering::Relaxed);
        self.condition_met
            .store(boolean("conditionMet"), Ordering::Relaxed);
        // Vanilla loads through `setAutomatic`, which only schedules a tick
        // once the entity has a level; chunk loading runs before that, and
        // the scheduled tick is saved with the chunk.
        self.load_automatic(boolean("auto"));
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.command_block.save(nbt);
        nbt.insert("powered", i8::from(self.is_powered()));
        nbt.insert("conditionMet", i8::from(self.was_condition_met()));
        nbt.insert("auto", i8::from(self.is_automatic()));
    }

    fn apply_implicit_components(&self, components: &mut ImplicitComponentGetter<'_>) {
        self.command_block
            .set_custom_name(components.get(CUSTOM_NAME));
    }

    fn collect_implicit_components(&self, components: &mut DataComponentMap) {
        components.set(CUSTOM_NAME, self.command_block.custom_name());
    }
}
