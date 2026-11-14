//! Vanilla `CrafterMenu`.
//!
//! Slot layout (46 total):
//! - Slots 0-8: the crafter's 3x3 grid
//! - Slots 9-35: Main inventory (27)
//! - Slots 36-44: Hotbar (9)
//! - Slot 45: the recipe result, shown but never taken
//!
//! Data slots 0-8 hold the slot states and data slot 9 the powered flag.

use std::array::from_fn;
use std::sync::Arc;

use steel_registry::vanilla_menu_types;
use steel_utils::{DowncastType, DowncastTypeKey, locks::Shared};

use crate::block_entity::entities::{
    CRAFTER_GRID_SIZE, CRAFTER_NUM_DATA, CrafterContainer, CrafterData,
};
use crate::inventory::container::ResultContainer;
use crate::inventory::prelude::*;
use crate::inventory::recipe_manager::{assemble_recipe, find_recipe, positioned_grid_input};
use crate::inventory::slots::{CrafterSlot, NonInteractiveResultSlot};
use crate::player::player_inventory::PlayerInventory;

/// Builds Vanilla's `CrafterMenu` over a crafter's container and shared data.
#[must_use]
pub fn crafter(
    inventory: Shared<PlayerInventory>,
    container_id: u8,
    container: ContainerRef,
    data: Arc<CrafterData>,
) -> Menu {
    let result_container = ResultContainer::new().into_shared();
    let mut builder = MenuBuilder::new(&vanilla_menu_types::CRAFTER_3X3, container_id);

    let slot_data = Arc::clone(&data);
    let grid = builder.section_with(
        container.clone(),
        CRAFTER_GRID_SIZE * CRAFTER_GRID_SIZE,
        SectionKind::custom(move |container, index| {
            Box::new(CrafterSlot::new(
                container.clone(),
                index,
                Arc::clone(&slot_data),
            ))
        }),
    );
    let player = builder.player_inventory(&inventory);
    let result_ref = ContainerRef::from(Arc::clone(&result_container));
    let result_id = result_ref.container_id();
    builder.section_with(
        result_ref,
        1,
        SectionKind::custom(|container, index| {
            Box::new(NonInteractiveResultSlot::new(container.clone(), index))
        }),
    );
    let data_slots = from_fn(|_| builder.data_slot(0));

    builder.route(grid, player.all(), FillDirection::Backward);
    builder.route(player.all(), grid, FillDirection::Forward);

    builder.build(CrafterKind {
        container,
        data,
        result_id,
        data_slots,
        last_grid: Vec::new(),
    })
}

/// State of Vanilla's `CrafterMenu`.
pub struct CrafterKind {
    container: ContainerRef,
    data: Arc<CrafterData>,
    result_id: ContainerId,
    data_slots: [DataSlot; CRAFTER_NUM_DATA],
    /// The grid the shown result was computed from.
    last_grid: Vec<ItemStack>,
}

// SAFETY: This Steel-owned key uniquely identifies crafter menu behavior.
unsafe impl DowncastType for CrafterKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:menu/crafter");
}

impl CrafterKind {
    /// Vanilla `CrafterBlockEntity.setSlotState` through the open menu.
    pub(crate) fn set_slot_state(
        &self,
        guard: &mut ContainerLockGuard,
        slot: usize,
        enabled: bool,
    ) {
        let id = self.container.container_id();
        let changed = guard
            .get_typed::<CrafterContainer>(id)
            .is_some_and(|container| container.set_slot_state(slot, enabled));
        if changed {
            guard.set_changed(id);
        }
    }

    /// Vanilla `addDataSlots(containerData)`: copies the slot states and the
    /// powered flag into the menu's data slots.
    fn update_data(&self, behavior: &mut MenuBehavior) {
        for (data_id, slot) in self.data_slots.into_iter().enumerate() {
            slot.set(behavior, self.data.get(data_id));
        }
    }

    /// Vanilla `refreshRecipeResult`, run whenever the grid changed since the
    /// shown result was computed.
    fn refresh_result(&mut self, guard: &mut ContainerLockGuard, player: &Player) {
        let Some(grid) = guard
            .get_typed::<CrafterContainer>(self.container.container_id())
            .map(|container| container.items().to_vec())
        else {
            return;
        };
        if grid == self.last_grid {
            return;
        }
        let world = player.get_world();
        let positioned =
            positioned_grid_input(CRAFTER_GRID_SIZE, CRAFTER_GRID_SIZE, grid.clone(), &world);
        let result = find_recipe(&positioned.input).map_or_else(ItemStack::empty, |recipe| {
            assemble_recipe(recipe, &positioned.input)
        });
        self.last_grid = grid;
        guard.set_item(self.result_id, 0, result);
    }
}

impl MenuKind for CrafterKind {
    fn still_valid(&self, _behavior: &MenuBehavior, player: &Player) -> bool {
        self.container.still_valid(player)
    }

    fn on_open(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        player: &Player,
    ) {
        self.update_data(behavior);
        self.refresh_result(guard, player);
    }

    /// Hoppers and other crafters change the grid while the menu is open, so
    /// the result is checked every tick like vanilla's slot listener does.
    fn on_tick(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        player: &Player,
    ) {
        self.update_data(behavior);
        self.refresh_result(guard, player);
    }

    fn slots_changed(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        player: &Player,
    ) {
        self.refresh_result(guard, player);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_items};
    use steel_utils::BlockPos;
    use steel_utils::locks::IntoShared as _;

    use super::*;
    use crate::block_entity::BlockEntity as _;
    use crate::block_entity::entities::CrafterBlockEntity;

    fn crafter_entity() -> CrafterBlockEntity {
        init_vanilla_registry();
        CrafterBlockEntity::new(
            Weak::new(),
            BlockPos::new(0, 64, 0),
            vanilla_blocks::CRAFTER.default_state(),
        )
    }

    fn menu(entity: &CrafterBlockEntity) -> Menu {
        let Some(container) = entity.container_ref() else {
            panic!("a crafter always has a container");
        };
        crafter(
            PlayerInventory::new().into_shared(),
            1,
            container,
            Arc::clone(entity.data()),
        )
    }

    #[test]
    fn layout_matches_vanilla() {
        let entity = crafter_entity();
        let menu = menu(&entity);
        let slots = menu.behavior().slots();
        assert_eq!(slots.len(), 9 + 36 + 1);
        assert!(slots[45].is_fake());
        assert!(!slots[45].may_place(&ItemStack::new(&vanilla_items::STONE)));
    }

    #[test]
    fn disabled_slots_reject_placement() {
        let entity = crafter_entity();
        let menu = menu(&entity);
        let planks = ItemStack::new(&vanilla_items::OAK_PLANKS);
        assert!(menu.behavior().slots()[3].may_place(&planks));
        entity.set_slot_state(3, false);
        assert!(!menu.behavior().slots()[3].may_place(&planks));
        assert!(menu.behavior().slots()[4].may_place(&planks));
    }

    #[test]
    fn slot_state_through_the_menu_needs_an_empty_slot() {
        let entity = crafter_entity();
        entity.set_item(0, ItemStack::new(&vanilla_items::OAK_PLANKS));
        let mut menu = menu(&entity);
        menu.set_crafter_slot_state(0, false);
        menu.set_crafter_slot_state(1, false);
        assert!(!entity.is_slot_disabled(0));
        assert!(entity.is_slot_disabled(1));
        menu.set_crafter_slot_state(1, true);
        assert!(!entity.is_slot_disabled(1));
    }
}
