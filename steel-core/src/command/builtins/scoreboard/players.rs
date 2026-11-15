//! Vanilla `ScoreboardCommand`'s `players` subcommands.

use steel_protocol::packets::game::NumberFormat;
use steel_utils::translations;
use text_components::TextComponent;

use super::{
    Builder, Context, count, error, format_list, number_formats, resolved, scoreboard_result,
};
use crate::command::{
    brigadier::{ArgumentType, CommandSyntaxError, SuggestionsBuilder},
    execution::{
        CommandSource, ElementType, ResponseTracker, ScoreHolderWildcard, ScoreOperation,
        SteelArgumentType, SteelSuggestionContext, argument, literal, suggest_list,
    },
};
use crate::scoreboard::{CustomCriteria, ObjectiveCriteria, ScoreHolder};

type Tracker = ResponseTracker<ScoreHolder>;

const TRIGGER: ObjectiveCriteria = ObjectiveCriteria::Custom(CustomCriteria::Trigger);

fn plain(value: i32) -> TextComponent {
    TextComponent::plain(value.to_string())
}

/// `ScoreHolderArgument.getNamesWithDefaultWildcard`.
fn targets(context: &Context, name: &str) -> Result<Vec<ScoreHolder>, CommandSyntaxError> {
    context.score_holders(name, ScoreHolderWildcard::Tracked)
}

fn holders() -> Builder {
    argument("targets", SteelArgumentType::score_holders())
}

fn objective() -> Builder {
    argument("objective", SteelArgumentType::objective())
}

pub(super) fn command() -> Builder {
    literal("players")
        .then(
            literal("list").executes(list_tracked_players).then(
                argument("target", SteelArgumentType::score_holder())
                    .executes(list_tracked_player_scores),
            ),
        )
        .then(literal("set").then(holders().then(objective().then(
            argument("score", ArgumentType::integer(i32::MIN, i32::MAX)).executes(set_score),
        ))))
        .then(
            literal("get").then(
                argument("target", SteelArgumentType::score_holder())
                    .then(objective().executes(get_score)),
            ),
        )
        .then(
            literal("add").then(
                holders().then(
                    objective().then(
                        argument("score", ArgumentType::integer(0, i32::MAX))
                            .executes(|context| add_score(context, false)),
                    ),
                ),
            ),
        )
        .then(
            literal("remove").then(
                holders().then(
                    objective().then(
                        argument("score", ArgumentType::integer(0, i32::MAX))
                            .executes(|context| add_score(context, true)),
                    ),
                ),
            ),
        )
        .then(
            literal("reset").then(
                holders()
                    .executes(reset_scores)
                    .then(objective().executes(reset_score)),
            ),
        )
        .then(
            literal("enable").then(
                holders().then(
                    objective()
                        .suggests(suggest_triggers)
                        .executes(enable_trigger),
                ),
            ),
        )
        .then(display_command())
        .then(operation_command())
}

fn display_command() -> Builder {
    literal("display")
        .then(
            literal("name").then(
                holders().then(
                    objective()
                        .then(argument("name", SteelArgumentType::component()).executes(
                            |context| set_score_display(context, Some(resolved(context, "name")?)),
                        ))
                        .executes(|context| set_score_display(context, None)),
                ),
            ),
        )
        .then(
            literal("numberformat")
                .then(holders().then(number_formats(objective(), set_score_number_format))),
        )
}

fn operation_command() -> Builder {
    literal("operation").then(
        holders().then(
            argument("targetObjective", SteelArgumentType::objective()).then(
                argument("operation", SteelArgumentType::operation()).then(
                    argument("source", SteelArgumentType::score_holders()).then(
                        argument("sourceObjective", SteelArgumentType::objective())
                            .executes(perform_operation),
                    ),
                ),
            ),
        ),
    )
}

/// Vanilla `suggestTriggers`: trigger objectives one of the targets may
/// still be enabled for.
fn suggest_triggers(
    context: &SteelSuggestionContext<'_, CommandSource>,
    builder: &mut SuggestionsBuilder<'_>,
) {
    let Ok(targets) = context.score_holders("targets") else {
        return;
    };
    let Some(scoreboard) = context
        .source()
        .server()
        .scoreboards
        .get(context.source().world().domain())
    else {
        return;
    };
    let names: Vec<String> = scoreboard
        .objectives_with_criteria(&TRIGGER)
        .into_iter()
        .filter(|objective| {
            targets.iter().any(|target| {
                scoreboard
                    .score_entry(target, objective)
                    .is_none_or(|score| score.is_locked())
            })
        })
        .map(|objective| objective.name().to_owned())
        .collect();
    suggest_list(builder, names);
}

