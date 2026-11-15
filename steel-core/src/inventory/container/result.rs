use core::slice;
use std::mem;

use steel_registry::item_stack::ItemStack;
use steel_registry::recipe::UntypedRecipeRef;
use steel_utils::{DowncastType, DowncastTypeKey, Identifier};

use crate::inventory::container::Container;
use crate::inventory::recipe_crafting_holder::take_recipe_to_award;

/// A simple container for holding a single crafting result.
pub struct ResultContainer {
    result: ItemStack,
    /// Vanilla `ResultContainer.recipeUsed`.
    recipe_used: Option<Identifier>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ResultContainer`.
unsafe impl DowncastType for ResultContainer {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:container/result");
}

impl ResultContainer {
    /// Creates a new empty result container.
    #[must_use]
    pub fn new() -> Self {
        Self {
            result: ItemStack::empty(),
            recipe_used: None,
        }
    }

    /// Vanilla `ResultContainer.setRecipeUsed`.
    pub fn set_recipe_used(&mut self, recipe: Option<Identifier>) {
        self.recipe_used = recipe;
    }

    /// Vanilla `ResultContainer.getRecipeUsed`.
    #[must_use]
    pub const fn recipe_used(&self) -> Option<&Identifier> {
        self.recipe_used.as_ref()
    }

    /// The container half of vanilla `RecipeCraftingHolder.awardUsedRecipes`; see
    /// [`take_recipe_to_award`].
    pub fn take_recipe_to_award(&mut self) -> Option<UntypedRecipeRef> {
        take_recipe_to_award(&mut self.recipe_used)
    }
}

impl Default for ResultContainer {
    fn default() -> Self {
        Self::new()
    }
}

impl Container for ResultContainer {
    fn get_container_size(&self) -> usize {
        1
    }

    fn get_item(&self, _slot: usize) -> &ItemStack {
        &self.result
    }

    fn get_item_mut(&mut self, _slot: usize) -> &mut ItemStack {
        &mut self.result
    }

    fn set_item(&mut self, _slot: usize, stack: ItemStack) {
        self.result = stack;
    }

    /// Removes items from the result container.
    ///
    /// Unlike normal containers, this **always takes the entire stack**
    /// regardless of the `count` parameter. This matches Java's
    /// `ResultContainer.removeItem()` behavior which uses `takeItem()`.
    ///
    /// This ensures that right-clicking on a crafting result takes the
    /// full crafted item, not half of it.
    fn remove_item(&mut self, _slot: usize, _count: i32) -> ItemStack {
        mem::take(&mut self.result)
    }

    fn set_changed(&mut self) {
        // Result container doesn't track dirty state.
    }

    fn items(&self) -> &[ItemStack] {
        slice::from_ref(&self.result)
    }

    fn items_mut(&mut self) -> &mut [ItemStack] {
        slice::from_mut(&mut self.result)
    }
}
