//! Vanilla `RecipeManager.unpackRecipeInfo`: every recipe-book display of every recipe,
//! numbered the way `RecipeDisplayId` refers to them.

use std::sync::LazyLock;

use rustc_hash::FxHashMap;
use steel_registry::recipe::{
    CookingBookCategory, CookingRecipe, CraftingBookCategory, CraftingRecipe,
    RecipeBookCategoryKind, RecipeBookCategoryRef, RecipeDisplay, RecipeDisplayEntry,
    RecipeProperties, SmithingRecipe, StonecuttingRecipe, UntypedRecipeRef,
    vanilla_recipe_book_categories as categories, vanilla_recipe_types,
};
use steel_registry::{REGISTRY, items::ItemRef, vanilla_items};
use steel_utils::Identifier;

/// Vanilla `RecipeManager.ServerDisplayInfo`: a display and the recipe it shows.
#[derive(Debug)]
pub struct ServerDisplayInfo {
    /// The display as the client receives it.
    pub display: RecipeDisplayEntry,
    /// The recipe shown.
    pub parent: UntypedRecipeRef,
}

/// The displays of every recipe and the recipes a player can learn.
#[derive(Debug)]
pub struct RecipeDisplays {
    all: Vec<ServerDisplayInfo>,
    by_recipe: FxHashMap<Identifier, Vec<usize>>,
    learnable: Vec<UntypedRecipeRef>,
}

/// The server's recipe displays, built from the frozen recipe registry on first use.
pub static RECIPE_DISPLAYS: LazyLock<RecipeDisplays> = LazyLock::new(RecipeDisplays::build);

/// Vanilla `Recipe.isSpecial`. A plugin recipe without recipe properties has nothing
/// to show in the book, so it counts as special too.
#[must_use]
pub fn is_special(recipe: UntypedRecipeRef) -> bool {
    recipe
        .data()
        .properties()
        .is_none_or(|properties| properties.special)
}

impl RecipeDisplays {
    fn build() -> Self {
        let mut all = Vec::new();
        let mut by_recipe: FxHashMap<Identifier, Vec<usize>> = FxHashMap::default();
        let mut groups: FxHashMap<&str, usize> = FxHashMap::default();
        let mut learnable = Vec::new();
        for recipe in REGISTRY.recipes.iter() {
            let special = is_special(recipe);
            if !special {
                learnable.push(recipe);
            }
            let Some(properties) = recipe.data().properties() else {
                continue;
            };
            let displays = displays(recipe);
            let Some(category) = category(recipe, properties) else {
                continue;
            };
            let group = group(properties).map(|group| {
                let next = groups.len();
                *groups.entry(group).or_insert(next)
            });
            // Vanilla `placementCheck`: none for special recipes.
            let crafting_requirements = (!special).then(|| {
                properties
                    .placement
                    .as_ref()
                    .map_or_else(Vec::new, |placement| placement.ingredients.clone())
            });
            for display in displays {
                let id = all.len();
                by_recipe.entry(recipe.key().clone()).or_default().push(id);
                all.push(ServerDisplayInfo {
                    display: RecipeDisplayEntry {
                        id,
                        display,
                        group,
                        category,
                        crafting_requirements: crafting_requirements.clone(),
                    },
                    parent: recipe,
                });
            }
        }
        Self {
            all,
            by_recipe,
            learnable,
        }
    }

    /// Vanilla `RecipeManager.getRecipeFromDisplay`.
    #[must_use]
    pub fn by_display_id(&self, id: i32) -> Option<&ServerDisplayInfo> {
        usize::try_from(id).ok().and_then(|id| self.all.get(id))
    }

    /// Vanilla `RecipeManager.listDisplaysForRecipe`.
    pub fn displays_for_recipe(
        &self,
        key: &Identifier,
    ) -> impl Iterator<Item = &RecipeDisplayEntry> {
        self.by_recipe
            .get(key)
            .into_iter()
            .flatten()
            .map(|&index| &self.all[index].display)
    }

    /// Vanilla `RecipeManager.getLearnableRecipes`.
    #[must_use]
    pub fn learnable(&self) -> &[UntypedRecipeRef] {
        &self.learnable
    }
}

/// Vanilla `Recipe.display` for each recipe type that has displays.
fn displays(recipe: UntypedRecipeRef) -> Vec<RecipeDisplay> {
    if let Some(crafting) = recipe.downcast_data::<CraftingRecipe>() {
        return crafting.displays();
    }
    if let Some(cooking) = recipe.downcast_data::<CookingRecipe>() {
        return furnace_icon(recipe.recipe_type().key.clone())
            .map_or_else(Vec::new, |station| cooking.displays(station));
    }
    if let Some(stonecutting) = recipe.downcast_data::<StonecuttingRecipe>() {
        return stonecutting.displays();
    }
    if let Some(smithing) = recipe.downcast_data::<SmithingRecipe>() {
        return smithing.displays();
    }
    Vec::new()
}

/// Vanilla `AbstractCookingRecipe.furnaceIcon` of each cooking recipe type.
fn furnace_icon(recipe_type: Identifier) -> Option<ItemRef> {
    let icon: ItemRef = if recipe_type == *vanilla_recipe_types::SMELTING.key() {
        &vanilla_items::FURNACE
    } else if recipe_type == *vanilla_recipe_types::BLASTING.key() {
        &vanilla_items::BLAST_FURNACE
    } else if recipe_type == *vanilla_recipe_types::SMOKING.key() {
        &vanilla_items::SMOKER
    } else if recipe_type == *vanilla_recipe_types::CAMPFIRE_COOKING.key() {
        &vanilla_items::CAMPFIRE
    } else {
        return None;
    };
    Some(icon)
}

