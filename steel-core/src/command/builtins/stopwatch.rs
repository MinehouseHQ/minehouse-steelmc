//! Vanilla `StopwatchCommand`.

use steel_utils::{Identifier, java::double_to_string, translations};
use text_components::TextComponent;

use crate::command::brigadier::{
    ArgumentType, CommandNodeBuilder, CommandSyntaxError, SuggestionsBuilder,
};
use crate::command::execution::{
    CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime,
    SteelSuggestionContext, argument, literal, suggest_resources,
};
use crate::command::registration::CommandRegistration;
use crate::command::stopwatches::Stopwatches;

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;
type Context = SteelCommandContext<CommandSource>;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("stopwatch"), |_| command())
}

fn command() -> Builder {
    literal("stopwatch")
        .then(
            literal("create")
                .then(argument("id", SteelArgumentType::identifier()).executes(create)),
        )
        .then(
            literal("query").then(
                stopwatch_id()
                    .then(
                        argument("scale", ArgumentType::double(f64::MIN, f64::MAX))
                            .executes(|context| query(context, context.double("scale")?)),
                    )
                    .executes(|context| query(context, 1.0)),
            ),
        )
        .then(literal("restart").then(stopwatch_id().executes(restart)))
        .then(literal("remove").then(stopwatch_id().executes(remove)))
}

/// An `id` argument that suggests the existing stopwatches.
pub(super) fn stopwatch_id() -> Builder {
    argument("id", SteelArgumentType::identifier()).suggests(suggest_stopwatches)
}

/// Vanilla `SUGGEST_STOPWATCHES`.
fn suggest_stopwatches(
    context: &SteelSuggestionContext<'_, CommandSource>,
    builder: &mut SuggestionsBuilder<'_>,
) {
    if let Some(stopwatches) = stopwatches(context.source()) {
        suggest_resources(stopwatches.ids().iter(), builder);
    }
}

/// The source domain's stopwatches; every loaded domain has them.
fn stopwatches(source: &CommandSource) -> Option<&Stopwatches> {
    source.server().stopwatches.get(source.world().domain())
}

/// Vanilla `ERROR_DOES_NOT_EXIST`.
pub(super) fn does_not_exist(id: &Identifier) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_STOPWATCH_DOES_NOT_EXIST
            .message([id.to_string()])
            .component(),
    )
}

/// Vanilla `get(id).elapsedSeconds(currentTime())`, failing for an unknown id.
pub(super) fn elapsed_seconds(context: &Context) -> Result<f64, CommandSyntaxError> {
    let id = context.identifier("id")?;
    stopwatches(context.source())
        .and_then(|stopwatches| stopwatches.elapsed_seconds(id))
        .ok_or_else(|| does_not_exist(id))
}

/// Vanilla `createStopwatch`.
fn create(context: &Context) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?;
    let added =
        stopwatches(context.source()).is_some_and(|stopwatches| stopwatches.add(id.clone()));
    if !added {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_STOPWATCH_ALREADY_EXISTS
                .message([id.to_string()])
                .component(),
        ));
    }
    context.source().send_success(
        &translations::COMMANDS_STOPWATCH_CREATE_SUCCESS
            .message([id.to_string()])
            .component(),
        true,
    );
    Ok(1)
}

/// Vanilla `queryStopwatch`.
fn query(context: &Context, scale: f64) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?;
    let elapsed = elapsed_seconds(context)?;
    context.source().send_success(
        &translations::COMMANDS_STOPWATCH_QUERY
            .message([
                TextComponent::plain(id.to_string()),
                TextComponent::plain(double_to_string(elapsed)),
            ])
            .component(),
        true,
    );
    Ok(scaled(elapsed, scale))
}

/// Vanilla `(int)(elapsedSeconds * scale)`: Java's saturating double to int cast.
#[expect(
    clippy::cast_possible_truncation,
    reason = "Rust float to int casts saturate and map NaN to zero, like Java's"
)]
fn scaled(value: f64, scale: f64) -> i32 {
    (value * scale) as i32
}

/// Vanilla `restartStopwatch`.
fn restart(context: &Context) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?;
    if !stopwatches(context.source()).is_some_and(|stopwatches| stopwatches.restart(id)) {
        return Err(does_not_exist(id));
    }
    context.source().send_success(
        &translations::COMMANDS_STOPWATCH_RESTART_SUCCESS
            .message([id.to_string()])
            .component(),
        true,
    );
    Ok(1)
}

/// Vanilla `removeStopwatch`.
fn remove(context: &Context) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?;
    if !stopwatches(context.source()).is_some_and(|stopwatches| stopwatches.remove(id)) {
        return Err(does_not_exist(id));
    }
    context.source().send_success(
        &translations::COMMANDS_STOPWATCH_REMOVE_SUCCESS
            .message([id.to_string()])
            .component(),
        true,
    );
    Ok(1)
}
