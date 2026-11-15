//! Vanilla `ServerPlaceRecipe` and `PlaceRecipeHelper`: moving a recipe's ingredients
//! from the player's inventory onto a menu's grid when the recipe book asks.

use steel_registry::item_stack::ItemStack;
use steel_registry::recipe::{CraftingRecipe, IngredientItem, PlacementInfo, UntypedRecipeRef};

use crate::inventory::container::Container as _;
use crate::inventory::lock::{ContainerId, ContainerLockGuard};
use crate::inventory::slots::Slot;
use crate::inventory::stacked_contents::StackedItemContents;
use crate::player::Player;
use crate::player::player_inventory::{PlayerInventory, Prediction};

/// Vanilla `RecipeBookMenu.PostPlaceAction`: what the client is told after a placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostPlaceAction {
    /// Nothing; the grid changes reach the client as slot updates.
    Nothing,
    /// The ingredients were missing: show the recipe as a ghost on the grid.
    PlaceGhostRecipe,
}

/// What `ServerboundPlaceRecipePacket` asks to place, with vanilla
/// `handlePlacement`'s flags.
#[derive(Debug, Clone, Copy)]
pub struct RecipePlacement {
    /// The recipe to place.
    pub recipe: UntypedRecipeRef,
    /// Place as many as the ingredients allow, rather than one more.
    pub use_max_items: bool,
    /// Vanilla `allowDroppingItemsToClear`, true in creative: the grid may be cleared
    /// even when its items do not fit back in the inventory.
    pub allow_dropping_items_to_clear: bool,
}

/// Vanilla `ServerPlaceRecipe.CraftingMenuAccess`: the menu side of a placement.
pub trait CraftingMenuAccess {
    /// Counts the items already on the grid as available.
    fn fill_craft_slots_stacked_contents(
        &self,
        guard: &ContainerLockGuard,
        contents: &mut StackedItemContents,
    );

    /// Empties the grid and whatever the menu clears with it.
    fn clear_crafting_content(&self, guard: &mut ContainerLockGuard);

    /// Whether the items on the grid already make `recipe`.
    fn recipe_matches(
        &self,
        guard: &ContainerLockGuard,
        player: &Player,
        recipe: UntypedRecipeRef,
    ) -> bool;
}

/// The grid of a menu a recipe is placed on: its size and its slots, as indices into
/// the menu's slots.
#[derive(Debug, Clone, Copy)]
pub struct PlacementGrid<'a> {
    /// Grid width.
    pub width: usize,
    /// Grid height.
    pub height: usize,
    /// The grid slots, row by row.
    pub input_slots: &'a [usize],
    /// The slots emptied before placing, which may include more than the grid.
    pub slots_to_clear: &'a [usize],
}

/// The recipe's placement, or `None` when it has none to place.
#[must_use]
pub fn placement_of(recipe: UntypedRecipeRef) -> Option<&'static PlacementInfo> {
    recipe
        .data()
        .properties()
        .and_then(|properties| properties.placement.as_ref())
        .filter(|placement| !placement.is_impossible_to_place())
}

/// Vanilla `ServerPlaceRecipe.placeRecipe`.
pub fn place_recipe(
    guard: &mut ContainerLockGuard,
    player: &Player,
    menu: &dyn CraftingMenuAccess,
    slots: &[Box<dyn Slot>],
    grid: PlacementGrid<'_>,
    placement: RecipePlacement,
) -> PostPlaceAction {
    let RecipePlacement {
        recipe,
        use_max_items,
        allow_dropping_items_to_clear,
    } = placement;
    let Some(placement) = placement_of(recipe) else {
        return PostPlaceAction::Nothing;
    };
    let server_place = ServerPlaceRecipe {
        menu,
        slots,
        grid,
        inventory: ContainerId::from_arc(&player.inventory),
        use_max_items,
    };
    if !allow_dropping_items_to_clear && !server_place.test_clear_grid(guard) {
        return PostPlaceAction::Nothing;
    }
    let mut available_items = StackedItemContents::default();
    if let Some(inventory) = guard.get_typed::<PlayerInventory>(server_place.inventory) {
        inventory.fill_stacked_contents(&mut available_items);
    }
    menu.fill_craft_slots_stacked_contents(guard, &mut available_items);
    server_place.try_place_recipe(guard, player, recipe, placement, &mut available_items)
}

struct ServerPlaceRecipe<'a> {
    menu: &'a dyn CraftingMenuAccess,
    slots: &'a [Box<dyn Slot>],
    grid: PlacementGrid<'a>,
    inventory: ContainerId,
    use_max_items: bool,
}