/// Vanilla `getScore`.
fn get_score(context: &Context) -> Result<i32, CommandSyntaxError> {
    let target = context.score_holder("target")?;
    let objective = context.objective("objective")?;
    let Some(score) = context.scoreboard()?.score(&target, &objective) else {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_SCOREBOARD_PLAYERS_GET_NULL
                .message([
                    TextComponent::plain(objective.name().to_owned()),
                    target.feedback_display_name(),
                ])
                .component(),
        ));
    };
    context.source().send_success(
        &translations::COMMANDS_SCOREBOARD_PLAYERS_GET_SUCCESS
            .message([
                target.feedback_display_name(),
                plain(score),
                objective.formatted_display_name(),
            ])
            .component(),
        false,
    );
    Ok(score)
}

/// Vanilla `performOperation`.
fn perform_operation(context: &Context) -> Result<i32, CommandSyntaxError> {
    let scoreboard = context.scoreboard()?;
    let target_holders = targets(context, "targets")?;
    let target_objective = context.writable_objective("targetObjective")?;
    let operation = context.operation("operation")?;
    let sources = targets(context, "source")?;
    let source_objective = context.objective("sourceObjective")?;
    let mut tracker = Tracker::default();
    for target in &target_holders {
        // Vanilla's `getOrCreatePlayerScore` creates the target score before
        // any source is read; `set_score` creates it and tells clients.
        let mut value = scoreboard.score(target, &target_objective).unwrap_or(0);
        for source in &sources {
            let source_value =
                scoreboard_result(scoreboard.get_or_create_score(source, &source_objective))?;
            let new_value = match operation.apply(value, source_value) {
                Ok(new_value) => new_value,
                Err(error) => {
                    scoreboard_result(scoreboard.get_or_create_score(target, &target_objective))?;
                    return Err(error);
                }
            };
            scoreboard_result(scoreboard.set_score(target, &target_objective, new_value))?;
            if operation == ScoreOperation::Swap {
                scoreboard_result(scoreboard.set_score(source, &source_objective, value))?;
            }
            value = scoreboard.score(target, &target_objective).unwrap_or(0);
        }
        tracker.track(target, value);
    }
    tracker.send_feedback(
        context.source(),
        true,
        ElementType::Any,
        None,
        |holder, total| {
            translations::COMMANDS_SCOREBOARD_PLAYERS_OPERATION_SUCCESS_SINGLE
                .message([
                    target_objective.formatted_display_name(),
                    holder.feedback_display_name(),
                    plain(total),
                ])
                .component()
        },
        |holders, _| {
            translations::COMMANDS_SCOREBOARD_PLAYERS_OPERATION_SUCCESS_MULTIPLE
                .message([target_objective.formatted_display_name(), plain(holders)])
                .component()
        },
    )
}

/// Vanilla `enableTrigger`.
fn enable_trigger(context: &Context) -> Result<i32, CommandSyntaxError> {
    let objective = context.objective("objective")?;
    if objective.criteria() != &TRIGGER {
        return Err(error(
            &translations::COMMANDS_SCOREBOARD_PLAYERS_ENABLE_INVALID,
        ));
    }
    let scoreboard = context.scoreboard()?;
    let mut tracker = Tracker::default();
    for target in targets(context, "targets")? {
        // A missing score is created locked, so it is unlocked here too.
        if scoreboard
            .score_entry(&target, &objective)
            .is_none_or(|score| score.is_locked())
        {
            scoreboard_result(scoreboard.set_score_locked(&target, &objective, false))?;
            tracker.track_one(&target);
        }
    }
    tracker.send_feedback(
        context.source(),
        true,
        ElementType::NonZero,
        Some(|| error(&translations::COMMANDS_SCOREBOARD_PLAYERS_ENABLE_FAILED)),
        |holder, _| {
            translations::COMMANDS_SCOREBOARD_PLAYERS_ENABLE_SUCCESS_SINGLE
                .message([
                    objective.formatted_display_name(),
                    holder.feedback_display_name(),
                ])
                .component()
        },
        |holders, _| {
            translations::COMMANDS_SCOREBOARD_PLAYERS_ENABLE_SUCCESS_MULTIPLE
                .message([objective.formatted_display_name(), plain(holders)])
                .component()
        },
    )
}

