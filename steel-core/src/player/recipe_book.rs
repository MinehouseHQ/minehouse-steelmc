//! Vanilla `ServerRecipeBook`: the recipes a player has unlocked, which of them
//! are still new, and the book's open and filter toggles.

use std::collections::BTreeSet;
use std::io::Cursor;

use simdnbt::owned::{NbtCompound, NbtList, NbtTag, read_compound};
use steel_protocol::packets::game::{
    CRecipeBookAdd, CRecipeBookRemove, CRecipeBookSettings, RecipeBookAddEntry, RecipeBookType,
    RecipeBookTypeSettings, SRecipeBookChangeSettings, SRecipeBookSeenRecipe,
};
use steel_registry::REGISTRY;
use steel_registry::recipe::UntypedRecipeRef;
use steel_utils::Identifier;
use steel_utils::codec::VarInt;

use super::Player;
use crate::inventory::recipe_displays::{RECIPE_DISPLAYS, is_special};
use crate::player::advancement::criteria::trigger;
use crate::player::advancement::criteria::triggers::RECIPE_UNLOCKED;

/// Vanilla `ServerRecipeBook.RECIPE_BOOK_TAG`.
pub const RECIPE_BOOK_TAG: &str = "recipeBook";

/// Vanilla `RecipeBookSettings.TypeSettings` NBT field names, in `RecipeBookType` order.
const SETTINGS_FIELDS: [(&str, &str); 4] = [
    ("isGuiOpen", "isFilteringCraftable"),
    ("isFurnaceGuiOpen", "isFurnaceFilteringCraftable"),
    (
        "isBlastingFurnaceGuiOpen",
        "isBlastingFurnaceFilteringCraftable",
    ),
    ("isSmokerGuiOpen", "isSmokerFilteringCraftable"),
];

/// Vanilla `ServerRecipeBook` state. Sorted sets keep saves deterministic where
/// vanilla's identity sets have no order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerRecipeBook {
    /// Vanilla `RecipeBookSettings`, in `RecipeBookType` order.
    settings: [RecipeBookTypeSettings; 4],
    /// Vanilla `known`: the unlocked recipes.
    known: BTreeSet<Identifier>,
    /// Vanilla `highlight`: unlocked recipes the player has not looked at yet.
    highlight: BTreeSet<Identifier>,
}

impl ServerRecipeBook {
    /// Vanilla `ServerRecipeBook.contains`.
    #[must_use]
    pub fn contains(&self, recipe: &Identifier) -> bool {
        self.known.contains(recipe)
    }

    /// Vanilla `ServerRecipeBook.add`.
    pub fn add(&mut self, recipe: Identifier) {
        self.known.insert(recipe);
    }

    /// Vanilla `ServerRecipeBook.remove`.
    pub fn remove(&mut self, recipe: &Identifier) {
        self.known.remove(recipe);
        self.highlight.remove(recipe);
    }

    /// Vanilla `ServerRecipeBook.removeHighlight`.
    pub fn remove_highlight(&mut self, recipe: &Identifier) {
        self.highlight.remove(recipe);
    }

    /// Whether `recipe` is still marked new.
    #[must_use]
    pub fn is_highlighted(&self, recipe: &Identifier) -> bool {
        self.highlight.contains(recipe)
    }

    /// Vanilla `RecipeBook.isOpen`.
    #[must_use]
    pub const fn is_open(&self, book: RecipeBookType) -> bool {
        self.settings[book as usize].open
    }

    /// Vanilla `RecipeBook.isFiltering`.
    #[must_use]
    pub const fn is_filtering(&self, book: RecipeBookType) -> bool {
        self.settings[book as usize].filtering
    }

    /// Vanilla `RecipeBook.setBookSetting`.
    pub const fn set_book_setting(&mut self, book: RecipeBookType, open: bool, filtering: bool) {
        self.settings[book as usize] = RecipeBookTypeSettings { open, filtering };
    }

    /// Vanilla `ClientboundRecipeBookSettingsPacket` of these settings.
    #[must_use]
    pub const fn settings_packet(&self) -> CRecipeBookSettings {
        CRecipeBookSettings {
            crafting: self.settings[0],
            furnace: self.settings[1],
            blast_furnace: self.settings[2],
            smoker: self.settings[3],
        }
    }

    /// Vanilla `ServerRecipeBook.pack` written with `Packed.CODEC` into a compound.
    #[must_use]
    pub fn save(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        for (settings, (open, filtering)) in self.settings.iter().zip(SETTINGS_FIELDS) {
            nbt.insert(open, NbtTag::Byte(i8::from(settings.open)));
            nbt.insert(filtering, NbtTag::Byte(i8::from(settings.filtering)));
        }
        nbt.insert("recipes", id_list(&self.known));
        nbt.insert("toBeDisplayed", id_list(&self.highlight));
        nbt
    }

