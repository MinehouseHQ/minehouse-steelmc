use crate::player::player_inventory::Prediction;
use std::sync::Arc;

use steel_registry::REGISTRY;
use steel_registry::item_stack::ItemStack;
use steel_registry::recipe::{CraftingRecipe, RecipeMatches as _, UntypedRecipeRef};
use steel_utils::locks::Shared;

use crate::behavior::on_crafted_by;
use crate::inventory::container::{CraftingContainer, ResultContainer};
use crate::inventory::place_recipe::{
    CraftingMenuAccess, PlacementGrid, PostPlaceAction, RecipePlacement, place_recipe,
};
use crate::inventory::recipe_crafting_holder::may_use_recipe;
use crate::inventory::slots::Slot;
use crate::inventory::stacked_contents::StackedItemContents;
use crate::{
    inventory::{
        container::Container,
        lock::{ContainerId, ContainerLockGuard, ContainerRef},
        recipe_manager,
        slots::result_handler::{CraftedStat, ResultHandler},
    },
    player::Player,
};

/// A Recipe Handler for Crafting Recipes
#[derive(Clone)]
pub struct CraftingHandler {
    crafting_container: Shared<CraftingContainer>,
    result_container: Shared<ResultContainer>,
    grid_size: usize,
}

impl CraftingHandler {
    /// Creates a new Crafting Recipe Handler
    pub const fn new(
        crafting_container: Shared<CraftingContainer>,
        result_container: Shared<ResultContainer>,
        grid_size: usize,
    ) -> Self {
        Self {
            crafting_container,
            result_container,
            grid_size,
        }
    }

    /// The `ContainerId` of the crafting container
    #[must_use]
    pub fn crafting_id(&self) -> ContainerId {
        ContainerId::from_arc(&self.crafting_container)
    }

    /// A shared handle to the crafting container.
    #[must_use]
    pub fn crafting_container(&self) -> Shared<CraftingContainer> {
        Arc::clone(&self.crafting_container)
    }

    /// The `ContainerId` of the result container
    #[must_use]
    pub fn result_id(&self) -> ContainerId {
        ContainerId::from_arc(&self.result_container)
    }

    /// Vanilla `ResultSlot.checkTakeAchievements`' recipe award, with the grid as the
    /// used items. It reaches the player, so it runs with the containers unlocked.
    fn award_used_recipe(&self, guard: &mut ContainerLockGuard, player: &Player) {
        let items = guard
            .get_typed::<CraftingContainer>(self.crafting_id())
            .expect("crafting container not locked")
            .items()
            .to_vec();
        let recipe = guard
            .get_typed_mut::<ResultContainer>(self.result_id())
            .expect("result container not locked")
            .take_recipe_to_award();
        if let Some(recipe) = recipe {
            guard.run_unlocked(|| player.award_used_recipe(recipe, &items));
        }
    }
}

impl CraftingHandler {
    /// Vanilla `CraftingMenu.slotChangedCraftingGrid` with its recipe hint: `hint` is
    /// tried first, then every crafting recipe in order.
    ///
    /// # Panics
    ///
    /// Panics if `guard` does not hold the crafting and result containers.
    pub fn update_result_with_hint(
        &self,
        guard: &mut ContainerLockGuard,
        player: &Player,
        hint: Option<UntypedRecipeRef>,
    ) {
        let crafting = guard
            .get_typed::<CraftingContainer>(self.crafting_id())
            .expect("crafting container not locked");
        let positioned = recipe_manager::positioned_input(crafting, &player.get_world());

        let hinted = hint.and_then(|hint| {
            hint.downcast_data::<CraftingRecipe>()
                .filter(|data| data.matches(&positioned.input))
                .map(|data| (hint, data))
        });
        let found = hinted.or_else(|| {
            let recipe = recipe_manager::find_recipe(&positioned.input)?;
            Some((REGISTRY.recipes.by_key(recipe.key())?, recipe.data()))
        });

        let mut result_stack = ItemStack::empty();
        let mut recipe_used = None;
        if let Some((untyped, data)) = found
            && may_use_recipe(player, untyped)
        {
            recipe_used = Some(untyped.key().clone());
            result_stack = recipe_manager::assemble_crafting(data, &positioned.input);
        }

        let result_container = guard
            .get_typed_mut::<ResultContainer>(self.result_id())
            .expect("result container not locked");
        // Vanilla keeps the last used recipe when nothing matches or it may not be used.
        if recipe_used.is_some() {
            result_container.set_recipe_used(recipe_used);
        }
        result_container.set_item(0, result_stack);
        result_container.set_changed();
    }

    /// Vanilla `AbstractCraftingMenu.handlePlacement` over this grid, whose slots follow
    /// the result slot. `None` for a recipe that is not a crafting recipe.
    pub fn handle_placement(
        &self,
        slots: &[Box<dyn Slot>],
        guard: &mut ContainerLockGuard,
        player: &Player,
        placement: RecipePlacement,
    ) -> Option<PostPlaceAction> {
        placement.recipe.downcast_data::<CraftingRecipe>()?;
        let input_slots: Vec<usize> = (1..=self.grid_size * self.grid_size).collect();
        let grid = PlacementGrid {
            width: self.grid_size,
            height: self.grid_size,
            input_slots: &input_slots,
            slots_to_clear: &input_slots,
        };
        Some(place_recipe(guard, player, self, slots, grid, placement))
    }
}