/// Vanilla `resetScores`.
fn reset_scores(context: &Context) -> Result<i32, CommandSyntaxError> {
    let scoreboard = context.scoreboard()?;
    let mut tracker = Tracker::default();
    for target in targets(context, "targets")? {
        scoreboard.reset_all_scores(&target);
        tracker.track_one(&target);
    }
    tracker.send_feedback(
        context.source(),
        true,
        ElementType::NonZero,
        None,
        |holder, _| {
            translations::COMMANDS_SCOREBOARD_PLAYERS_RESET_ALL_SINGLE
                .message([holder.feedback_display_name()])
                .component()
        },
        |holders, _| {
            translations::COMMANDS_SCOREBOARD_PLAYERS_RESET_ALL_MULTIPLE
                .message([plain(holders)])
                .component()
        },
    )
}

/// Vanilla `resetScore`.
fn reset_score(context: &Context) -> Result<i32, CommandSyntaxError> {
    let scoreboard = context.scoreboard()?;
    let objective = context.objective("objective")?;
    let mut tracker = Tracker::default();
    for target in targets(context, "targets")? {
        scoreboard.reset_score(&target, &objective);
        tracker.track_one(&target);
    }
    tracker.send_feedback(
        context.source(),
        true,
        ElementType::NonZero,
        None,
        |holder, _| {
            translations::COMMANDS_SCOREBOARD_PLAYERS_RESET_SPECIFIC_SINGLE
                .message([
                    objective.formatted_display_name(),
                    holder.feedback_display_name(),
                ])
                .component()
        },
        |holders, _| {
            translations::COMMANDS_SCOREBOARD_PLAYERS_RESET_SPECIFIC_MULTIPLE
                .message([objective.formatted_display_name(), plain(holders)])
                .component()
        },
    )
}

/// Vanilla `setScore`.
fn set_score(context: &Context) -> Result<i32, CommandSyntaxError> {
    let scoreboard = context.scoreboard()?;
    let holders = targets(context, "targets")?;
    let objective = context.writable_objective("objective")?;
    let value = context.integer("score")?;
    let mut tracker = Tracker::default();
    for target in &holders {
        scoreboard_result(scoreboard.set_score(target, &objective, value))?;
        tracker.track(target, value);
    }
    tracker.send_feedback(
        context.source(),
        true,
        ElementType::NonZero,
        None,
        |holder, _| {
            translations::COMMANDS_SCOREBOARD_PLAYERS_SET_SUCCESS_SINGLE
                .message([
                    objective.formatted_display_name(),
                    holder.feedback_display_name(),
                    plain(value),
                ])
                .component()
        },
        |holders, _| {
            translations::COMMANDS_SCOREBOARD_PLAYERS_SET_SUCCESS_MULTIPLE
                .message([
                    objective.formatted_display_name(),
                    plain(holders),
                    plain(value),
                ])
                .component()
        },
    )
}

/// Vanilla `addScore` and `removeScore`.
fn add_score(context: &Context, remove: bool) -> Result<i32, CommandSyntaxError> {
    let scoreboard = context.scoreboard()?;
    let holders = targets(context, "targets")?;
    let objective = context.writable_objective("objective")?;
    let value = context.integer("score")?;
    let mut tracker = Tracker::default();
    for target in &holders {
        let amount = if remove { value.wrapping_neg() } else { value };
        let new_value = scoreboard_result(scoreboard.add_score(target, &objective, amount))?;
        tracker.track(target, new_value);
    }
    let (single, multiple) = if remove {
        (
            &translations::COMMANDS_SCOREBOARD_PLAYERS_REMOVE_SUCCESS_SINGLE,
            &translations::COMMANDS_SCOREBOARD_PLAYERS_REMOVE_SUCCESS_MULTIPLE,
        )
    } else {
        (
            &translations::COMMANDS_SCOREBOARD_PLAYERS_ADD_SUCCESS_SINGLE,
            &translations::COMMANDS_SCOREBOARD_PLAYERS_ADD_SUCCESS_MULTIPLE,
        )
    };
    tracker.send_feedback(
        context.source(),
        true,
        ElementType::Any,
        None,
        |holder, total| {
            single
                .message([
                    plain(value),
                    objective.formatted_display_name(),
                    holder.feedback_display_name(),
                    plain(total),
                ])
                .component()
        },
        |holders, _| {
            multiple
                .message([
                    plain(value),
                    objective.formatted_display_name(),
                    plain(holders),
                ])
                .component()
        },
    )
}

