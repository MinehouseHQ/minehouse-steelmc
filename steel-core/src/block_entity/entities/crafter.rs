//! Vanilla `CrafterBlockEntity`: a 3 by 3 crafting grid whose slots can be
//! switched off, crafted from by a redstone pulse.

use std::{
    mem,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU16, Ordering},
    },
};

use simdnbt::ToNbtTag;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::data_components::DataComponentMap;
use steel_registry::item_stack::ItemStack;
use steel_registry::{vanilla_block_entity_types, vanilla_blocks};
use steel_utils::types::UpdateFlags;
use steel_utils::{
    BlockPos, BlockStateId, DowncastType, DowncastTypeKey, locks::SyncMutex, translations,
};
use text_components::TextComponent;

use crate::block_entity::container_loot::{ContainerLoot, LOOT_TABLE_SEED_TAG, LOOT_TABLE_TAG};
use crate::block_entity::container_name::ContainerName;
use crate::block_entity::{BlockEntity, BlockEntityBase, ImplicitComponentGetter};
use crate::inventory::container::Container;
use crate::inventory::lock::{ContainerLockGuard, ContainerRef, SharedContainer};
use crate::world::World;

/// Vanilla `CrafterBlockEntity.CONTAINER_WIDTH` and `CONTAINER_HEIGHT`.
pub const CRAFTER_GRID_SIZE: usize = 3;
/// Vanilla `CrafterBlockEntity.CONTAINER_SIZE`.
pub const CRAFTER_SLOTS: usize = 9;
/// Vanilla `CrafterBlockEntity.DATA_TRIGGERED`: the menu data slot holding the
/// powered flag, after the nine slot states.
pub const CRAFTER_DATA_TRIGGERED: usize = 9;
/// Vanilla `CrafterBlockEntity.NUM_DATA`.
pub const CRAFTER_NUM_DATA: usize = 10;

const CRAFTING_TICKS_TAG: &str = "crafting_ticks_remaining";
const DISABLED_SLOTS_TAG: &str = "disabled_slots";
const TRIGGERED_TAG: &str = "triggered";

/// Vanilla `CrafterBlockEntity.containerData`: the slot states and the
/// powered flag, shared with every open crafter menu.
///
/// The crafter's slots read it when a player places an item, without the
/// container lock, so it lives outside the container mutex.
#[derive(Default)]
pub struct CrafterData {
    /// One bit per slot, set while the slot is disabled.
    disabled: AtomicU16,
    triggered: AtomicBool,
}

impl CrafterData {
    /// Vanilla `isSlotDisabled`: false for any index outside the grid.
    #[must_use]
    pub fn is_slot_disabled(&self, slot: usize) -> bool {
        slot < CRAFTER_SLOTS && self.disabled.load(Ordering::Relaxed) & (1 << slot) != 0
    }

    fn set_slot_disabled(&self, slot: usize, disabled: bool) {
        if disabled {
            self.disabled.fetch_or(1 << slot, Ordering::Relaxed);
        } else {
            self.disabled.fetch_and(!(1 << slot), Ordering::Relaxed);
        }
    }

    /// Vanilla `isTriggered`.
    #[must_use]
    pub fn is_triggered(&self) -> bool {
        self.triggered.load(Ordering::Relaxed)
    }

    /// Vanilla `setTriggered`.
    pub fn set_triggered(&self, triggered: bool) {
        self.triggered.store(triggered, Ordering::Relaxed);
    }

    /// Vanilla `ContainerData.get` for the crafter: 1 for a disabled slot,
    /// then the powered flag at [`CRAFTER_DATA_TRIGGERED`].
    #[must_use]
    pub fn get(&self, data_id: usize) -> i16 {
        if data_id == CRAFTER_DATA_TRIGGERED {
            i16::from(self.is_triggered())
        } else {
            i16::from(self.is_slot_disabled(data_id))
        }
    }
}

/// The lockable part of a crafter: its grid and crafting animation timer.
pub struct CrafterContainer {
    items: Vec<ItemStack>,
    crafting_ticks_remaining: i32,
    data: Arc<CrafterData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `CrafterBlockEntity`.
unsafe impl DowncastType for CrafterBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/crafter");
}

