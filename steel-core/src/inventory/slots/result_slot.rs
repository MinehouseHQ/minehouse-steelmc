use std::sync::atomic::{AtomicI32, Ordering};
use std::{mem, sync::Arc};

use steel_registry::item_stack::ItemStack;
use steel_registry::stat::vanilla_stat_types;
use steel_utils::{DowncastType, DowncastTypeKey};

use crate::{
    inventory::{
        lock::{ContainerLockGuard, ContainerRef},
        slots::{CraftedStat, ResultHandler, Slot, SlotStorage},
    },
    player::Player,
};

/// A fake Slot that contains the resulting item of f.e. a craft
pub struct ResultSlot {
    handler: Arc<dyn ResultHandler + Send + Sync>,
    storage: SlotStorage,
    /// Vanilla `removeCount`: the results taken since the crafted stat was last awarded.
    remove_count: AtomicI32,
}

// SAFETY: This key uniquely identifies Steel's `ResultSlot`.
unsafe impl DowncastType for ResultSlot {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:slot/result");
}

impl ResultSlot {
    /// Creates a new `ResultSlot` backed by the handler's result container.
    pub fn new(handler: impl ResultHandler + 'static) -> Self {
        let result_container = handler.result_container();
        let storage =
            SlotStorage::physical(result_container, 0).with_dependencies(handler.dependencies());
        Self {
            handler: Arc::new(handler),
            storage,
            remove_count: AtomicI32::new(0),
        }
    }

    /// Whether this result counts the items taken, like vanilla `ResultSlot` and
    /// `MerchantResultSlot`.
    fn counts_removed(&self) -> bool {
        matches!(
            self.handler.crafted_stat(),
            CraftedStat::Crafting | CraftedStat::Trading
        )
    }

    /// The crafted-stat part of vanilla `checkTakeAchievements`, awarding `ITEM_CRAFTED` for
    /// `carried` as the handler's [`CraftedStat`] says.
    fn award_crafted(&self, carried: &ItemStack, player: &Player) {
        let count = match self.handler.crafted_stat() {
            CraftedStat::None => return,
            CraftedStat::TakenStack => carried.count(),
            CraftedStat::Crafting => match self.remove_count.swap(0, Ordering::Relaxed) {
                0 => return,
                count => count,
            },
            CraftedStat::Trading => self.remove_count.swap(0, Ordering::Relaxed),
        };
        player.award_stat_with_count(&vanilla_stat_types::ITEM_CRAFTED, carried.item(), count);
    }

    /// Returns the result container this slot derived from its handler.
    pub(crate) fn result_container(&self) -> &ContainerRef {
        let Some((container, _)) = self.storage.physical_backing() else {
            unreachable!("ResultSlot always has physical storage");
        };
        container
    }
}

impl Slot for ResultSlot {
    fn storage(&self) -> &SlotStorage {
        &self.storage
    }

    fn get_item<'a>(&self, guard: &'a ContainerLockGuard) -> &'a ItemStack {
        guard
            .get(self.result_container().container_id())
            .expect("failed to get item from result container")
            .get_item(0)
    }

    fn get_item_mut<'a>(&self, guard: &'a mut ContainerLockGuard) -> &'a mut ItemStack {
        guard
            .get_mut(self.result_container().container_id())
            .expect("failed to get item mutabily from result container")
            .get_item_mut(0)
    }

    fn set_item(&self, guard: &mut ContainerLockGuard, stack: ItemStack) {
        guard
            .get_mut(self.result_container().container_id())
            .expect("failed to get item mutabily from result container")
            .set_item(0, stack);
    }

    fn get_max_stack_size(&self, guard: &ContainerLockGuard) -> i32 {
        guard
            .get(self.result_container().container_id())
            .expect("result container not locked")
            .get_max_stack_size()
    }

    fn set_changed(&self, guard: &mut ContainerLockGuard) {
        guard
            .get_mut(self.result_container().container_id())
            .expect("result container not locked")
            .set_changed();
    }

    fn get_container_slot(&self) -> usize {
        0
    }

    fn on_take(
        &self,
        guard: &mut ContainerLockGuard,
        stack: &mut ItemStack,
        player: &Player,
    ) -> Option<ItemStack> {
        self.award_crafted(stack, player);
        self.handler.on_crafted_by(stack, player);
        self.handler.on_result_taken(guard, player)
    }

    fn prepare_quick_move(&self, stack: &mut ItemStack, player: &Player) {
        self.handler.on_crafted_by(stack, player);
    }

    fn remove(&self, guard: &mut ContainerLockGuard, amount: i32) -> ItemStack {
        let taken = mem::take(self.get_item_mut(guard));
        if !taken.is_empty() && self.counts_removed() {
            self.remove_count
                .fetch_add(amount.min(taken.count()), Ordering::Relaxed);
        }
        taken
    }

    fn on_quick_craft_count(&self, picked: &ItemStack, count: i32, player: &Player) {
        if self.counts_removed() {
            self.remove_count.fetch_add(count, Ordering::Relaxed);
            self.award_crafted(picked, player);
        }
    }

    /// Vanilla `ResultSlot.onSwapCraft`; a trade's result slot does not count swaps.
    fn on_swap_craft(&self, count: i32) {
        if self.handler.crafted_stat() == CraftedStat::Crafting {
            self.remove_count.fetch_add(count, Ordering::Relaxed);
        }
    }

    fn is_fake(&self) -> bool {
        true
    }

    fn allow_modification(&self, _guard: &ContainerLockGuard, _player: &Player) -> bool {
        false
    }

    fn may_place(&self, _stack: &ItemStack) -> bool {
        false
    }

    fn may_pickup(&self, guard: &ContainerLockGuard, player: &Player) -> bool {
        self.handler.is_result_valid(guard, player)
    }
}

#[cfg(test)]
mod tests {
    use steel_utils::locks::IntoShared;

    use super::*;
    use crate::inventory::container::SimpleContainer;

    struct NoopResultHandler(ContainerRef);

    impl ResultHandler for NoopResultHandler {
        fn result_container(&self) -> ContainerRef {
            self.0.clone()
        }

        fn dependencies(&self) -> Vec<ContainerRef> {
            Vec::new()
        }

        fn update_result(&self, _guard: &mut ContainerLockGuard, _player: &Player) {}

        fn on_result_taken(
            &self,
            _guard: &mut ContainerLockGuard,
            _player: &Player,
        ) -> Option<ItemStack> {
            None
        }

        fn is_result_valid(&self, _guard: &ContainerLockGuard, _player: &Player) -> bool {
            true
        }
    }

    #[test]
    fn constructor_uses_the_handlers_result_container() {
        let container = ContainerRef::from(SimpleContainer::new(1).into_shared());
        let expected = container.container_id();

        let slot = ResultSlot::new(NoopResultHandler(container));

        assert_eq!(slot.storage().physical_key(), Some((expected, 0)));
    }
}
