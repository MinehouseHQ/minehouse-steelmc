//! Serverbound packet telling that the player saw a highlighted recipe.

use steel_macros::{ReadFrom, ServerPacket};

/// Vanilla `ServerboundRecipeBookSeenRecipePacket`.
#[derive(ServerPacket, ReadFrom, Clone, Debug)]
pub struct SRecipeBookSeenRecipe {
    /// The `RecipeDisplayId` seen.
    #[read(as = VarInt)]
    pub recipe: i32,
}
