//! Waxed maps and smithing templates (the Fabric `MapLock` and `TemplateLock`).
//!
//! On the cartography table a filled map with honeycomb in the lower slot gives the same
//! map, waxed. On the smithing table a template with honeycomb in the middle slot gives the
//! same template, waxed. A waxed one cannot be copied: a waxed map neither with an empty map
//! on the cartography table nor in the crafting grid, a waxed template in no shaped recipe.
//! Everything else stays vanilla: the map still updates, zooms out and locks, and the
//! template still works on the smithing table. Players can sell them knowing the buyer
//! cannot make copies.
//!
//! The mark is `custom_data.minehouse_waxed`, the one of the Fabric mod, plus a lore line a
//! vanilla client shows.

use std::sync::LazyLock;

use simdnbt::owned::NbtCompound;
use steel_core::inventory::craft_observer::CraftObserver;
use steel_registry::{
    REGISTRY, Registry,
    data_components::vanilla_components::{CUSTOM_DATA, CustomData, ItemLore, LORE, MAP_ID},
    item_stack::ItemStack,
    item_stack_template::ItemStackTemplate,
    items::Item,
    recipe::{
        CraftingInput, CraftingRecipe, Ingredient, Recipe, RecipeProperties, SmithingRecipe,
        SmithingRecipeInput, SmithingTransformRecipe, vanilla_recipe_types,
    },
    vanilla_items,
};
use steel_utils::Identifier;
use text_components::{Modifier as _, TextComponent, format::Color};

/// The key of the mark in `custom_data`.
const TAG: &str = "minehouse_waxed";
/// The lore line of a waxed map.
const MAP_LORE: &str = "Вощёная: нельзя скопировать";
/// The lore line of a waxed template.
const TEMPLATE_LORE: &str = "Вощёный: нельзя скопировать";

/// Every smithing template, with the path of its item.
static TEMPLATES: [(&str, &LazyLock<Item>); 19] = [
    (
        "bolt_armor_trim_smithing_template",
        &vanilla_items::BOLT_ARMOR_TRIM_SMITHING_TEMPLATE,
    ),
    (
        "coast_armor_trim_smithing_template",
        &vanilla_items::COAST_ARMOR_TRIM_SMITHING_TEMPLATE,
    ),
    (
        "dune_armor_trim_smithing_template",
        &vanilla_items::DUNE_ARMOR_TRIM_SMITHING_TEMPLATE,
    ),
    (
        "eye_armor_trim_smithing_template",
        &vanilla_items::EYE_ARMOR_TRIM_SMITHING_TEMPLATE,
    ),
    (
        "flow_armor_trim_smithing_template",
        &vanilla_items::FLOW_ARMOR_TRIM_SMITHING_TEMPLATE,
    ),
    (
        "host_armor_trim_smithing_template",
        &vanilla_items::HOST_ARMOR_TRIM_SMITHING_TEMPLATE,
    ),
    (
        "netherite_upgrade_smithing_template",
        &vanilla_items::NETHERITE_UPGRADE_SMITHING_TEMPLATE,
    ),
    (
        "raiser_armor_trim_smithing_template",
        &vanilla_items::RAISER_ARMOR_TRIM_SMITHING_TEMPLATE,
    ),
    (
        "rib_armor_trim_smithing_template",
        &vanilla_items::RIB_ARMOR_TRIM_SMITHING_TEMPLATE,
    ),
    (
        "sentry_armor_trim_smithing_template",
        &vanilla_items::SENTRY_ARMOR_TRIM_SMITHING_TEMPLATE,
    ),
    (
        "shaper_armor_trim_smithing_template",
        &vanilla_items::SHAPER_ARMOR_TRIM_SMITHING_TEMPLATE,
    ),
    (
        "silence_armor_trim_smithing_template",
        &vanilla_items::SILENCE_ARMOR_TRIM_SMITHING_TEMPLATE,
    ),
    (
        "snout_armor_trim_smithing_template",
        &vanilla_items::SNOUT_ARMOR_TRIM_SMITHING_TEMPLATE,
    ),
    (
        "spire_armor_trim_smithing_template",
        &vanilla_items::SPIRE_ARMOR_TRIM_SMITHING_TEMPLATE,
    ),
    (
        "tide_armor_trim_smithing_template",
        &vanilla_items::TIDE_ARMOR_TRIM_SMITHING_TEMPLATE,
    ),
    (
        "vex_armor_trim_smithing_template",
        &vanilla_items::VEX_ARMOR_TRIM_SMITHING_TEMPLATE,
    ),
    (
        "ward_armor_trim_smithing_template",
        &vanilla_items::WARD_ARMOR_TRIM_SMITHING_TEMPLATE,
    ),
    (
        "wayfinder_armor_trim_smithing_template",
        &vanilla_items::WAYFINDER_ARMOR_TRIM_SMITHING_TEMPLATE,
    ),
    (
        "wild_armor_trim_smithing_template",
        &vanilla_items::WILD_ARMOR_TRIM_SMITHING_TEMPLATE,
    ),
];