    /// Vanilla `ServerRecipeBook.loadUntrusted` of a `Packed.CODEC` compound: recipes
    /// the server does not have are dropped.
    #[must_use]
    pub fn load(nbt: &NbtCompound) -> Self {
        let mut book = Self::default();
        for (settings, (open, filtering)) in book.settings.iter_mut().zip(SETTINGS_FIELDS) {
            settings.open = nbt.byte(open).is_some_and(|value| value != 0);
            settings.filtering = nbt.byte(filtering).is_some_and(|value| value != 0);
        }
        book.known = known_recipes(nbt, "recipes");
        book.highlight = known_recipes(nbt, "toBeDisplayed");
        book
    }

    /// [`Self::save`] as Steel's stored NBT bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        self.save().write(&mut bytes);
        bytes
    }

    /// [`Self::load`] from Steel's stored NBT bytes; empty or broken bytes are an
    /// empty book.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        if bytes.is_empty() {
            return Self::default();
        }
        match read_compound(&mut Cursor::new(bytes)) {
            Ok(nbt) => Self::load(&nbt),
            Err(error) => {
                log::error!("Couldn't parse player recipe book data: {error:?}");
                Self::default()
            }
        }
    }
}

fn id_list(ids: &BTreeSet<Identifier>) -> NbtTag {
    NbtTag::List(NbtList::String(
        ids.iter().map(|id| id.to_string().into()).collect(),
    ))
}

/// Vanilla `ServerRecipeBook.loadRecipes`, logging each recipe it drops.
fn known_recipes(nbt: &NbtCompound, key: &str) -> BTreeSet<Identifier> {
    let Some(values) = nbt.list(key).and_then(NbtList::strings) else {
        return BTreeSet::new();
    };
    values
        .iter()
        .filter_map(|value| {
            let value = value.to_str();
            let recipe = value
                .parse::<Identifier>()
                .ok()
                .filter(|id| REGISTRY.recipes.by_key(id).is_some());
            if recipe.is_none() {
                log::error!("Tried to load unrecognized recipe: {value} removed now.");
            }
            recipe
        })
        .collect()
}

impl Player {
    /// Vanilla `ServerPlayer.awardRecipes` (`ServerRecipeBook.addRecipes`): unlocks the
    /// learnable recipes not known yet and returns how many displays were added.
    ///
    /// Every recipe is added before any `recipe_unlocked` criterion fires, since a
    /// criterion can grant an advancement whose reward awards recipes again.
    pub fn award_recipes(&self, recipes: &[UntypedRecipeRef]) -> usize {
        let mut unlocked = Vec::new();
        let mut entries = Vec::new();
        {
            let mut book = self.recipe_book.lock();
            for &recipe in recipes {
                if book.contains(recipe.key()) || is_special(recipe) {
                    continue;
                }
                book.add(recipe.key().clone());
                book.highlight.insert(recipe.key().clone());
                let notification = recipe
                    .data()
                    .properties()
                    .and_then(|properties| properties.recipe_book.as_ref())
                    .is_some_and(|book| book.show_notification);
                entries.extend(
                    RECIPE_DISPLAYS
                        .displays_for_recipe(recipe.key())
                        .map(|display| RecipeBookAddEntry {
                            contents: display.clone(),
                            notification,
                            highlight: true,
                        }),
                );
                unlocked.push(recipe);
            }
        }
        for recipe in unlocked {
            self.trigger_recipe_unlocked(recipe);
        }
        let added = entries.len();
        if added > 0 {
            self.send_packet(CRecipeBookAdd {
                entries,
                replace: false,
            });
        }
        added
    }

    /// Vanilla `ServerPlayer.awardRecipesByKey`: unknown keys are skipped.
    pub fn award_recipes_by_key(&self, keys: &[Identifier]) -> usize {
        let recipes: Vec<UntypedRecipeRef> = keys
            .iter()
            .filter_map(|key| REGISTRY.recipes.by_key(key))
            .collect();
        self.award_recipes(&recipes)
    }

    /// Vanilla `ServerPlayer.resetRecipes` (`ServerRecipeBook.removeRecipes`): locks the
    /// known recipes again and returns how many displays were removed.
    pub fn reset_recipes(&self, recipes: &[UntypedRecipeRef]) -> usize {
        let mut removed = Vec::new();
        {
            let mut book = self.recipe_book.lock();
            for &recipe in recipes {
                if !book.contains(recipe.key()) {
                    continue;
                }
                book.remove(recipe.key());
                removed.extend(
                    RECIPE_DISPLAYS
                        .displays_for_recipe(recipe.key())
                        .map(|display| VarInt(i32::try_from(display.id).unwrap_or(i32::MAX))),
                );
            }
        }
        let count = removed.len();
        if count > 0 {
            self.send_packet(CRecipeBookRemove { recipes: removed });
        }
        count
    }

