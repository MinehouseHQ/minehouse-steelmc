//! Vanilla `RecipeCraftingHolder`: the recipe a result came from, unlocked for the
//! player who takes the result.

use steel_registry::REGISTRY;
use steel_registry::item_stack::ItemStack;
use steel_registry::recipe::UntypedRecipeRef;
use steel_registry::vanilla_game_rules::LIMITED_CRAFTING;
use steel_utils::Identifier;

use crate::inventory::recipe_displays::is_special;
use crate::player::Player;
use crate::player::advancement::criteria::triggers::{CRAFTER_RECIPE_CRAFTED, RECIPE_CRAFTED};
use crate::player::advancement::criteria::{Conditions, trigger};

/// Vanilla `RecipeCraftingHolder.setRecipeUsed(ServerPlayer, RecipeHolder)`'s check:
/// with `limitedCrafting` on, only special or unlocked recipes may be used.
#[must_use]
pub fn may_use_recipe(player: &Player, recipe: UntypedRecipeRef) -> bool {
    is_special(recipe)
        || !player.get_world().get_game_rule(&LIMITED_CRAFTING)
        || player.knows_recipe(recipe.key())
}

/// The recipe-taking half of vanilla `RecipeCraftingHolder.awardUsedRecipes`: the
/// recipe to award, forgetting it unless it is special. The rest, which reaches
/// the player, is [`Player::award_used_recipe`], run once the containers are unlocked.
pub fn take_recipe_to_award(recipe_used: &mut Option<Identifier>) -> Option<UntypedRecipeRef> {
    let recipe = REGISTRY.recipes.by_key(recipe_used.as_ref()?)?;
    if !is_special(recipe) {
        *recipe_used = None;
    }
    Some(recipe)
}

impl Player {
    /// The player half of vanilla `RecipeCraftingHolder.awardUsedRecipes`.
    pub fn award_used_recipe(&self, recipe: UntypedRecipeRef, items: &[ItemStack]) {
        self.trigger_recipe_crafted(recipe.key(), items);
        if !is_special(recipe) {
            self.award_recipes(&[recipe]);
        }
    }

    /// Vanilla `Player.triggerRecipeCrafted` (`RecipeCraftedTrigger.trigger`).
    pub fn trigger_recipe_crafted(&self, recipe: &Identifier, items: &[ItemStack]) {
        trigger(self, &RECIPE_CRAFTED, |conditions| {
            recipe_crafted_matches(conditions, recipe, items)
        });
    }

    /// Vanilla `CriteriaTriggers.CRAFTER_RECIPE_CRAFTED.trigger`, a
    /// `RecipeCraftedTrigger` fired by a crafter.
    pub fn trigger_crafter_recipe_crafted(&self, recipe: &Identifier, items: &[ItemStack]) {
        trigger(self, &CRAFTER_RECIPE_CRAFTED, |conditions| {
            recipe_crafted_matches(conditions, recipe, items)
        });
    }
}

/// Vanilla `RecipeCraftedTrigger.TriggerInstance.matches`: the recipe is listed and
/// every ingredient predicate takes a different used item.
fn recipe_crafted_matches(
    conditions: &Conditions,
    recipe: &Identifier,
    items: &[ItemStack],
) -> bool {
    if !conditions
        .recipes
        .as_ref()
        .is_some_and(|recipes| recipes.0.contains(recipe))
    {
        return false;
    }
    let mut remaining: Vec<&ItemStack> = items.iter().collect();
    conditions.ingredients.iter().all(|predicate| {
        remaining
            .iter()
            .position(|stack| predicate.test(stack))
            .map(|index| remaining.remove(index))
            .is_some()
    })
}

#[cfg(test)]
mod tests {
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{init_vanilla_registry, vanilla_items};
    use steel_utils::Identifier;

    use super::recipe_crafted_matches;
    use crate::player::advancement::criteria::Conditions;

    fn conditions(json: &str) -> Conditions {
        serde_json::from_str(json).expect("vanilla recipe_crafted conditions decode")
    }

    #[test]
    fn each_ingredient_predicate_takes_a_different_item() {
        init_vanilla_registry();
        let recipe = Identifier::vanilla_static("chest");
        let two_logs = conditions(
            r#"{"recipes":"minecraft:chest","ingredients":[{"items":"minecraft:oak_log"},{"items":"minecraft:oak_log"}]}"#,
        );
        let log = || ItemStack::new(&vanilla_items::OAK_LOG);
        let empty = ItemStack::empty;

        assert!(recipe_crafted_matches(
            &two_logs,
            &recipe,
            &[log(), empty(), log()]
        ));
        assert!(!recipe_crafted_matches(
            &two_logs,
            &recipe,
            &[log(), empty()]
        ));
        assert!(!recipe_crafted_matches(
            &two_logs,
            &Identifier::vanilla_static("barrel"),
            &[log(), log()]
        ));

        let any = conditions(r#"{"recipes":["minecraft:barrel","minecraft:chest"]}"#);
        assert!(recipe_crafted_matches(&any, &recipe, &[]));
    }
}