/// How many waxing recipes there are.
pub(super) const RECIPES: usize = TEMPLATES.len();

/// Whether `stack` carries the wax mark.
#[must_use]
pub fn is_waxed(stack: &ItemStack) -> bool {
    stack
        .get(CUSTOM_DATA)
        .and_then(|data| data.as_compound().byte(TAG))
        .is_some_and(|waxed| waxed != 0)
}

fn is_map(stack: &ItemStack) -> bool {
    stack.has(MAP_ID)
}

fn is_template(stack: &ItemStack) -> bool {
    TEMPLATES.iter().any(|(_, item)| stack.is(item))
}

/// Puts the wax mark and `lore` on `stack`, keeping the custom data and lore it has.
fn mark(stack: &mut ItemStack, lore: &'static str) {
    let mut tag = stack
        .get(CUSTOM_DATA)
        .map_or_else(NbtCompound::new, CustomData::copy_tag);
    tag.insert(TAG, true);
    if let Some(data) = CustomData::try_from_compound(tag) {
        stack.set(CUSTOM_DATA, data);
    }
    let mut lines = stack
        .get(LORE)
        .map_or_else(Vec::new, |lore| lore.lines().to_vec());
    lines.push(TextComponent::plain(lore).color(Color::Gold).italic(false));
    if let Ok(lore) = ItemLore::new(lines) {
        stack.set(LORE, lore);
    }
}

/// One waxed copy of `map`.
fn waxed_map(map: &ItemStack) -> ItemStack {
    let mut result = map.copy_with_count(1);
    mark(&mut result, MAP_LORE);
    result
}

/// Template plus honeycomb gives the waxed template, one recipe per template, under the
/// keys of the Fabric mod.
static TEMPLATE_RECIPES: LazyLock<Vec<Recipe<SmithingRecipe, SmithingRecipeInput>>> =
    LazyLock::new(|| {
        TEMPLATES
            .iter()
            .map(|(path, item)| {
                let mut waxed = ItemStack::new(LazyLock::force(item));
                mark(&mut waxed, TEMPLATE_LORE);
                Recipe::new(
                    Identifier::new("minehouse-tweaks", format!("waxed_{path}")),
                    &vanilla_recipe_types::SMITHING,
                    SmithingRecipe::Transform(SmithingTransformRecipe {
                        properties: RecipeProperties {
                            special: false,
                            placement: None,
                            recipe_book: None,
                        },
                        template: Some(Ingredient::Item(item)),
                        base: Ingredient::Item(&vanilla_items::HONEYCOMB),
                        addition: None,
                        result: ItemStackTemplate::for_registry_extension(
                            item,
                            1,
                            waxed.components_patch().clone(),
                        ),
                    }),
                )
            })
            .collect()
    });

