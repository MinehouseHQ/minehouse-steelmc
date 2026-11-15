//! How our items are crafted. The recipes are the ones of the Fabric mods, under the same keys.

use std::sync::LazyLock;

use steel_registry::{
    REGISTRY, Registry,
    item_stack_template::ItemStackTemplate,
    items::Item,
    recipe::{
        CraftingBookCategory, CraftingInput, CraftingRecipe, Ingredient, Recipe,
        RecipeBookCategoryKind, RecipeBookProperties, RecipeProperties, ShapedRecipe,
        ShapelessRecipe, vanilla_recipe_types,
    },
    vanilla_items,
};
use steel_utils::Identifier;

use super::{CLOUD, CustomItem, JUNCTION, LEAF, LEAF_BLOCK, MAGNIFIER, PAPIROSA, STATION};
use crate::guides::book;

/// The result of a recipe: one of `item`. Built without the registry, which is not published
/// yet when our recipes are added to it.
fn result(item: &CustomItem, count: i32) -> ItemStackTemplate {
    let stack = item.stack(1);
    ItemStackTemplate::for_registry_extension(item.base, count, stack.components_patch().clone())
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
            result(&MAGNIFIER, 1),
        )),
    )
});

/// Four clouds from a feather with white wool on its four sides.
static CLOUD_RECIPE: LazyLock<Recipe<CraftingRecipe, CraftingInput>> = LazyLock::new(|| {
    let wool = || Ingredient::Item(&vanilla_items::WHITE_WOOL);
    let pattern = vec![
        Ingredient::Empty,
        wool(),
        Ingredient::Empty,
        wool(),
        Ingredient::Item(&vanilla_items::FEATHER),
        wool(),
        Ingredient::Empty,
        wool(),
        Ingredient::Empty,
    ];
    let properties = RecipeProperties {
        special: false,
        placement: None,
        recipe_book: Some(RecipeBookProperties {
            category: RecipeBookCategoryKind::Crafting(CraftingBookCategory::Building),
            group: Some("cloud".to_owned()),
            show_notification: true,
        }),
    };
    Recipe::new(
        Identifier::new("minehouse-tweaks", "cloud"),
        &vanilla_recipe_types::CRAFTING,
        CraftingRecipe::Shaped(ShapedRecipe::new(
            properties,
            3,
            3,
            pattern.into_boxed_slice(),
            result(&CLOUD, 4),
        )),
    )
});

/// A block of the metro: iron ingots around `heart`, with `rail` in the middle of the
/// bottom row.
fn metro_recipe(
    item: &CustomItem,
    heart: &'static LazyLock<Item>,
    rail: &'static LazyLock<Item>,
) -> Recipe<CraftingRecipe, CraftingInput> {
    let iron = || Ingredient::Item(&vanilla_items::IRON_INGOT);
    let pattern = vec![
        iron(),
        iron(),
        iron(),
        iron(),
        Ingredient::Item(heart),
        iron(),
        iron(),
        Ingredient::Item(rail),
        iron(),
    ];
    let properties = RecipeProperties {
        special: false,
        placement: None,
        recipe_book: Some(RecipeBookProperties {
            category: RecipeBookCategoryKind::Crafting(CraftingBookCategory::Redstone),
            group: None,
            show_notification: true,
        }),
    };
    Recipe::new(
        Identifier::new("minehouse-tweaks", item.short_id()),
        &vanilla_recipe_types::CRAFTING,
        CraftingRecipe::Shaped(ShapedRecipe::new(
            properties,
            3,
            3,
            pattern.into_boxed_slice(),
            result(item, 1),
        )),
    )
}

/// A station: iron around a clock, over a powered rail.
static STATION_RECIPE: LazyLock<Recipe<CraftingRecipe, CraftingInput>> = LazyLock::new(|| {
    metro_recipe(
        &STATION,
        &vanilla_items::CLOCK,
        &vanilla_items::POWERED_RAIL,
    )
});

/// A junction: iron around a compass, over a rail.
static JUNCTION_RECIPE: LazyLock<Recipe<CraftingRecipe, CraftingInput>> =
    LazyLock::new(|| metro_recipe(&JUNCTION, &vanilla_items::COMPASS, &vanilla_items::RAIL));

const fn misc() -> RecipeProperties {
    RecipeProperties {
        special: false,
        placement: None,
        recipe_book: Some(RecipeBookProperties {
            category: RecipeBookCategoryKind::Crafting(CraftingBookCategory::Misc),
            group: None,
            show_notification: true,
        }),
    }
}

/// Nine magic leaves make a block. Paper, which a leaf is to the client, does not.
static LEAF_BLOCK_RECIPE: LazyLock<Recipe<CraftingRecipe, CraftingInput>> = LazyLock::new(|| {
    let pattern: Vec<Ingredient> = (0..9).map(|_| LEAF.ingredient()).collect();
    Recipe::new(
        Identifier::new("minehouse-herbs", "magic_leaf_block"),
        &vanilla_recipe_types::CRAFTING,
        CraftingRecipe::Shaped(ShapedRecipe::new(
            misc(),
            3,
            3,
            pattern.into_boxed_slice(),
            result(&LEAF_BLOCK, 1),
        )),
    )
});

/// And a block comes apart into its nine leaves.
static LEAF_RECIPE: LazyLock<Recipe<CraftingRecipe, CraftingInput>> = LazyLock::new(|| {
    Recipe::new(
        Identifier::new("minehouse-herbs", "magic_leaf_from_block"),
        &vanilla_recipe_types::CRAFTING,
        CraftingRecipe::Shapeless(ShapelessRecipe::new(
            misc(),
            Box::new([LEAF_BLOCK.ingredient()]),
            result(&LEAF, 9),
        )),
    )
});

