//! Vanilla `SlotAccess`: one item slot that commands, loot and predicates can
//! read and replace, whatever holds it.

use steel_registry::item_stack::ItemStack;
use steel_utils::locks::SyncMutex;

use crate::inventory::container::Container;
use crate::inventory::lock::{ContainerLockGuard, ContainerRef};

type Getter<'a> = Box<dyn Fn() -> ItemStack + Send + Sync + 'a>;
type Setter<'a> = Box<dyn Fn(ItemStack) -> bool + Send + Sync + 'a>;

/// A readable and replaceable item slot.
pub struct SlotAccess<'a> {
    get: Getter<'a>,
    set: Setter<'a>,
}

impl<'a> SlotAccess<'a> {
    /// A slot whose setter may refuse a stack, returning `false`.
    pub fn new(
        get: impl Fn() -> ItemStack + Send + Sync + 'a,
        set: impl Fn(ItemStack) -> bool + Send + Sync + 'a,
    ) -> Self {
        Self {
            get: Box::new(get),
            set: Box::new(set),
        }
    }

    /// Vanilla `SlotAccess.of`: a slot that takes any stack.
    pub fn of(
        get: impl Fn() -> ItemStack + Send + Sync + 'a,
        set: impl Fn(ItemStack) + Send + Sync + 'a,
    ) -> Self {
        Self::new(get, move |stack| {
            set(stack);
            true
        })
    }

    /// Vanilla `Container.getSlot` over a container behind its own lock:
    /// `None` outside the container.
    pub fn for_container<C: Container + ?Sized>(
        container: &'a SyncMutex<C>,
        slot: i32,
    ) -> Option<Self> {
        let index = usize::try_from(slot)
            .ok()
            .filter(|&index| index < container.lock().get_container_size())?;
        Some(Self::of(
            move || container.lock().get_item(index).clone(),
            move |stack| container.lock().set_item(index, stack),
        ))
    }

    /// Vanilla `Container.getSlot` over a shared container, such as a block
    /// entity's: setting notifies the container's owner like vanilla `setItem`.
    #[must_use]
    pub fn for_container_ref(container: ContainerRef, slot: i32) -> Option<Self> {
        let id = container.container_id();
        let size = ContainerLockGuard::lock_all(&[&container])
            .get(id)
            .map_or(0, Container::get_container_size);
        let index = usize::try_from(slot).ok().filter(|&index| index < size)?;
        let getter = container.clone();
        Some(Self::of(
            move || {
                ContainerLockGuard::lock_all(&[&getter])
                    .get(id)
                    .map_or_else(ItemStack::empty, |container| {
                        container.get_item(index).clone()
                    })
            },
            move |stack| {
                ContainerLockGuard::lock_all(&[&container]).set_item(id, index, stack);
            },
        ))
    }

    /// Vanilla `SlotAccess.get`: a copy of what the slot holds.
    #[must_use]
    pub fn get(&self) -> ItemStack {
        (self.get)()
    }

    /// Vanilla `SlotAccess.set`: whether the slot took the stack.
    #[must_use]
    pub fn set(&self, stack: ItemStack) -> bool {
        (self.set)(stack)
    }
}

#[cfg(test)]
mod tests;