// SAFETY: This key is owned by Steel and uniquely identifies the independently
// lockable inventory data used by a crafter block entity.
unsafe impl DowncastType for CrafterContainer {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:container/crafter");
}

impl CrafterContainer {
    fn new(data: Arc<CrafterData>) -> Self {
        Self {
            items: vec![ItemStack::empty(); CRAFTER_SLOTS],
            crafting_ticks_remaining: 0,
            data,
        }
    }

    /// The slot states and powered flag shared with open menus.
    #[must_use]
    pub const fn data(&self) -> &Arc<CrafterData> {
        &self.data
    }

    /// Vanilla `slotCanBeDisabled`: only an empty slot of the grid.
    fn slot_can_be_disabled(&self, slot: usize) -> bool {
        self.items.get(slot).is_some_and(ItemStack::is_empty)
    }

    /// Vanilla `setSlotState` without its `setChanged`. Returns whether the
    /// slot could take the new state.
    #[must_use]
    pub fn set_slot_state(&self, slot: usize, enabled: bool) -> bool {
        if !self.slot_can_be_disabled(slot) {
            return false;
        }
        self.data.set_slot_disabled(slot, !enabled);
        true
    }

    /// Vanilla `smallerStackExist`: a later enabled slot that is empty or holds
    /// fewer of the same item, which a hopper fills first.
    fn smaller_stack_exists(
        &self,
        base_size: i32,
        base_item: &ItemStack,
        base_slot: usize,
    ) -> bool {
        (base_slot + 1..CRAFTER_SLOTS).any(|slot| {
            if self.data.is_slot_disabled(slot) {
                return false;
            }
            let stack = &self.items[slot];
            stack.is_empty()
                || (stack.count() < base_size
                    && ItemStack::is_same_item_same_components(stack, base_item))
        })
    }

    /// Vanilla `getRedstoneSignal`: the filled or disabled slots.
    #[must_use]
    pub fn redstone_signal(&self) -> i32 {
        let count = self
            .items
            .iter()
            .enumerate()
            .filter(|(slot, stack)| !stack.is_empty() || self.data.is_slot_disabled(*slot))
            .count();
        i32::try_from(count).unwrap_or(i32::MAX)
    }
}

impl Container for CrafterContainer {
    fn items(&self) -> &[ItemStack] {
        &self.items
    }

    fn items_mut(&mut self) -> &mut [ItemStack] {
        &mut self.items
    }

    fn get_container_size(&self) -> usize {
        CRAFTER_SLOTS
    }

    /// Vanilla `CrafterBlockEntity.setItem`: an item put into a disabled slot
    /// enables it first.
    fn set_item(&mut self, slot: usize, mut stack: ItemStack) {
        if slot >= CRAFTER_SLOTS {
            return;
        }
        if self.data.is_slot_disabled(slot) {
            self.data.set_slot_disabled(slot, false);
        }
        let max_stack_size = self.get_max_stack_size_for_item(&stack);
        if !stack.is_empty() && stack.count() > max_stack_size {
            stack.set_count(max_stack_size);
        }
        self.items[slot] = stack;
    }

    /// Vanilla `CrafterBlockEntity.canPlaceItem`: automation spreads items
    /// over the enabled slots, filling the smallest stack first.
    fn can_place_item(&self, slot: usize, _stack: &ItemStack) -> bool {
        if slot >= CRAFTER_SLOTS || self.data.is_slot_disabled(slot) {
            return false;
        }
        let existing = &self.items[slot];
        if existing.is_empty() {
            return true;
        }
        let count = existing.count();
        count < existing.max_stack_size() && !self.smaller_stack_exists(count, existing, slot)
    }

    fn set_changed(&mut self) {}
}

/// Vanilla `CrafterBlockEntity`.
pub struct CrafterBlockEntity {
    base: Arc<BlockEntityBase>,
    container: Arc<SyncMutex<CrafterContainer>>,
    container_ref: ContainerRef,
    data: Arc<CrafterData>,
    /// Vanilla `RandomizableContainerBlockEntity` loot table.
    loot: Arc<ContainerLoot>,
    /// Saved fields this block entity does not read, written back unchanged.
    preserved: SyncMutex<NbtCompound>,
    /// Vanilla `BaseContainerBlockEntity` custom name and lock.
    name: ContainerName,
}

