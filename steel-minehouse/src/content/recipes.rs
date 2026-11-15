//! How our items are crafted. The recipes are the ones of the Fabric mods, under the same keys.

use std::sync::LazyLock;

use steel_registry::{
    REGISTRY, Registry,
    item_stack_template::ItemStackTemplate,
    recipe::{
        CraftingBookCategory, CraftingInput, CraftingRecipe, Ingredient, Recipe,
        RecipeBookCategoryKind, RecipeBookProperties, RecipeProperties, ShapedRecipe,
        vanilla_recipe_types,
    },
    vanilla_items,
};
use steel_utils::Identifier;

use super::{CustomItem, MAGNIFIER};

/// The result of a recipe: one of `item`. Built without the registry, which is not published
/// yet when our recipes are added to it.
fn result(item: &CustomItem) -> ItemStackTemplate {
    let stack = item.stack(1);
    ItemStackTemplate::for_registry_extension(item.base, 1, stack.components_patch().clone())
}

const fn equipment() -> RecipeProperties {
    RecipeProperties {
        special: false,
        placement: None,
        recipe_book: Some(RecipeBookProperties {
            category: RecipeBookCategoryKind::Crafting(CraftingBookCategory::Equipment),
            group: None,
            show_notification: true,
        }),
    }
}

/// Copper around a glass pane, on a stick.
static MAGNIFIER_RECIPE: LazyLock<Recipe<CraftingRecipe, CraftingInput>> = LazyLock::new(|| {
    let copper = || Ingredient::Item(&vanilla_items::COPPER_INGOT);
    let glass = Ingredient::Item(&vanilla_items::GLASS_PANE);
    let stick = Ingredient::Item(&vanilla_items::STICK);
    let pattern = vec![
        Ingredient::Empty,
        copper(),
        Ingredient::Empty,
        copper(),
        glass,
        copper(),
        stick,
        copper(),
        Ingredient::Empty,
    ];
    Recipe::new(
        Identifier::new("minehouse-tweaks", "magnifier"),
        &vanilla_recipe_types::CRAFTING,
        CraftingRecipe::Shaped(ShapedRecipe::new(
            equipment(),
            3,
            3,
            pattern.into_boxed_slice(),
            result(&MAGNIFIER),
        )),
    )
});

/// The keys of our recipes.
pub(super) const KEYS: &[(&str, &str)] = &[("minehouse-tweaks", "magnifier")];

/// How many of our recipes the published registry holds.
pub(super) fn registered() -> usize {
    KEYS.iter()
        .filter(|(namespace, path)| {
            REGISTRY
                .recipes
                .by_key(&Identifier::new(*namespace, *path))
                .is_some()
        })
        .count()
}

/// Adds our recipes to the registry. Given to [`steel_registry::extend_registry`].
pub(super) fn register(registry: &mut Registry) {
    registry.recipes.register(&*MAGNIFIER_RECIPE);
}

#[cfg(test)]
mod tests {
    use steel_registry::{
        init_vanilla_registry, item_stack::ItemStack, items::Item, recipe::RecipeMatches as _,
    };

    use super::*;

    fn grid(items: [Option<&'static LazyLock<Item>>; 9]) -> CraftingInput {
        CraftingInput::new(
            3,
            3,
            items
                .into_iter()
                .map(|item| item.map_or_else(ItemStack::empty, |item| ItemStack::new(item)))
                .collect(),
        )
    }

    #[test]
    fn the_magnifier_is_crafted_from_copper_glass_and_a_stick() {
        init_vanilla_registry();
        let copper = Some(&vanilla_items::COPPER_INGOT);
        let glass = Some(&vanilla_items::GLASS_PANE);
        let stick = Some(&vanilla_items::STICK);
        let recipe = MAGNIFIER_RECIPE.data();
        assert!(recipe.matches(&grid([
            None, copper, None, copper, glass, copper, stick, copper, None,
        ])));
        // Without the stick it is nothing.
        assert!(!recipe.matches(&grid([
            None, copper, None, copper, glass, copper, None, copper, None,
        ])));
        let CraftingRecipe::Shaped(shaped) = recipe else {
            panic!("the magnifier is a shaped recipe");
        };
        assert!(MAGNIFIER.is(&shaped.result.create()));
    }
}
