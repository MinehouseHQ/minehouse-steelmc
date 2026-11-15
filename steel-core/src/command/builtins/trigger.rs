//! Vanilla `TriggerCommand`.

use steel_utils::{Identifier, translations};
use text_components::TextComponent;
use text_components::translation::Translation;

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError, SuggestionsBuilder},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime,
        SteelSuggestionContext, argument, entity_holder, literal, suggest_list,
    },
    registration::CommandRegistration,
};
use crate::scoreboard::{
    CustomCriteria, ObjectiveCriteria, ScoreHolder, Scoreboard, ScoreboardError,
    ScoreboardObjective,
};

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;
type Context = SteelCommandContext<CommandSource>;

const TRIGGER: ObjectiveCriteria = ObjectiveCriteria::Custom(CustomCriteria::Trigger);

/// Vanilla registers `/trigger` without a permission requirement.
pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("trigger"), |_| command()).default_access()
}

fn command() -> Builder {
    literal("trigger").then(
        argument("objective", SteelArgumentType::objective())
            .suggests(suggest_objectives)
            .executes(simple_trigger)
            .then(literal("add").then(
                argument("value", ArgumentType::integer(i32::MIN, i32::MAX)).executes(add_value),
            ))
            .then(literal("set").then(
                argument("value", ArgumentType::integer(i32::MIN, i32::MAX)).executes(set_value),
            )),
    )
}

/// Vanilla `suggestObjectives`: the trigger objectives the source entity
/// is enabled for.
fn suggest_objectives(
    context: &SteelSuggestionContext<'_, CommandSource>,
    builder: &mut SuggestionsBuilder<'_>,
) {
    let source = context.source();
    let Some(entity) = source.entity() else {
        return;
    };
    let Some(scoreboard) = source.server().scoreboards.get(source.world().domain()) else {
        return;
    };
    let holder = entity_holder(&**entity);
    let names: Vec<String> = scoreboard
        .objectives()
        .into_iter()
        .filter(|objective| objective.criteria() == &TRIGGER)
        .filter(|objective| {
            scoreboard
                .score_entry(&holder, objective)
                .is_some_and(|score| !score.is_locked())
        })
        .map(|objective| objective.name().to_owned())
        .collect();
    suggest_list(builder, names);
}

fn error(message: &'static Translation<0>) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(message))
}

fn scoreboard_result<T>(result: Result<T, ScoreboardError>) -> Result<T, CommandSyntaxError> {
    result.map_err(|error| CommandSyntaxError::dynamic(error.to_string()))
}

/// The source player, its holder and the objective, vanilla's
/// `getPlayerOrException` and `ObjectiveArgument.getObjective`.
fn target(context: &Context) -> Result<(ScoreHolder, ScoreboardObjective), CommandSyntaxError> {
    let Some(player) = context.source().player() else {
        return Err(error(&translations::PERMISSIONS_REQUIRES_PLAYER));
    };
    let holder = entity_holder(&**player);
    Ok((holder, context.objective("objective")?))
}

/// Vanilla `getScore`: a trigger score the holder is enabled for, locked
/// again before it changes.
fn locked_score(
    scoreboard: &Scoreboard,
    holder: &ScoreHolder,
    objective: &ScoreboardObjective,
) -> Result<(), CommandSyntaxError> {
    if objective.criteria() != &TRIGGER {
        return Err(error(&translations::COMMANDS_TRIGGER_FAILED_INVALID));
    }
    if scoreboard
        .score_entry(holder, objective)
        .is_none_or(|score| score.is_locked())
    {
        return Err(error(&translations::COMMANDS_TRIGGER_FAILED_UNPRIMED));
    }
    scoreboard_result(scoreboard.set_score_locked(holder, objective, true))
}

/// Vanilla `addValue`.
fn add_value(context: &Context) -> Result<i32, CommandSyntaxError> {
    let (holder, objective) = target(context)?;
    let amount = context.integer("value")?;
    let scoreboard = context.scoreboard()?;
    locked_score(scoreboard, &holder, &objective)?;
    let value = scoreboard_result(scoreboard.add_score(&holder, &objective, amount))?;
    context.source().send_success(
        &translations::COMMANDS_TRIGGER_ADD_SUCCESS
            .message([
                objective.formatted_display_name(),
                TextComponent::plain(amount.to_string()),
            ])
            .component(),
        true,
    );
    Ok(value)
}

/// Vanilla `setValue`.
fn set_value(context: &Context) -> Result<i32, CommandSyntaxError> {
    let (holder, objective) = target(context)?;
    let amount = context.integer("value")?;
    let scoreboard = context.scoreboard()?;
    locked_score(scoreboard, &holder, &objective)?;
    scoreboard_result(scoreboard.set_score(&holder, &objective, amount))?;
    context.source().send_success(
        &translations::COMMANDS_TRIGGER_SET_SUCCESS
            .message([
                objective.formatted_display_name(),
                TextComponent::plain(amount.to_string()),
            ])
            .component(),
        true,
    );
    Ok(amount)
}

/// Vanilla `simpleTrigger`.
fn simple_trigger(context: &Context) -> Result<i32, CommandSyntaxError> {
    let (holder, objective) = target(context)?;
    let scoreboard = context.scoreboard()?;
    locked_score(scoreboard, &holder, &objective)?;
    let value = scoreboard_result(scoreboard.add_score(&holder, &objective, 1))?;
    context.source().send_success(
        &translations::COMMANDS_TRIGGER_SIMPLE_SUCCESS
            .message([objective.formatted_display_name()])
            .component(),
        true,
    );
    Ok(value)
}