impl CrafterBlockEntity {
    /// Creates a crafter block entity, powered when `state` is triggered
    /// (vanilla `CrafterBlock.newBlockEntity`).
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        let base = Arc::new(BlockEntityBase::new(
            &vanilla_block_entity_types::CRAFTER,
            level,
            pos,
            state,
        ));
        let data = Arc::new(CrafterData::default());
        data.set_triggered(
            state
                .try_get_value(&BlockStateProperties::TRIGGERED)
                .unwrap_or(false),
        );
        let container = Arc::new(SyncMutex::new(CrafterContainer::new(Arc::clone(&data))));
        let shared_container: SharedContainer =
            Arc::<SyncMutex<CrafterContainer>>::clone(&container);
        let loot = Arc::new(ContainerLoot::new());
        Self {
            container_ref: ContainerRef::owned_by_randomizable_block_entity(
                shared_container,
                Arc::clone(&base),
                Arc::clone(&loot),
            ),
            base,
            container,
            data,
            loot,
            preserved: SyncMutex::new(NbtCompound::new()),
            name: ContainerName::new(&translations::CONTAINER_CRAFTER),
        }
    }

    /// The slot states and powered flag shared with open menus.
    #[must_use]
    pub const fn data(&self) -> &Arc<CrafterData> {
        &self.data
    }

    /// Vanilla `setTriggered`.
    pub fn set_triggered(&self, triggered: bool) {
        self.data.set_triggered(triggered);
    }

    /// Vanilla `setSlotState`: enables or disables an empty slot.
    pub fn set_slot_state(&self, slot: usize, enabled: bool) {
        let changed = self.container.lock().set_slot_state(slot, enabled);
        if changed {
            self.set_changed();
        }
    }

    /// Vanilla `isSlotDisabled`.
    #[must_use]
    pub fn is_slot_disabled(&self, slot: usize) -> bool {
        self.data.is_slot_disabled(slot)
    }

    /// A copy of the grid, after rolling any pending loot table.
    #[must_use]
    pub fn items(&self) -> Vec<ItemStack> {
        self.container_ref.unpack_loot_table(None);
        self.container.lock().items.clone()
    }

    /// Returns a copy of the stack in `slot`.
    #[must_use]
    pub fn get_item(&self, slot: usize) -> ItemStack {
        self.container
            .lock()
            .items
            .get(slot)
            .cloned()
            .unwrap_or_else(ItemStack::empty)
    }

    /// Replaces the stack in `slot` and notifies the block entity.
    pub fn set_item(&self, slot: usize, stack: ItemStack) {
        self.container.lock().set_item(slot, stack);
        self.set_changed();
    }

    /// Vanilla `getRedstoneSignal`.
    #[must_use]
    pub fn redstone_signal(&self) -> i32 {
        self.container.lock().redstone_signal()
    }

    /// Vanilla `setCraftingTicksRemaining`.
    pub fn set_crafting_ticks_remaining(&self, ticks: i32) {
        self.container.lock().crafting_ticks_remaining = ticks;
    }

    /// The last part of a craft: every filled slot gives up one item.
    pub fn consume_one_of_each(&self) {
        {
            let mut container = self.container.lock();
            for stack in &mut container.items {
                if !stack.is_empty() {
                    stack.shrink(1);
                }
            }
        }
        self.set_changed();
    }

    /// Returns the saved custom name, if any.
    #[must_use]
    pub fn custom_name(&self) -> Option<TextComponent> {
        self.name.custom_name()
    }
}

/// Reads a crafter's grid through a locked menu or transfer guard.
#[must_use]
pub fn crafter_container<'a>(
    guard: &'a ContainerLockGuard,
    container: &ContainerRef,
) -> Option<&'a CrafterContainer> {
    guard.get_typed::<CrafterContainer>(container.container_id())
}