/// Adds the waxing recipes to the registry.
pub(super) fn register(registry: &mut Registry) {
    for recipe in TEMPLATE_RECIPES.iter() {
        registry.recipes.register(recipe);
    }
}

/// How many of the waxing recipes the published registry holds.
pub(super) fn registered() -> usize {
    TEMPLATES
        .iter()
        .filter(|(path, _)| {
            REGISTRY
                .recipes
                .by_key(&Identifier::new(
                    "minehouse-tweaks",
                    format!("waxed_{path}"),
                ))
                .is_some()
        })
        .count()
}

/// Keeps waxed maps and templates from being copied, and waxes maps.
pub(super) struct WaxedItems;

/// The one observer of crafting.
pub(super) static WAXED_ITEMS: WaxedItems = WaxedItems;

impl CraftObserver for WaxedItems {
    /// A waxed map is not cloned (vanilla `map_cloning` is a transmute recipe), and a waxed
    /// template is in no shaped recipe (the ones that duplicate it with diamonds).
    fn blocks_crafting(&self, recipe: &CraftingRecipe, input: &CraftingInput) -> bool {
        let holds = |is: fn(&ItemStack) -> bool| {
            input.items.iter().any(|stack| is(stack) && is_waxed(stack))
        };
        match recipe {
            CraftingRecipe::Transmute(_) => holds(is_map),
            CraftingRecipe::Shaped(_) => holds(is_template),
            _ => false,
        }
    }

    /// A waxed template is not waxed again: the honeycomb would be wasted.
    fn blocks_smithing(&self, _recipe: &SmithingRecipe, input: &SmithingRecipeInput) -> bool {
        input.base.is(&vanilla_items::HONEYCOMB)
            && is_template(&input.template)
            && is_waxed(&input.template)
    }

    fn cartography_additional(&self, stack: &ItemStack) -> bool {
        stack.is(&vanilla_items::HONEYCOMB)
    }