/// Vanilla `AbstractCraftingMenu`'s `CraftingMenuAccess`.
impl CraftingMenuAccess for CraftingHandler {
    fn fill_craft_slots_stacked_contents(
        &self,
        guard: &ContainerLockGuard,
        contents: &mut StackedItemContents,
    ) {
        if let Some(crafting) = guard.get_typed::<CraftingContainer>(self.crafting_id()) {
            for stack in crafting.items() {
                contents.account_simple_stack(stack);
            }
        }
    }

    fn clear_crafting_content(&self, guard: &mut ContainerLockGuard) {
        for id in [self.result_id(), self.crafting_id()] {
            if let Some(container) = guard.get_mut(id) {
                container.clear_content();
            }
        }
    }

    fn recipe_matches(
        &self,
        guard: &ContainerLockGuard,
        player: &Player,
        recipe: UntypedRecipeRef,
    ) -> bool {
        let Some(crafting) = guard.get_typed::<CraftingContainer>(self.crafting_id()) else {
            return false;
        };
        let positioned = recipe_manager::positioned_input(crafting, &player.get_world());
        recipe
            .downcast_data::<CraftingRecipe>()
            .is_some_and(|data| data.matches(&positioned.input))
    }
}

impl ResultHandler for CraftingHandler {
    /// Vanilla `ResultSlot` counts the crafted items taken.
    fn crafted_stat(&self) -> CraftedStat {
        CraftedStat::Crafting
    }

    fn result_container(&self) -> ContainerRef {
        ContainerRef::from(Arc::clone(&self.result_container))
    }

    fn dependencies(&self) -> Vec<ContainerRef> {
        vec![ContainerRef::from(Arc::clone(&self.crafting_container))]
    }

    /// Vanilla `CraftingMenu.slotChangedCraftingGrid`.
    fn update_result(&self, guard: &mut ContainerLockGuard, player: &Player) {
        self.update_result_with_hint(guard, player, None);
    }

    fn on_result_taken(
        &self,
        guard: &mut ContainerLockGuard,
        player: &Player,
    ) -> Option<ItemStack> {
        self.award_used_recipe(guard, player);
        let mut remainder_overflow: Vec<ItemStack> = Vec::new();

        let remainders_and_positioned = {
            let crafting = guard
                .get_typed::<CraftingContainer>(self.crafting_id())
                .expect("crafting container not locked");
            recipe_manager::get_remaining_items(crafting, &player.get_world())
        };

        let Some((remainders, positioned)) = remainders_and_positioned else {
            guard
                .get_typed_mut::<ResultContainer>(self.result_id())
                .expect("result container not locked")
                .set_item(0, ItemStack::empty());
            return None;
        };

        {
            let crafting = guard
                .get_typed_mut::<CraftingContainer>(self.crafting_id())
                .expect("crafting container not locked");

            let input = &positioned.input;

            for y in 0..input.height {
                for x in 0..input.width {
                    let grid_slot = positioned.to_grid_slot(x, y, self.grid_size);
                    let remainder_idx = x + y * input.width;
                    let replacement = if remainder_idx < remainders.len() {
                        remainders[remainder_idx].clone()
                    } else {
                        ItemStack::empty()
                    };

                    {
                        let item = crafting.get_item_mut(grid_slot);
                        if !item.is_empty() {
                            item.shrink_one();
                        }
                    }

                    if !replacement.is_empty() {
                        let current_item = crafting.get_item(grid_slot).clone();

                        if current_item.is_empty() {
                            crafting.set_item(grid_slot, replacement);
                        } else if ItemStack::is_same_item_same_components(
                            &current_item,
                            &replacement,
                        ) {
                            crafting.get_item_mut(grid_slot).grow(replacement.count());
                        } else {
                            remainder_overflow.push(replacement);
                        }
                    }
                }
            }

            crafting.set_changed();
        }

        self.update_result(guard, player);

        for remainder in remainder_overflow {
            player.add_item_or_drop_with_guard(guard, remainder, Prediction::Predicted);
        }

        None
    }

    /// Vanilla `ResultSlot.checkTakeAchievements` runs the item's `onCraftedBy`, which gives a
    /// scaled map its own new map.
    fn on_crafted_by(&self, stack: &mut ItemStack, player: &Player) {
        on_crafted_by(stack, player);
    }

    fn is_result_valid(&self, guard: &ContainerLockGuard, player: &Player) -> bool {
        let Some(result) = guard.get(self.result_id()) else {
            return false;
        };
        let result_item = result.get_item(0);
        if result_item.is_empty() {
            return false;
        }

        let Some(crafting) = guard.get_typed::<CraftingContainer>(self.crafting_id()) else {
            return false;
        };

        let Some(assembled) = recipe_manager::assemble_for_container(crafting, &player.get_world())
        else {
            return false;
        };

        ItemStack::matches(result_item, &assembled)
    }
}
