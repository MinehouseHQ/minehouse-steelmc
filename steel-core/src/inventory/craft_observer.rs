//! A hook for content crates into crafting, smithing and the cartography table.

use std::sync::OnceLock;

use steel_registry::{
    item_stack::ItemStack,
    recipe::{CraftingInput, CraftingRecipe, SmithingRecipe, SmithingRecipeInput},
};

/// Lets a content crate turn recipes down and add its own cartography table results. With
/// no observer, and by default, everything is vanilla.
pub trait CraftObserver: Send + Sync {
    /// Whether `recipe`, which matches `input`, must not be crafted. The next matching
    /// recipe is tried, as if this one did not match.
    fn blocks_crafting(&self, _recipe: &CraftingRecipe, _input: &CraftingInput) -> bool {
        false
    }

    /// Whether `recipe`, which matches `input`, must not be made on the smithing table.
    fn blocks_smithing(&self, _recipe: &SmithingRecipe, _input: &SmithingRecipeInput) -> bool {
        false
    }

    /// Whether `stack` may go into the additional slot of the cartography table, besides
    /// what vanilla lets in.
    fn cartography_additional(&self, _stack: &ItemStack) -> bool {
        false
    }

    /// The result of the cartography table for `map` and `additional`, or `None` for the
    /// vanilla one. An empty stack is no result. One of each input is used up.
    fn cartography_result(&self, _map: &ItemStack, _additional: &ItemStack) -> Option<ItemStack> {
        None
    }
}

static CRAFT_OBSERVER: OnceLock<&'static dyn CraftObserver> = OnceLock::new();

/// Installs the craft observer. Returns `false` if one is already installed.
pub fn set_craft_observer(observer: &'static dyn CraftObserver) -> bool {
    CRAFT_OBSERVER.set(observer).is_ok()
}

/// The installed craft observer.
pub(crate) fn craft_observer() -> Option<&'static dyn CraftObserver> {
    CRAFT_OBSERVER.get().copied()
}
