//! Vanilla `ScheduleCommand`.

use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError, SuggestionsBuilder},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime,
        SteelSuggestionContext, argument, literal, suggest_resources,
    },
    function::{CommandFunction, TimerCallback, TimerQueue},
    registration::CommandRegistration,
};

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;
type Context = SteelCommandContext<CommandSource>;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("schedule"), |_| command())
}

fn command() -> Builder {
    literal("schedule")
        .then(
            literal("function").then(
                argument("function", SteelArgumentType::function()).then(
                    argument("time", SteelArgumentType::time(0))
                        .executes(|context| schedule(context, true))
                        .then(literal("append").executes(|context| schedule(context, false)))
                        .then(literal("replace").executes(|context| schedule(context, true))),
                ),
            ),
        )
        .then(
            literal("clear").then(
                argument("function", SteelArgumentType::identifier())
                    .suggests(suggest_schedule)
                    .executes(remove),
            ),
        )
}

/// Vanilla `SUGGEST_SCHEDULE`.
///
/// Vanilla maps every id through `Identifier.parse`, which throws on a tag's
/// `#` id and leaves the player with no suggestions at all; Steel suggests
/// the ids that parse instead.
fn suggest_schedule(
    context: &SteelSuggestionContext<'_, CommandSource>,
    builder: &mut SuggestionsBuilder<'_>,
) {
    let Some(queue) = queue(context.source()) else {
        return;
    };
    let ids = queue
        .event_ids()
        .into_iter()
        .filter_map(|id| id.parse::<Identifier>().ok())
        .collect::<Vec<_>>();
    suggest_resources(ids.iter(), builder);
}

/// The source's domain queue; every loaded domain has one.
fn queue(source: &CommandSource) -> Option<&TimerQueue> {
    source
        .server()
        .scheduled_events
        .get(source.world().domain())
}

/// Vanilla `FunctionArgument.getFunctionOrTag`: an unknown function is an
/// error, an unknown tag is empty. Also says whether the function is a macro.
fn callback(context: &Context) -> Result<(TimerCallback, bool), CommandSyntaxError> {
    let reference = context.function_reference("function")?;
    if reference.tag {
        return Ok((TimerCallback::FunctionTag(reference.id.clone()), false));
    }
    let library = context
        .source()
        .server()
        .functions
        .library(context.source().world().domain())
        .unwrap_or_default();
    let function = library.function(&reference.id).ok_or_else(|| {
        CommandSyntaxError::dynamic(
            translations::ARGUMENTS_FUNCTION_UNKNOWN
                .message([reference.id.to_string()])
                .component(),
        )
    })?;
    let is_macro = matches!(**function, CommandFunction::Macro(_));
    Ok((TimerCallback::Function(reference.id.clone()), is_macro))
}

/// Vanilla `ScheduleCommand.schedule`.
fn schedule(context: &Context, replace: bool) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let (callback, is_macro) = callback(context)?;
    let time = context.time("time")?;
    if time == 0 {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_SCHEDULE_SAME_TICK.msg().component(),
        ));
    }
    if is_macro {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_SCHEDULE_MACRO.msg().component(),
        ));
    }
    let tick_time = source.world().game_time() + i64::from(time);
    let (schedule_id, callback_id, created) = match &callback {
        TimerCallback::Function(id) => (
            id.to_string(),
            id,
            &translations::COMMANDS_SCHEDULE_CREATED_FUNCTION,
        ),
        TimerCallback::FunctionTag(id) => (
            format!("#{id}"),
            id,
            &translations::COMMANDS_SCHEDULE_CREATED_TAG,
        ),
    };
    let message = created
        .message([
            TextComponent::plain(callback_id.to_string()),
            TextComponent::plain(time.to_string()),
            TextComponent::plain(tick_time.to_string()),
        ])
        .component();
    if let Some(queue) = queue(source) {
        if replace {
            queue.remove(&schedule_id);
        }
        queue.schedule(schedule_id, tick_time, callback);
    }
    source.send_success(&message, true);
    Ok(floor_mod_int_max(tick_time))
}

/// Java's `Math.floorMod(tickTime, Integer.MAX_VALUE)`.
fn floor_mod_int_max(value: i64) -> i32 {
    let reduced = value.rem_euclid(i64::from(i32::MAX));
    i32::try_from(reduced).unwrap_or(i32::MAX)
}

/// Vanilla `ScheduleCommand.remove`.
fn remove(context: &Context) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let id = context.identifier("function")?.to_string();
    let count = queue(source).map_or(0, |queue| queue.remove(&id));
    if count == 0 {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_SCHEDULE_CLEARED_FAILURE
                .message([id])
                .component(),
        ));
    }
    let count = i32::try_from(count).unwrap_or(i32::MAX);
    source.send_success(
        &translations::COMMANDS_SCHEDULE_CLEARED_SUCCESS
            .message([
                TextComponent::plain(count.to_string()),
                TextComponent::plain(id),
            ])
            .component(),
        true,
    );
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::floor_mod_int_max;

    #[test]
    fn the_result_wraps_like_java_floor_mod() {
        assert_eq!(floor_mod_int_max(20), 20);
        assert_eq!(floor_mod_int_max(i64::from(i32::MAX)), 0);
        assert_eq!(floor_mod_int_max(i64::from(i32::MAX) + 5), 5);
        assert_eq!(floor_mod_int_max(-1), i32::MAX - 1);
    }
}
