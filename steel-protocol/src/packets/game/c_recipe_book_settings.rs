//! Clientbound packet syncing the recipe book's open and filter toggles.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_RECIPE_BOOK_SETTINGS;

/// Vanilla `RecipeBookSettings.TypeSettings`.
#[derive(WriteTo, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecipeBookTypeSettings {
    /// Whether the book is open beside the menu.
    pub open: bool,
    /// Whether the book shows only craftable recipes.
    pub filtering: bool,
}

/// Vanilla `ClientboundRecipeBookSettingsPacket`, one entry per `RecipeBookType`.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_RECIPE_BOOK_SETTINGS)]
pub struct CRecipeBookSettings {
    /// The crafting book.
    pub crafting: RecipeBookTypeSettings,
    /// The furnace book.
    pub furnace: RecipeBookTypeSettings,
    /// The blast furnace book.
    pub blast_furnace: RecipeBookTypeSettings,
    /// The smoker book.
    pub smoker: RecipeBookTypeSettings,
}