/// Two magic leaves rolled in a sheet of paper.
static PAPIROSA_RECIPE: LazyLock<Recipe<CraftingRecipe, CraftingInput>> = LazyLock::new(|| {
    Recipe::new(
        Identifier::new("minehouse-herbs", "magic_papirosa"),
        &vanilla_recipe_types::CRAFTING,
        CraftingRecipe::Shapeless(ShapelessRecipe::new(
            misc(),
            Box::new([
                LEAF.ingredient(),
                LEAF.ingredient(),
                Ingredient::Item(&vanilla_items::PAPER),
            ]),
            result(&PAPIROSA, 1),
        )),
    )
});

/// The keys of our recipes.
pub(super) const KEYS: &[(&str, &str)] = &[
    ("minehouse-tweaks", "magnifier"),
    ("minehouse-tweaks", "cloud"),
    ("minehouse-tweaks", "metro_station"),
    ("minehouse-tweaks", "metro_junction"),
    ("minehouse-herbs", "magic_leaf_block"),
    ("minehouse-herbs", "magic_leaf_from_block"),
    ("minehouse-herbs", "magic_papirosa"),
    book::RECIPE,
];

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
    registry.recipes.register(&*CLOUD_RECIPE);
    registry.recipes.register(&*STATION_RECIPE);
    registry.recipes.register(&*JUNCTION_RECIPE);
    registry.recipes.register(&*LEAF_BLOCK_RECIPE);
    registry.recipes.register(&*LEAF_RECIPE);
    registry.recipes.register(&*PAPIROSA_RECIPE);
    super::waxed_items::register(registry);
    book::register_recipe(registry);
}

#[cfg(test)]
mod tests {
    use steel_registry::{
        init_vanilla_registry, item_stack::ItemStack, recipe::RecipeMatches as _,
    };

    use super::*;
    use crate::content::recipe_id;
    use steel_registry::recipe::set_extension_item_id;

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

    #[test]
    fn four_clouds_are_crafted_from_wool_around_a_feather() {
        init_vanilla_registry();
        let wool = Some(&vanilla_items::WHITE_WOOL);
        let feather = Some(&vanilla_items::FEATHER);
        let recipe = CLOUD_RECIPE.data();
        assert!(recipe.matches(&grid([
            None, wool, None, wool, feather, wool, None, wool, None,
        ])));
        assert!(!recipe.matches(&grid([
            None, wool, None, wool, None, wool, None, wool, None,
        ])));
        let CraftingRecipe::Shaped(shaped) = recipe else {
            panic!("the cloud is a shaped recipe");
        };
        let clouds = shaped.result.create();
        assert!(CLOUD.is(&clouds));
        assert_eq!(clouds.count(), 4);
    }

    #[test]
    fn the_metro_blocks_are_crafted_from_iron_around_a_clock_or_a_compass() {
        init_vanilla_registry();
        let iron = Some(&vanilla_items::IRON_INGOT);
        let clock = Some(&vanilla_items::CLOCK);
        let compass = Some(&vanilla_items::COMPASS);
        let powered = Some(&vanilla_items::POWERED_RAIL);
        let rail = Some(&vanilla_items::RAIL);
        let around = |heart, rail| grid([iron, iron, iron, iron, heart, iron, iron, rail, iron]);
        for (recipe, item, heart, under) in [
            (&STATION_RECIPE, &STATION, clock, powered),
            (&JUNCTION_RECIPE, &JUNCTION, compass, rail),
        ] {
            let recipe = recipe.data();
            assert!(recipe.matches(&around(heart, under)));
            let CraftingRecipe::Shaped(shaped) = recipe else {
                panic!("a block of the metro is a shaped recipe");
            };
            let made = shaped.result.create();
            assert!(item.is(&made));
            assert_eq!(made.count(), 1);
        }
        // Each needs its own rail and its own heart.
        assert!(!STATION_RECIPE.data().matches(&around(clock, rail)));
        assert!(!STATION_RECIPE.data().matches(&around(compass, powered)));
        assert!(!JUNCTION_RECIPE.data().matches(&around(compass, powered)));
    }

    #[test]
    fn nine_leaves_pack_into_a_block_and_back_but_paper_does_not() {
        init_vanilla_registry();
        set_extension_item_id(recipe_id);
        let stacks = |stack: &ItemStack, count: usize| {
            let mut items = vec![ItemStack::empty(); 9];
            for slot in items.iter_mut().take(count) {
                *slot = stack.clone();
            }
            CraftingInput::new(3, 3, items)
        };
        let leaf = LEAF.stack(1);
        let paper = ItemStack::new(&vanilla_items::PAPER);

        let pack = LEAF_BLOCK_RECIPE.data();
        assert!(pack.matches(&stacks(&leaf, 9)));
        assert!(!pack.matches(&stacks(&leaf, 8)));
        assert!(!pack.matches(&stacks(&paper, 9)));
        let CraftingRecipe::Shaped(shaped) = pack else {
            panic!("packing leaves is a shaped recipe");
        };
        assert!(LEAF_BLOCK.is(&shaped.result.create()));

        let unpack = LEAF_RECIPE.data();
        assert!(unpack.matches(&stacks(&LEAF_BLOCK.stack(1), 1)));
        // A real warped wart block is not nine leaves.
        let wart = ItemStack::new(&vanilla_items::WARPED_WART_BLOCK);
        assert!(!unpack.matches(&stacks(&wart, 1)));
        let CraftingRecipe::Shapeless(shapeless) = unpack else {
            panic!("unpacking a block is a shapeless recipe");
        };
        let leaves = shapeless.result.create();
        assert!(LEAF.is(&leaves));
        assert_eq!(leaves.count(), 9);
    }
}