    fn cartography_result(&self, map: &ItemStack, additional: &ItemStack) -> Option<ItemStack> {
        if additional.is(&vanilla_items::HONEYCOMB) {
            return Some(if is_waxed(map) {
                ItemStack::empty()
            } else {
                waxed_map(map)
            });
        }
        (additional.is(&vanilla_items::MAP) && is_waxed(map)).then(ItemStack::empty)
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{
        data_components::MapId,
        init_vanilla_registry,
        recipe::{RecipeMatches as _, UntypedRecipeRef},
    };

    use super::*;

    fn crafting(key: &'static str) -> &'static CraftingRecipe {
        REGISTRY
            .recipes
            .by_key(&Identifier::vanilla_static(key))
            .and_then(UntypedRecipeRef::downcast_data::<CraftingRecipe>)
            .expect("a vanilla crafting recipe")
    }

    fn map() -> ItemStack {
        let mut map = ItemStack::new(&vanilla_items::FILLED_MAP);
        map.set(MAP_ID, MapId::new(7));
        map
    }

    #[test]
    fn honeycomb_waxes_a_map_once_and_a_waxed_map_is_not_copied() {
        init_vanilla_registry();
        let honeycomb = ItemStack::new(&vanilla_items::HONEYCOMB);
        let empty_map = ItemStack::new(&vanilla_items::MAP);
        let paper = ItemStack::new(&vanilla_items::PAPER);
        assert!(WAXED_ITEMS.cartography_additional(&honeycomb));
        assert!(!WAXED_ITEMS.cartography_additional(&paper));

        let mut plain = ItemStack::with_count(&vanilla_items::FILLED_MAP, 3);
        plain.set(MAP_ID, MapId::new(7));
        let waxed = WAXED_ITEMS
            .cartography_result(&plain, &honeycomb)
            .expect("honeycomb gives a result");
        assert!(is_waxed(&waxed));
        assert_eq!(waxed.count(), 1);
        assert_eq!(waxed.get(MAP_ID), Some(&MapId::new(7)));
        assert_eq!(waxed.get(LORE).map(|lore| lore.lines().len()), Some(1));

        // Waxing again gives nothing, and so does an empty map.
        assert!(
            WAXED_ITEMS
                .cartography_result(&waxed, &honeycomb)
                .is_some_and(|result| result.is_empty())
        );
        assert!(
            WAXED_ITEMS
                .cartography_result(&waxed, &empty_map)
                .is_some_and(|result| result.is_empty())
        );
        // The rest is left to vanilla.
        assert!(WAXED_ITEMS.cartography_result(&waxed, &paper).is_none());
        assert!(WAXED_ITEMS.cartography_result(&plain, &empty_map).is_none());
    }

    #[test]
    fn a_waxed_map_is_not_cloned_in_the_grid() {
        init_vanilla_registry();
        let cloning = crafting("map_cloning");
        assert!(matches!(cloning, CraftingRecipe::Transmute(_)));
        let grid = |map: ItemStack| {
            CraftingInput::new(2, 1, vec![map, ItemStack::new(&vanilla_items::MAP)])
        };
        assert!(!WAXED_ITEMS.blocks_crafting(cloning, &grid(map())));
        assert!(WAXED_ITEMS.blocks_crafting(cloning, &grid(waxed_map(&map()))));
    }

    #[test]
    fn a_template_is_waxed_on_the_smithing_table_and_then_not_duplicated() {
        init_vanilla_registry();
        let template = ItemStack::new(&vanilla_items::WILD_ARMOR_TRIM_SMITHING_TEMPLATE);
        let honeycomb = ItemStack::new(&vanilla_items::HONEYCOMB);
        let recipe = TEMPLATE_RECIPES
            .iter()
            .find(|recipe| recipe.key().path == "waxed_wild_armor_trim_smithing_template")
            .expect("every template has a recipe");
        let input =
            SmithingRecipeInput::new(template.clone(), honeycomb.clone(), ItemStack::empty());
        assert!(recipe.data().matches(&input));
        assert!(!WAXED_ITEMS.blocks_smithing(recipe.data(), &input));
        // Another template does not do for this recipe.
        let other = ItemStack::new(&vanilla_items::WARD_ARMOR_TRIM_SMITHING_TEMPLATE);
        assert!(!recipe.data().matches(&SmithingRecipeInput::new(
            other,
            honeycomb.clone(),
            ItemStack::empty()
        )));

        let SmithingRecipe::Transform(transform) = recipe.data() else {
            panic!("waxing is a transform recipe");
        };
        let waxed = transform.result.apply(1, honeycomb.components_patch());
        assert!(waxed.is(&vanilla_items::WILD_ARMOR_TRIM_SMITHING_TEMPLATE));
        assert!(is_waxed(&waxed));
        assert!(!is_waxed(&template));

        // No second coat of wax.
        let again = SmithingRecipeInput::new(waxed.clone(), honeycomb, ItemStack::empty());
        assert!(recipe.data().matches(&again));
        assert!(WAXED_ITEMS.blocks_smithing(recipe.data(), &again));

        // Seven diamonds, a block and the template: two templates, unless it is waxed.
        let duplicate = crafting("wild_armor_trim_smithing_template");
        let grid = |template: ItemStack| {
            let diamond = || ItemStack::new(&vanilla_items::DIAMOND);
            CraftingInput::new(
                3,
                3,
                vec![
                    diamond(),
                    template,
                    diamond(),
                    diamond(),
                    ItemStack::new(&vanilla_items::MOSSY_COBBLESTONE),
                    diamond(),
                    diamond(),
                    diamond(),
                    diamond(),
                ],
            )
        };
        assert!(duplicate.matches(&grid(template.clone())));
        assert!(!WAXED_ITEMS.blocks_crafting(duplicate, &grid(template)));
        assert!(duplicate.matches(&grid(waxed.clone())));
        assert!(WAXED_ITEMS.blocks_crafting(duplicate, &grid(waxed)));
    }
}
