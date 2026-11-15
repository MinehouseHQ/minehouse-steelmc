//! Serverbound packet toggling a recipe book open or filtering.

use steel_macros::{ReadFrom, ServerPacket};

/// Vanilla `RecipeBookType`.
#[derive(ReadFrom, Clone, Copy, Debug, PartialEq, Eq)]
#[read(as = VarInt)]
pub enum RecipeBookType {
    /// The crafting table and inventory book.
    Crafting = 0,
    /// The furnace book.
    Furnace = 1,
    /// The blast furnace book.
    BlastFurnace = 2,
    /// The smoker book.
    Smoker = 3,
}

/// Vanilla `ServerboundRecipeBookChangeSettingsPacket`.
#[derive(ServerPacket, ReadFrom, Clone, Debug)]
pub struct SRecipeBookChangeSettings {
    /// The book changed.
    pub book_type: RecipeBookType,
    /// Whether it is open.
    pub is_open: bool,
    /// Whether it filters to craftable recipes.
    pub is_filtering: bool,
}
