//! Serverbound packet asking to place a recipe from the recipe book onto a grid.

use steel_macros::{ReadFrom, ServerPacket};

/// Vanilla `ServerboundPlaceRecipePacket`.
#[derive(ServerPacket, ReadFrom, Clone, Debug)]
pub struct SPlaceRecipe {
    /// The menu to place into.
    #[read(as = VarInt)]
    pub container_id: i32,
    /// The `RecipeDisplayId` of the recipe.
    #[read(as = VarInt)]
    pub recipe: i32,
    /// Place as many as the ingredients allow (shift-click).
    pub use_max_items: bool,
}