impl BlockEntity for CrafterBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    /// Vanilla `serverTick`: the crafting animation ends after its ticks.
    fn tick(&self, world: &Arc<World>) {
        let finished = {
            let mut container = self.container.lock();
            let remaining = container.crafting_ticks_remaining - 1;
            if remaining < 0 {
                return;
            }
            container.crafting_ticks_remaining = remaining;
            remaining == 0
        };
        if finished {
            let pos = self.get_block_pos();
            let state = world.get_block_state(pos);
            if state.get_block() == &vanilla_blocks::CRAFTER {
                world.set_block(
                    pos,
                    state.set_value(&BlockStateProperties::CRAFTING, false),
                    UpdateFlags::UPDATE_ALL,
                );
            }
        }
    }

    fn pre_remove_side_effects(&self, pos: BlockPos, _state: BlockStateId) {
        // Vanilla `Containers.dropContents` reads every slot, rolling the loot.
        self.container_ref.unpack_loot_table_for_removal();
        let items = {
            let mut container = self.container.lock();
            mem::replace(
                &mut container.items,
                vec![ItemStack::empty(); CRAFTER_SLOTS],
            )
        };
        let Some(world) = self.get_level() else {
            return;
        };
        for item in items {
            world.drop_item_stack(pos, item);
        }
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt_view: NbtCompoundView<'_, '_> = nbt.into();
        let mut container = self.container.lock();
        container.crafting_ticks_remaining = nbt_view.int(CRAFTING_TICKS_TAG).unwrap_or(0);
        container.items.fill(ItemStack::empty());

        if !self.loot.load(nbt_view)
            && let Some(items_list) = nbt_view.list("Items")
            && let Some(compounds) = items_list.compounds()
        {
            for compound in compounds {
                let Some(slot) = compound.byte("Slot") else {
                    continue;
                };
                let slot = slot as usize;
                if slot < CRAFTER_SLOTS
                    && let Some(item) = ItemStack::from_borrowed_compound(&compound)
                {
                    container.items[slot] = item;
                }
            }
        }

        self.data.disabled.store(0, Ordering::Relaxed);
        if let Some(disabled_slots) = nbt_view.int_array(DISABLED_SLOTS_TAG) {
            for slot in disabled_slots {
                if let Ok(slot) = usize::try_from(slot)
                    && container.slot_can_be_disabled(slot)
                {
                    self.data.set_slot_disabled(slot, true);
                }
            }
        }
        self.data
            .set_triggered(nbt_view.int(TRIGGERED_TAG).unwrap_or(0) == 1);
        drop(container);

        let mut preserved = nbt_view.to_owned();
        for key in [
            "Items",
            "components",
            LOOT_TABLE_TAG,
            LOOT_TABLE_SEED_TAG,
            CRAFTING_TICKS_TAG,
            DISABLED_SLOTS_TAG,
            TRIGGERED_TAG,
        ] {
            while preserved.remove(key).is_some() {}
        }
        self.name.load(nbt_view);
        ContainerName::remove_tags(&mut preserved);
        *self.preserved.lock() = preserved;
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.name.save(nbt);
        for (key, value) in self.preserved.lock().iter() {
            nbt.insert(key.to_string(), value.clone());
        }
        let container = self.container.lock();
        nbt.insert(CRAFTING_TICKS_TAG, container.crafting_ticks_remaining);
        if !self.loot.save(nbt) {
            let mut items: Vec<NbtCompound> = Vec::new();
            for (slot, item) in container.items.iter().enumerate() {
                if item.is_empty() {
                    continue;
                }
                if let NbtTag::Compound(mut item_nbt) = item.clone().to_nbt_tag() {
                    item_nbt.insert("Slot", slot as i8);
                    items.push(item_nbt);
                }
            }
            nbt.insert("Items", NbtList::Compound(items));
        }
        let disabled: Vec<i32> = (0..CRAFTER_SLOTS)
            .filter(|&slot| self.data.is_slot_disabled(slot))
            .map(|slot| slot as i32)
            .collect();
        nbt.insert(DISABLED_SLOTS_TAG, NbtTag::IntArray(disabled));
        nbt.insert(TRIGGERED_TAG, i32::from(self.data.is_triggered()));
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        None
    }

    fn container_name(&self) -> Option<&ContainerName> {
        Some(&self.name)
    }

    fn apply_implicit_components(&self, components: &mut ImplicitComponentGetter<'_>) {
        self.name.apply_implicit_components(components);
    }

    fn collect_implicit_components(&self, components: &mut DataComponentMap) {
        self.name.collect_implicit_components(components);
    }

    fn container_ref(&self) -> Option<ContainerRef> {
        Some(self.container_ref.clone())
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_items};

    use super::*;

    fn crafter() -> CrafterBlockEntity {
        init_vanilla_registry();
        CrafterBlockEntity::new(
            Weak::new(),
            BlockPos::new(0, 64, 0),
            vanilla_blocks::CRAFTER.default_state(),
        )
    }

    fn stack(count: i32) -> ItemStack {
        ItemStack::with_count(&vanilla_items::OAK_PLANKS, count)
    }

    #[test]
    fn only_empty_slots_can_be_disabled() {
        let crafter = crafter();
        crafter.set_item(0, stack(1));
        crafter.set_slot_state(0, false);
        crafter.set_slot_state(1, false);
        crafter.set_slot_state(9, false);
        assert!(!crafter.is_slot_disabled(0));
        assert!(crafter.is_slot_disabled(1));
        assert!(!crafter.is_slot_disabled(9));
        assert_eq!(crafter.redstone_signal(), 2);
    }

    #[test]
    fn placing_into_a_disabled_slot_enables_it() {
        let crafter = crafter();
        crafter.set_slot_state(4, false);
        crafter.set_item(4, stack(1));
        assert!(!crafter.is_slot_disabled(4));
    }

    #[test]
    fn automation_fills_the_smallest_enabled_stack_first() {
        let crafter = crafter();
        for slot in 0..CRAFTER_SLOTS {
            crafter.set_item(slot, stack(2));
        }
        crafter.set_item(5, stack(1));
        let container = crafter.container.lock();
        // Slot 5 holds fewer planks, so it fills before the earlier slots.
        assert!(!container.can_place_item(0, &stack(1)));
        assert!(container.can_place_item(5, &stack(1)));
        assert!(container.can_place_item(8, &stack(1)));
        drop(container);

        crafter.set_item(6, ItemStack::empty());
        crafter.set_slot_state(6, false);
        let container = crafter.container.lock();
        // A disabled empty slot neither takes items nor counts as smaller.
        assert!(!container.can_place_item(6, &stack(1)));
        assert!(!container.can_place_item(0, &stack(1)));
    }

    #[test]
    fn save_and_load_keep_slot_states_and_trigger() {
        let crafter = crafter();
        crafter.set_item(0, stack(3));
        crafter.set_slot_state(2, false);
        crafter.set_slot_state(7, false);
        crafter.set_triggered(true);
        crafter.set_crafting_ticks_remaining(4);
        let mut saved = NbtCompound::new();
        crafter.save_additional(&mut saved);
        assert_eq!(
            saved.int_array(DISABLED_SLOTS_TAG).map(<[i32]>::to_vec),
            Some(vec![2, 7])
        );
        assert_eq!(saved.int(TRIGGERED_TAG), Some(1));
        assert_eq!(saved.int(CRAFTING_TICKS_TAG), Some(4));

        let loaded = CrafterBlockEntity::new(
            Weak::new(),
            BlockPos::new(0, 64, 0),
            vanilla_blocks::CRAFTER.default_state(),
        );
        // A disabled slot that holds an item is not restored as disabled.
        saved.insert(DISABLED_SLOTS_TAG, NbtTag::IntArray(vec![0, 2, 7, 12]));
        if let Err(error) = loaded.load_with_owned_components(&saved) {
            panic!("saved NBT must load: {error}");
        }
        assert!(!loaded.is_slot_disabled(0));
        assert!(loaded.is_slot_disabled(2));
        assert!(loaded.is_slot_disabled(7));
        assert!(loaded.data().is_triggered());
        assert_eq!(loaded.get_item(0), stack(3));
        assert_eq!(loaded.container.lock().crafting_ticks_remaining, 4);
    }
}
