//! Crafter menu slots.

use std::sync::Arc;

use steel_registry::item_stack::ItemStack;
use steel_utils::{DowncastType, DowncastTypeKey};

use crate::block_entity::entities::CrafterData;
use crate::inventory::lock::{ContainerLockGuard, ContainerRef};
use crate::inventory::slots::{NormalSlot, Slot, SlotStorage};
use crate::player::Player;

/// Vanilla `CrafterSlot`: a grid slot that takes nothing while disabled.
pub struct CrafterSlot {
    base: NormalSlot,
    data: Arc<CrafterData>,
}

// SAFETY: This Steel-owned key uniquely identifies crafter grid slots.
unsafe impl DowncastType for CrafterSlot {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:slot/crafter");
}

impl CrafterSlot {
    /// Creates a crafter grid slot reading the slot states from `data`.
    #[must_use]
    pub fn new(container: impl Into<ContainerRef>, index: usize, data: Arc<CrafterData>) -> Self {
        Self {
            base: NormalSlot::new(container, index),
            data,
        }
    }
}

impl Slot for CrafterSlot {
    fn storage(&self) -> &SlotStorage {
        self.base.storage()
    }

    fn get_item<'a>(&self, guard: &'a ContainerLockGuard) -> &'a ItemStack {
        self.base.get_item(guard)
    }

    fn get_item_mut<'a>(&self, guard: &'a mut ContainerLockGuard) -> &'a mut ItemStack {
        self.base.get_item_mut(guard)
    }

    fn set_item(&self, guard: &mut ContainerLockGuard, stack: ItemStack) {
        self.base.set_item(guard, stack);
    }

    fn remove(&self, guard: &mut ContainerLockGuard, amount: i32) -> ItemStack {
        self.base.remove(guard, amount)
    }

    /// Vanilla `CrafterSlot.mayPlace`.
    fn may_place(&self, _stack: &ItemStack) -> bool {
        !self.data.is_slot_disabled(self.base.get_container_slot())
    }

    fn get_max_stack_size(&self, guard: &ContainerLockGuard) -> i32 {
        self.base.get_max_stack_size(guard)
    }

    fn set_changed(&self, guard: &mut ContainerLockGuard) {
        self.base.set_changed(guard);
    }

    fn get_container_slot(&self) -> usize {
        self.base.get_container_slot()
    }
}

/// Vanilla `NonInteractiveResultSlot`: shows a result that cannot be taken,
/// replaced or clicked.
pub struct NonInteractiveResultSlot {
    base: NormalSlot,
}

// SAFETY: This Steel-owned key uniquely identifies non-interactive result slots.
unsafe impl DowncastType for NonInteractiveResultSlot {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:slot/non_interactive_result");
}

impl NonInteractiveResultSlot {
    /// Creates a read-only result slot over `index` of `container`.
    #[must_use]
    pub fn new(container: impl Into<ContainerRef>, index: usize) -> Self {
        Self {
            base: NormalSlot::new(container, index),
        }
    }
}

impl Slot for NonInteractiveResultSlot {
    fn storage(&self) -> &SlotStorage {
        self.base.storage()
    }

    fn get_item<'a>(&self, guard: &'a ContainerLockGuard) -> &'a ItemStack {
        self.base.get_item(guard)
    }

    fn get_item_mut<'a>(&self, guard: &'a mut ContainerLockGuard) -> &'a mut ItemStack {
        self.base.get_item_mut(guard)
    }

    fn set_item(&self, guard: &mut ContainerLockGuard, stack: ItemStack) {
        self.base.set_item(guard, stack);
    }

    fn may_place(&self, _stack: &ItemStack) -> bool {
        false
    }

    fn may_pickup(&self, _guard: &ContainerLockGuard, _player: &Player) -> bool {
        false
    }

    fn allow_modification(&self, _guard: &ContainerLockGuard, _player: &Player) -> bool {
        false
    }

    fn remove(&self, _guard: &mut ContainerLockGuard, _amount: i32) -> ItemStack {
        ItemStack::empty()
    }

    fn try_remove(
        &self,
        _guard: &mut ContainerLockGuard,
        _amount: i32,
        _max_amount: i32,
        _player: &Player,
    ) -> Option<ItemStack> {
        None
    }

    fn on_take(
        &self,
        _guard: &mut ContainerLockGuard,
        _stack: &mut ItemStack,
        _player: &Player,
    ) -> Option<ItemStack> {
        None
    }

    fn safe_take(
        &self,
        _guard: &mut ContainerLockGuard,
        _amount: i32,
        _max_amount: i32,
        _player: &Player,
    ) -> ItemStack {
        ItemStack::empty()
    }

    fn safe_insert(
        &self,
        _guard: &mut ContainerLockGuard,
        input: ItemStack,
        _amount: i32,
    ) -> ItemStack {
        input
    }

    fn get_max_stack_size(&self, guard: &ContainerLockGuard) -> i32 {
        self.base.get_max_stack_size(guard)
    }

    fn set_changed(&self, guard: &mut ContainerLockGuard) {
        self.base.set_changed(guard);
    }

    fn get_container_slot(&self) -> usize {
        self.base.get_container_slot()
    }

    fn is_fake(&self) -> bool {
        true
    }
}