impl ServerPlaceRecipe<'_> {
    fn slot(&self, index: usize) -> &dyn Slot {
        self.slots[index].as_ref()
    }

    fn try_place_recipe(
        &self,
        guard: &mut ContainerLockGuard,
        player: &Player,
        recipe: UntypedRecipeRef,
        placement: &PlacementInfo,
        available_items: &mut StackedItemContents,
    ) -> PostPlaceAction {
        let action = if available_items.can_craft(placement, 1, None) {
            self.place(guard, player, recipe, placement, available_items);
            PostPlaceAction::Nothing
        } else {
            self.clear_grid(guard, player);
            PostPlaceAction::PlaceGhostRecipe
        };
        guard.set_changed(self.inventory);
        action
    }

    /// Vanilla `clearGrid`: every slot to clear goes back to the inventory, or drops.
    fn clear_grid(&self, guard: &mut ContainerLockGuard, player: &Player) {
        for &index in self.grid.slots_to_clear {
            let stack = self.slot(index).get_item(guard).clone();
            player.add_item_or_drop_with_guard(guard, stack, Prediction::ServerOnly);
            self.slot(index).set_item(guard, ItemStack::empty());
        }
        self.menu.clear_crafting_content(guard);
    }

    /// Vanilla `placeRecipe(RecipeHolder, StackedItemContents)`.
    fn place(
        &self,
        guard: &mut ContainerLockGuard,
        player: &Player,
        recipe: UntypedRecipeRef,
        placement: &PlacementInfo,
        available_items: &mut StackedItemContents,
    ) {
        let recipe_matches_placed = self.menu.recipe_matches(guard, player, recipe);
        let biggest_craftable_stack =
            available_items.biggest_craftable_stack(placement, i32::MAX, None);
        if recipe_matches_placed
            && self.grid.input_slots.iter().any(|&index| {
                let stack = self.slot(index).get_item(guard);
                !stack.is_empty()
                    && biggest_craftable_stack.min(stack.max_stack_size()) < stack.count() + 1
            })
        {
            return;
        }

        let amount_to_craft =
            self.calculate_amount_to_craft(guard, biggest_craftable_stack, recipe_matches_placed);
        let Some(items_used_per_ingredient) =
            items_used(available_items, placement, amount_to_craft)
        else {
            return;
        };
        let adjusted_amount_to_craft =
            clamp_to_max_stack_size(amount_to_craft, &items_used_per_ingredient);
        let items_used_per_ingredient = if adjusted_amount_to_craft == amount_to_craft {
            items_used_per_ingredient
        } else {
            let Some(items) = items_used(available_items, placement, adjusted_amount_to_craft)
            else {
                return;
            };
            items
        };

        self.clear_grid(guard, player);
        let (recipe_width, recipe_height) = recipe_size(recipe, &self.grid);
        place_on_grid(
            self.grid.width,
            self.grid.height,
            recipe_width,
            recipe_height,
            &placement.slots_to_ingredient,
            &mut |ingredient_index, grid_index| {
                let Some(ingredient_index) = ingredient_index else {
                    return;
                };
                let (Some(&target), Some(&item)) = (
                    self.grid.input_slots.get(grid_index),
                    items_used_per_ingredient.get(ingredient_index),
                ) else {
                    return;
                };
                let mut remaining_count = adjusted_amount_to_craft;
                while remaining_count > 0 {
                    match self.move_item_to_grid(guard, target, item, remaining_count) {
                        Some(remaining) => remaining_count = remaining,
                        None => return,
                    }
                }
            },
        );
    }

    /// Vanilla `calculateAmountToCraft`.
    fn calculate_amount_to_craft(
        &self,
        guard: &ContainerLockGuard,
        biggest_craftable_stack: i32,
        recipe_matches_placed: bool,
    ) -> i32 {
        if self.use_max_items {
            return biggest_craftable_stack;
        }
        if !recipe_matches_placed {
            return 1;
        }
        self.grid
            .input_slots
            .iter()
            .map(|&index| self.slot(index).get_item(guard))
            .filter(|stack| !stack.is_empty())
            .map(ItemStack::count)
            .min()
            .map_or(i32::MAX, |smallest_stack_size| smallest_stack_size + 1)
    }

    /// Vanilla `moveItemToGrid`: moves up to `count` of `item` from the inventory onto
    /// `target`, returning how many are still to move, or `None` when the inventory
    /// has none left.
    fn move_item_to_grid(
        &self,
        guard: &mut ContainerLockGuard,
        target: usize,
        item: IngredientItem,
        count: i32,
    ) -> Option<i32> {
        let item_in_target_slot = self.slot(target).get_item(guard).clone();
        let inventory = guard.get_typed_mut::<PlayerInventory>(self.inventory)?;
        let inventory_slot =
            inventory.find_slot_matching_crafting_ingredient(item, &item_in_target_slot)?;
        let taken_stack = if count < inventory.get_item(inventory_slot).count() {
            inventory.remove_item(inventory_slot, count)
        } else {
            inventory.remove_item_no_update(inventory_slot)
        };
        let taken_count = taken_stack.count();
        if item_in_target_slot.is_empty() {
            self.slot(target).set_item(guard, taken_stack);
        } else {
            self.slot(target).get_item_mut(guard).grow(taken_count);
        }
        Some(count - taken_count)
    }

    /// Vanilla `testClearGrid`: whether everything on the grid fits back in the
    /// inventory.
    fn test_clear_grid(&self, guard: &ContainerLockGuard) -> bool {
        let Some(inventory) = guard.get_typed::<PlayerInventory>(self.inventory) else {
            return false;
        };
        let free_slots_in_inventory = inventory
            .get_items()
            .iter()
            .filter(|stack| stack.is_empty())
            .count();
        let mut free_slots: Vec<ItemStack> = Vec::new();
        for &index in self.grid.input_slots {
            let mut stack = self.slot(index).get_item(guard).clone();
            if stack.is_empty() {
                continue;
            }
            if inventory.get_slot_with_remaining_space(&stack).is_some() {
                continue;
            }
            if free_slots.len() > free_slots_in_inventory {
                return false;
            }
            if let Some(listed) = free_slots.iter_mut().find(|listed| {
                ItemStack::is_same_item(listed, &stack)
                    && listed.count() != listed.max_stack_size()
                    && listed.count() + stack.count() <= listed.max_stack_size()
            }) {
                listed.grow(stack.count());
                stack.set_count(0);
            }
            if !stack.is_empty() {
                if free_slots.len() >= free_slots_in_inventory {
                    return false;
                }
                free_slots.push(stack);
            }
        }
        true
    }
}

