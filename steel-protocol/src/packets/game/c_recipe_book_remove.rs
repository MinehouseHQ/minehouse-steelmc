//! Clientbound packet removing recipes from the client's recipe book.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_RECIPE_BOOK_REMOVE;
use steel_utils::codec::VarInt;

/// Vanilla `ClientboundRecipeBookRemovePacket`: the `RecipeDisplayId`s removed.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_RECIPE_BOOK_REMOVE)]
pub struct CRecipeBookRemove {
    /// The display ids removed.
    pub recipes: Vec<VarInt>,
}
