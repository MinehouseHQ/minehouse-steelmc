//! Clientbound packet adding recipes to the client's recipe book.

use std::io::{Result, Write};

use steel_macros::ClientPacket;
use steel_registry::packets::play::C_RECIPE_BOOK_ADD;
use steel_registry::recipe::RecipeDisplayEntry;
use steel_utils::serial::WriteTo;

/// Vanilla `ClientboundRecipeBookAddPacket.Entry`.
#[derive(Clone, Debug)]
pub struct RecipeBookAddEntry {
    /// The display added.
    pub contents: RecipeDisplayEntry,
    /// Vanilla `FLAG_NOTIFICATION`: show the "new recipe" toast.
    pub notification: bool,
    /// Vanilla `FLAG_HIGHLIGHT`: mark the recipe as not seen yet.
    pub highlight: bool,
}

impl WriteTo for RecipeBookAddEntry {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.contents.write(writer)?;
        let flags = i8::from(self.notification) | (i8::from(self.highlight) << 1);
        flags.write(writer)
    }
}

/// Vanilla `ClientboundRecipeBookAddPacket`.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_RECIPE_BOOK_ADD)]
pub struct CRecipeBookAdd {
    /// The displays added.
    pub entries: Vec<RecipeBookAddEntry>,
    /// Whether these replace the whole book, as when a player joins.
    pub replace: bool,
}

impl WriteTo for CRecipeBookAdd {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.entries.write(writer)?;
        self.replace.write(writer)
    }
}
