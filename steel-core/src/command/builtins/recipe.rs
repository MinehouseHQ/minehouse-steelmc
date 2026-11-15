//! Vanilla `RecipeCommand`.

use std::sync::Arc;

use steel_registry::recipe::UntypedRecipeRef;
use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, ElementType, ResponseTracker, SteelArgumentType, SteelCommandContext,
        SteelCommandRuntime, argument, literal,
    },
    registration::CommandRegistration,
};
use crate::entity::Entity as _;
use crate::inventory::recipe_displays::RECIPE_DISPLAYS;
use crate::player::Player;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("recipe"), |_| command())
}

/// Whether a `/recipe` branch gives or takes recipes.
#[derive(Clone, Copy)]
enum Action {
    Give,
    Take,
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("recipe")
        .then(literal("give").then(targets(Action::Give)))
        .then(literal("take").then(targets(Action::Take)))
}

fn targets(action: Action) -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    argument("targets", SteelArgumentType::players())
        .then(
            argument("recipe", SteelArgumentType::learnable_recipe()).executes(move |context| {
                let recipe = context.learnable_recipe("recipe")?;
                perform(context, action, &[recipe])
            }),
        )
        .then(
            literal("*")
                .executes(move |context| perform(context, action, RECIPE_DISPLAYS.learnable())),
        )
}

fn perform(
    context: &SteelCommandContext<CommandSource>,
    action: Action,
    recipes: &[UntypedRecipeRef],
) -> Result<i32, CommandSyntaxError> {
    let players = context.players("targets")?;
    change_recipes(context.source(), &players, action, recipes)
}

/// Vanilla `giveRecipes` and `takeRecipes`.
fn change_recipes(
    source: &CommandSource,
    players: &[Arc<Player>],
    action: Action,
    recipes: &[UntypedRecipeRef],
) -> Result<i32, CommandSyntaxError> {
    let mut tracker = ResponseTracker::default();
    for player in players {
        let changed = match action {
            Action::Give => player.award_recipes(recipes),
            Action::Take => player.reset_recipes(recipes),
        };
        tracker.track(player, i32::try_from(changed).unwrap_or(i32::MAX));
    }
    let (failed, single, multiple) = match action {
        Action::Give => (
            give_failed as fn() -> CommandSyntaxError,
            &translations::COMMANDS_RECIPE_GIVE_SUCCESS_SINGLE,
            &translations::COMMANDS_RECIPE_GIVE_SUCCESS_MULTIPLE,
        ),
        Action::Take => (
            take_failed as fn() -> CommandSyntaxError,
            &translations::COMMANDS_RECIPE_TAKE_SUCCESS_SINGLE,
            &translations::COMMANDS_RECIPE_TAKE_SUCCESS_MULTIPLE,
        ),
    };
    tracker.send_feedback(
        source,
        true,
        ElementType::NonZero,
        Some(failed),
        |player: &Arc<Player>, total| {
            single
                .message([plain(total), player.display_name()])
                .component()
        },
        |count, total| multiple.message([plain(total), plain(count)]).component(),
    )
}

fn plain(value: i32) -> TextComponent {
    TextComponent::plain(value.to_string())
}

fn give_failed() -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(
        &translations::COMMANDS_RECIPE_GIVE_FAILED,
    ))
}

fn take_failed() -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(
        &translations::COMMANDS_RECIPE_TAKE_FAILED,
    ))
}