/// Vanilla `Recipe.recipeBookCategory`; cooking categories depend on the recipe type.
fn category(
    recipe: UntypedRecipeRef,
    properties: &RecipeProperties,
) -> Option<RecipeBookCategoryRef> {
    let kind = properties.recipe_book.as_ref().map_or(
        RecipeBookCategoryKind::Crafting(CraftingBookCategory::Misc),
        |book| book.category,
    );
    let recipe_type = &recipe.recipe_type().key;
    let category: RecipeBookCategoryRef = match kind {
        RecipeBookCategoryKind::Crafting(CraftingBookCategory::Building) => {
            &categories::CRAFTING_BUILDING_BLOCKS
        }
        RecipeBookCategoryKind::Crafting(CraftingBookCategory::Redstone) => {
            &categories::CRAFTING_REDSTONE
        }
        RecipeBookCategoryKind::Crafting(CraftingBookCategory::Equipment) => {
            &categories::CRAFTING_EQUIPMENT
        }
        RecipeBookCategoryKind::Crafting(CraftingBookCategory::Misc) => &categories::CRAFTING_MISC,
        RecipeBookCategoryKind::Cooking(cooking) => cooking_category(recipe_type, cooking)?,
        RecipeBookCategoryKind::Stonecutter => &categories::STONECUTTER,
        RecipeBookCategoryKind::Smithing => &categories::SMITHING,
    };
    Some(category)
}

/// Vanilla `SmeltingRecipe`, `BlastingRecipe`, `SmokingRecipe` and
/// `CampfireCookingRecipe.recipeBookCategory`.
fn cooking_category(
    recipe_type: &Identifier,
    category: CookingBookCategory,
) -> Option<RecipeBookCategoryRef> {
    let resolved: RecipeBookCategoryRef = if recipe_type == vanilla_recipe_types::SMELTING.key() {
        match category {
            CookingBookCategory::Food => &categories::FURNACE_FOOD,
            CookingBookCategory::Blocks => &categories::FURNACE_BLOCKS,
            CookingBookCategory::Misc => &categories::FURNACE_MISC,
        }
    } else if recipe_type == vanilla_recipe_types::BLASTING.key() {
        match category {
            CookingBookCategory::Blocks => &categories::BLAST_FURNACE_BLOCKS,
            CookingBookCategory::Food | CookingBookCategory::Misc => {
                &categories::BLAST_FURNACE_MISC
            }
        }
    } else if recipe_type == vanilla_recipe_types::SMOKING.key() {
        &categories::SMOKER_FOOD
    } else if recipe_type == vanilla_recipe_types::CAMPFIRE_COOKING.key() {
        &categories::CAMPFIRE
    } else {
        return None;
    };
    Some(resolved)
}

/// Vanilla `Recipe.group`: stonecutter and smithing recipes have none.
fn group(properties: &RecipeProperties) -> Option<&str> {
    let book = properties.recipe_book.as_ref()?;
    match book.category {
        RecipeBookCategoryKind::Crafting(_) | RecipeBookCategoryKind::Cooking(_) => {
            book.group.as_deref()
        }
        RecipeBookCategoryKind::Stonecutter | RecipeBookCategoryKind::Smithing => None,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use steel_registry::{REGISTRY, init_vanilla_registry};
    use steel_utils::Identifier;
    use steel_utils::codec::VarInt;
    use steel_utils::serial::{ReadFrom as _, WriteTo as _};

    use super::{RECIPE_DISPLAYS, is_special};

    #[test]
    fn display_ids_number_every_display_in_order() {
        init_vanilla_registry();
        for (index, info) in RECIPE_DISPLAYS.all.iter().enumerate() {
            assert_eq!(info.display.id, index);
            let found = RECIPE_DISPLAYS
                .by_display_id(i32::try_from(index).expect("display ids fit an int"))
                .expect("every id resolves");
            assert_eq!(found.parent.key(), info.parent.key());
            // Vanilla's `placementCheck` is empty only for special recipes.
            assert_eq!(
                info.display.crafting_requirements.is_none(),
                is_special(info.parent)
            );
        }
        assert!(RECIPE_DISPLAYS.by_display_id(-1).is_none());
    }

    #[test]
    fn learnable_recipes_skip_special_ones() {
        init_vanilla_registry();
        let learnable = RECIPE_DISPLAYS.learnable();
        assert!(learnable.iter().all(|&recipe| !is_special(recipe)));
        let special = REGISTRY
            .recipes
            .by_key(&Identifier::vanilla_static("book_cloning"))
            .expect("vanilla has book cloning");
        assert!(is_special(special));
        assert_eq!(
            learnable.len() + REGISTRY.recipes.iter().filter(|&r| is_special(r)).count(),
            REGISTRY.recipes.iter().count()
        );
    }

    #[test]
    fn shaped_recipe_display_writes_its_id_first() {
        init_vanilla_registry();
        let planks = Identifier::vanilla_static("oak_planks");
        let displays: Vec<_> = RECIPE_DISPLAYS.displays_for_recipe(&planks).collect();
        assert_eq!(displays.len(), 1);
        let display = displays[0];
        let mut bytes = Vec::new();
        display.write(&mut bytes).expect("display encodes");
        let mut cursor = Cursor::new(bytes.as_slice());
        let id = VarInt::read(&mut cursor).expect("display id decodes");
        assert_eq!(usize::try_from(id.0).ok(), Some(display.id));
        // Oak planks are a shapeless crafting recipe with a group.
        let kind = VarInt::read(&mut cursor).expect("display type decodes");
        assert_eq!(kind.0, 0, "shapeless display type");
        assert!(display.group.is_some());
    }
}