    /// Vanilla `ServerRecipeBook.sendInitialRecipeBook`: the settings, then every known
    /// recipe replacing what the client had.
    pub fn send_initial_recipe_book(&self) {
        let book = self.recipe_book.lock().clone();
        self.send_packet(book.settings_packet());
        let entries = book
            .known
            .iter()
            .flat_map(|recipe| {
                let highlight = book.is_highlighted(recipe);
                RECIPE_DISPLAYS
                    .displays_for_recipe(recipe)
                    .map(move |display| RecipeBookAddEntry {
                        contents: display.clone(),
                        notification: false,
                        highlight,
                    })
            })
            .collect();
        self.send_packet(CRecipeBookAdd {
            entries,
            replace: true,
        });
    }

    /// A copy of the player's recipe book.
    #[must_use]
    pub fn recipe_book(&self) -> ServerRecipeBook {
        self.recipe_book.lock().clone()
    }

    /// Replaces the player's recipe book, as loading saved data does.
    pub fn set_recipe_book(&self, book: ServerRecipeBook) {
        *self.recipe_book.lock() = book;
    }

    /// Whether the player has unlocked `recipe`.
    #[must_use]
    pub fn knows_recipe(&self, recipe: &Identifier) -> bool {
        self.recipe_book.lock().contains(recipe)
    }

    /// Vanilla `ServerGamePacketListenerImpl.handleRecipeBookSeenRecipePacket`.
    pub fn handle_recipe_book_seen_recipe(&self, packet: &SRecipeBookSeenRecipe) {
        if let Some(info) = RECIPE_DISPLAYS.by_display_id(packet.recipe) {
            self.recipe_book.lock().remove_highlight(info.parent.key());
        }
    }

    /// Vanilla `ServerGamePacketListenerImpl.handleRecipeBookChangeSettingsPacket`.
    pub fn handle_recipe_book_change_settings(&self, packet: &SRecipeBookChangeSettings) {
        self.recipe_book.lock().set_book_setting(
            packet.book_type,
            packet.is_open,
            packet.is_filtering,
        );
    }

    /// Vanilla `RecipeUnlockedTrigger.trigger`.
    fn trigger_recipe_unlocked(&self, recipe: UntypedRecipeRef) {
        trigger(self, &RECIPE_UNLOCKED, |conditions| {
            conditions
                .recipes
                .as_ref()
                .is_some_and(|recipes| recipes.0.contains(recipe.key()))
        });
    }
}

#[cfg(test)]
mod tests {
    use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
    use steel_protocol::packets::game::RecipeBookType;
    use steel_registry::init_vanilla_registry;
    use steel_utils::Identifier;

    use super::ServerRecipeBook;

    #[test]
    fn saved_books_load_back_with_vanilla_field_names() {
        init_vanilla_registry();
        let mut book = ServerRecipeBook::default();
        let planks = Identifier::vanilla_static("oak_planks");
        let furnace = Identifier::vanilla_static("furnace");
        book.add(planks.clone());
        book.highlight.insert(planks.clone());
        book.add(furnace.clone());
        book.set_book_setting(RecipeBookType::Crafting, true, true);
        book.set_book_setting(RecipeBookType::BlastFurnace, false, true);

        let nbt = book.save();
        assert_eq!(nbt.byte("isGuiOpen"), Some(1));
        assert_eq!(nbt.byte("isFilteringCraftable"), Some(1));
        assert_eq!(nbt.byte("isBlastingFurnaceFilteringCraftable"), Some(1));
        assert_eq!(nbt.byte("isSmokerGuiOpen"), Some(0));
        assert_eq!(
            nbt.list("toBeDisplayed")
                .and_then(NbtList::strings)
                .map(<[_]>::len),
            Some(1)
        );
        assert_eq!(ServerRecipeBook::load(&nbt), book);
        assert_eq!(ServerRecipeBook::from_bytes(&book.to_bytes()), book);
        assert!(book.is_highlighted(&planks));
        assert!(!book.is_highlighted(&furnace));
    }

    #[test]
    fn unknown_recipes_and_broken_bytes_are_dropped() {
        init_vanilla_registry();
        let mut nbt = NbtCompound::new();
        nbt.insert(
            "recipes",
            NbtTag::List(NbtList::String(vec![
                "minecraft:torch".into(),
                "minecraft:not_a_recipe".into(),
                "not a key".into(),
            ])),
        );
        let book = ServerRecipeBook::load(&nbt);
        assert!(book.contains(&Identifier::vanilla_static("torch")));
        assert_eq!(book.known.len(), 1);
        assert!(!book.is_open(RecipeBookType::Furnace));

        assert_eq!(
            ServerRecipeBook::from_bytes(&[]),
            ServerRecipeBook::default()
        );
        assert_eq!(
            ServerRecipeBook::from_bytes(&[0xff, 0x00, 0x13]),
            ServerRecipeBook::default()
        );
    }

    #[test]
    fn removing_a_recipe_clears_its_highlight() {
        let mut book = ServerRecipeBook::default();
        let torch = Identifier::vanilla_static("torch");
        book.add(torch.clone());
        book.highlight.insert(torch.clone());
        book.remove(&torch);
        assert!(!book.contains(&torch));
        assert!(!book.is_highlighted(&torch));
    }
}