/// Vanilla `setScoreDisplay`.
fn set_score_display(
    context: &Context,
    display: Option<TextComponent>,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = context.scoreboard()?;
    let objective = context.objective("objective")?;
    let mut tracker = Tracker::default();
    for target in targets(context, "targets")? {
        scoreboard_result(scoreboard.set_score_display(&target, &objective, display.clone()))?;
        tracker.track_one(&target);
    }
    let name = objective.formatted_display_name();
    match display {
        None => tracker.send_feedback(
            context.source(),
            true,
            ElementType::NonZero,
            None,
            |holder, _| {
                translations::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NAME_CLEAR_SUCCESS_SINGLE
                    .message([holder.feedback_display_name(), name.clone()])
                    .component()
            },
            |holders, _| {
                translations::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NAME_CLEAR_SUCCESS_MULTIPLE
                    .message([plain(holders), name.clone()])
                    .component()
            },
        ),
        Some(display) => tracker.send_feedback(
            context.source(),
            true,
            ElementType::NonZero,
            None,
            |holder, _| {
                translations::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NAME_SET_SUCCESS_SINGLE
                    .message([
                        display.clone(),
                        holder.feedback_display_name(),
                        name.clone(),
                    ])
                    .component()
            },
            |holders, _| {
                translations::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NAME_SET_SUCCESS_MULTIPLE
                    .message([display.clone(), plain(holders), name.clone()])
                    .component()
            },
        ),
    }
}

/// Vanilla `setScoreNumberFormat`.
fn set_score_number_format(
    context: &Context,
    number_format: Option<NumberFormat>,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = context.scoreboard()?;
    let objective = context.objective("objective")?;
    let mut tracker = Tracker::default();
    let set = number_format.is_some();
    for target in targets(context, "targets")? {
        scoreboard_result(scoreboard.set_score_number_format(
            &target,
            &objective,
            number_format.clone(),
        ))?;
        tracker.track_one(&target);
    }
    let (single, multiple) = if set {
        (
            &translations::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NUMBER_FORMAT_SET_SUCCESS_SINGLE,
            &translations::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NUMBER_FORMAT_SET_SUCCESS_MULTIPLE,
        )
    } else {
        (
            &translations::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NUMBER_FORMAT_CLEAR_SUCCESS_SINGLE,
            &translations::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NUMBER_FORMAT_CLEAR_SUCCESS_MULTIPLE,
        )
    };
    tracker.send_feedback(
        context.source(),
        true,
        ElementType::NonZero,
        None,
        |holder, _| {
            single
                .message([
                    holder.feedback_display_name(),
                    objective.formatted_display_name(),
                ])
                .component()
        },
        |holders, _| {
            multiple
                .message([plain(holders), objective.formatted_display_name()])
                .component()
        },
    )
}

/// Vanilla `listTrackedPlayers`.
fn list_tracked_players(context: &Context) -> Result<i32, CommandSyntaxError> {
    let holders = context.scoreboard()?.tracked_holders();
    let message = if holders.is_empty() {
        TextComponent::from(&translations::COMMANDS_SCOREBOARD_PLAYERS_LIST_EMPTY)
    } else {
        translations::COMMANDS_SCOREBOARD_PLAYERS_LIST_SUCCESS
            .message([
                plain(count(holders.len())),
                format_list(holders.iter().map(ScoreHolder::feedback_display_name)),
            ])
            .component()
    };
    context.source().send_success(&message, false);
    Ok(count(holders.len()))
}

/// Vanilla `listTrackedPlayerScores`.
fn list_tracked_player_scores(context: &Context) -> Result<i32, CommandSyntaxError> {
    let holder = context.score_holder("target")?;
    let scores = context.scoreboard()?.holder_scores(&holder);
    let source = context.source();
    if scores.is_empty() {
        source.send_success(
            &translations::COMMANDS_SCOREBOARD_PLAYERS_LIST_ENTITY_EMPTY
                .message([holder.feedback_display_name()])
                .component(),
            false,
        );
    } else {
        source.send_success(
            &translations::COMMANDS_SCOREBOARD_PLAYERS_LIST_ENTITY_SUCCESS
                .message([holder.feedback_display_name(), plain(count(scores.len()))])
                .component(),
            false,
        );
        for (objective, value) in &scores {
            source.send_success(
                &translations::COMMANDS_SCOREBOARD_PLAYERS_LIST_ENTITY_ENTRY
                    .message([objective.formatted_display_name(), plain(*value)])
                    .component(),
                false,
            );
        }
    }
    Ok(count(scores.len()))
}
