//! Clientbound packet showing a recipe as a ghost on a grid the player lacks the
//! ingredients for.

use std::io::{Result, Write};

use steel_macros::ClientPacket;
use steel_registry::packets::play::C_PLACE_GHOST_RECIPE;
use steel_registry::recipe::RecipeDisplay;
use steel_utils::codec::VarInt;
use steel_utils::serial::WriteTo;

/// Vanilla `ClientboundPlaceGhostRecipePacket`.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_PLACE_GHOST_RECIPE)]
pub struct CPlaceGhostRecipe {
    /// The menu to show it in.
    pub container_id: i32,
    /// How the recipe is laid out.
    pub recipe_display: RecipeDisplay,
}

impl WriteTo for CPlaceGhostRecipe {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        VarInt(self.container_id).write(writer)?;
        self.recipe_display.write(writer)
    }
}