/// The items each ingredient takes for `amount` crafts, or `None` when they are not
/// all there.
fn items_used(
    available_items: &mut StackedItemContents,
    placement: &PlacementInfo,
    amount: i32,
) -> Option<Vec<IngredientItem>> {
    let mut items = Vec::with_capacity(placement.ingredients.len());
    available_items
        .can_craft(placement, amount, Some(&mut |item| items.push(item)))
        .then_some(items)
}

/// Vanilla `clampToMaxStackSize`: no more than the smallest default stack size of the
/// items used.
fn clamp_to_max_stack_size(value: i32, items: &[IngredientItem]) -> i32 {
    items.iter().fold(value, |value, item| {
        value.min(ItemStack::new(item.item).max_stack_size())
    })
}

/// The shape `PlaceRecipeHelper.placeRecipe` lays out: a shaped recipe's own size, or
/// the whole grid.
fn recipe_size(recipe: UntypedRecipeRef, grid: &PlacementGrid<'_>) -> (usize, usize) {
    match recipe.downcast_data::<CraftingRecipe>() {
        Some(CraftingRecipe::Shaped(shaped)) => (shaped.width, shaped.height),
        _ => (grid.width, grid.height),
    }
}

/// Vanilla `PlaceRecipeHelper.placeRecipe`: walks the grid row by row, centering a
/// recipe narrower or shorter than half the grid, and hands each recipe slot's entry
/// to `output` with the grid index it lands on.
///
/// Vanilla centers with float halves floored; for grid sizes that is the same as the
/// integer arithmetic here.
pub fn place_on_grid<T: Copy>(
    grid_width: usize,
    grid_height: usize,
    recipe_width: usize,
    recipe_height: usize,
    entries: &[T],
    output: &mut dyn FnMut(T, usize),
) {
    let center_start = |grid: usize, recipe: usize| {
        (2 * recipe < grid).then(|| (grid.cast_signed() - recipe.cast_signed()).div_euclid(2))
    };
    let mut entries = entries.iter().copied();
    let mut grid_index = 0;
    let mut grid_y = 0;
    while grid_y < grid_height {
        if let Some(start) = center_start(grid_height, recipe_height)
            && start > grid_y.cast_signed()
        {
            grid_index += grid_width;
            grid_y += 1;
        }
        for grid_x in 0..grid_width {
            if entries.len() == 0 {
                return;
            }
            let x = grid_x.cast_signed();
            let width = recipe_width.cast_signed();
            let (total_recipe_width_in_grid, add_ingredient_to_slot) =
                match center_start(grid_width, recipe_width) {
                    Some(start) => (start + width, start <= x && x < start + width),
                    None => (width, x < width),
                };
            if add_ingredient_to_slot {
                if let Some(entry) = entries.next() {
                    output(entry, grid_index);
                }
            } else if total_recipe_width_in_grid == x {
                grid_index += grid_width - grid_x;
                break;
            }
            grid_index += 1;
        }
        grid_y += 1;
    }
}

#[cfg(test)]
mod tests;
