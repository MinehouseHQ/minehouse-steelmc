//! Vanilla `ScoreboardCommand`.

mod players;

use steel_protocol::packets::game::{NumberFormat, ObjectiveRenderType};
use steel_utils::{Identifier, translations};
use text_components::format::Color;
use text_components::translation::Translation;
use text_components::{Modifier as _, TextComponent};

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, CommandTextResolver, SteelArgumentType, SteelCommandContext,
        SteelCommandRuntime, argument, join_components, literal,
    },
    registration::CommandRegistration,
};
use crate::scoreboard::{
    ObjectiveCriteria, ObjectiveProperty, ScoreboardError, ScoreboardObjective,
};

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;
type Context = SteelCommandContext<CommandSource>;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("scoreboard"), |_| command())
}

fn command() -> Builder {
    literal("scoreboard")
        .then(objectives_command())
        .then(players::command())
}

fn objectives_command() -> Builder {
    literal("objectives")
        .then(literal("list").executes(list_objectives))
        .then(
            literal("add").then(
                argument("objective", ArgumentType::word()).then(
                    argument("criteria", SteelArgumentType::objective_criteria())
                        .executes(|context| {
                            let name = context.string("objective")?;
                            add_objective(context, TextComponent::plain(name.to_owned()))
                        })
                        .then(
                            argument("displayName", SteelArgumentType::component()).executes(
                                |context| add_objective(context, resolved(context, "displayName")?),
                            ),
                        ),
                ),
            ),
        )
        .then(literal("modify").then(modify_command()))
        .then(
            literal("remove").then(
                argument("objective", SteelArgumentType::objective()).executes(remove_objective),
            ),
        )
        .then(
            literal("setdisplay").then(
                argument("slot", SteelArgumentType::scoreboard_slot())
                    .executes(clear_display_slot)
                    .then(
                        argument("objective", SteelArgumentType::objective())
                            .executes(set_display_slot),
                    ),
            ),
        )
}

fn modify_command() -> Builder {
    let mut render_type = literal("rendertype");
    for value in ObjectiveRenderType::ALL {
        render_type = render_type
            .then(literal(value.name()).executes(move |context| set_render_type(context, value)));
    }
    argument("objective", SteelArgumentType::objective())
        .then(literal("displayname").then(
            argument("displayName", SteelArgumentType::component()).executes(set_display_name),
        ))
        .then(render_type)
        .then(
            literal("displayautoupdate")
                .then(argument("value", ArgumentType::bool()).executes(set_display_auto_update)),
        )
        .then(number_formats(
            literal("numberformat"),
            set_objective_format,
        ))
}

/// Vanilla `addNumberFormats`: `blank`, `fixed <contents>`,
/// `styled <style>`, or nothing to clear the format.
fn number_formats(
    top: Builder,
    callback: fn(&Context, Option<NumberFormat>) -> Result<i32, CommandSyntaxError>,
) -> Builder {
    top.then(literal("blank").executes(move |context| callback(context, Some(NumberFormat::Blank))))
        .then(literal("fixed").then(
            argument("contents", SteelArgumentType::component()).executes(move |context| {
                let contents = resolved(context, "contents")?;
                callback(context, Some(NumberFormat::Fixed(contents)))
            }),
        ))
        .then(
            literal("styled").then(argument("style", SteelArgumentType::style()).executes(
                move |context| {
                    let style = context.style("style")?.clone();
                    callback(context, Some(NumberFormat::Styled(style)))
                },
            )),
        )
        .executes(move |context| callback(context, None))
}

/// Vanilla `ComponentArgument.getResolvedComponent`.
fn resolved(context: &Context, name: &str) -> Result<TextComponent, CommandSyntaxError> {
    context
        .text_component(name)?
        .try_resolve(&CommandTextResolver::for_source(context.source()))
}

/// A scoreboard operation the command already validated.
fn scoreboard_result<T>(result: Result<T, ScoreboardError>) -> Result<T, CommandSyntaxError> {
    result.map_err(|error| CommandSyntaxError::dynamic(error.to_string()))
}

fn error(message: &'static Translation<0>) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(message))
}

fn count(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// Vanilla `ComponentUtils.formatList` with its gray comma separator.
fn format_list(values: impl IntoIterator<Item = TextComponent>) -> TextComponent {
    join_components(values, &TextComponent::plain(", ").color(Color::Gray))
}

fn modify(
    context: &Context,
    objective: &ScoreboardObjective,
    property: ObjectiveProperty,
) -> Result<(), CommandSyntaxError> {
    scoreboard_result(
        context
            .scoreboard()?
            .modify_objective(objective.name(), property),
    )
}

/// Vanilla `listObjectives`.
fn list_objectives(context: &Context) -> Result<i32, CommandSyntaxError> {
    let objectives = context.scoreboard()?.objectives();
    let message = if objectives.is_empty() {
        TextComponent::from(&translations::COMMANDS_SCOREBOARD_OBJECTIVES_LIST_EMPTY)
    } else {
        translations::COMMANDS_SCOREBOARD_OBJECTIVES_LIST_SUCCESS
            .message([
                TextComponent::plain(objectives.len().to_string()),
                format_list(
                    objectives
                        .iter()
                        .map(ScoreboardObjective::formatted_display_name),
                ),
            ])
            .component()
    };
    context.source().send_success(&message, false);
    Ok(count(objectives.len()))
}

/// Vanilla `addObjective`.
fn add_objective(
    context: &Context,
    display_name: TextComponent,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = context.scoreboard()?;
    let name = context.string("objective")?;
    if scoreboard.objective(name).is_some() {
        return Err(error(
            &translations::COMMANDS_SCOREBOARD_OBJECTIVES_ADD_DUPLICATE,
        ));
    }
    let criteria: ObjectiveCriteria = context.objective_criteria("criteria")?;
    let objective = scoreboard_result(scoreboard.add_objective(
        name,
        criteria,
        display_name,
        criteria.default_render_type(),
        false,
        None,
    ))?;
    context.source().send_success(
        &translations::COMMANDS_SCOREBOARD_OBJECTIVES_ADD_SUCCESS
            .message([objective.formatted_display_name()])
            .component(),
        true,
    );
    Ok(count(scoreboard.objective_names().len()))
}

/// Vanilla `removeObjective`.
fn remove_objective(context: &Context) -> Result<i32, CommandSyntaxError> {
    let scoreboard = context.scoreboard()?;
    let objective = context.objective("objective")?;
    scoreboard.remove_objective(objective.name());
    context.source().send_success(
        &translations::COMMANDS_SCOREBOARD_OBJECTIVES_REMOVE_SUCCESS
            .message([objective.formatted_display_name()])
            .component(),
        true,
    );
    Ok(count(scoreboard.objective_names().len()))
}

/// Vanilla `clearDisplaySlot`.
fn clear_display_slot(context: &Context) -> Result<i32, CommandSyntaxError> {
    let scoreboard = context.scoreboard()?;
    let slot = context.scoreboard_slot("slot")?;
    if scoreboard.display_objective(slot).is_none() {
        return Err(error(
            &translations::COMMANDS_SCOREBOARD_OBJECTIVES_DISPLAY_ALREADY_EMPTY,
        ));
    }
    scoreboard_result(scoreboard.set_display_objective(slot, None))?;
    context.source().send_success(
        &translations::COMMANDS_SCOREBOARD_OBJECTIVES_DISPLAY_CLEARED
            .message([slot.name()])
            .component(),
        true,
    );
    Ok(0)
}

/// Vanilla `setDisplaySlot`.
fn set_display_slot(context: &Context) -> Result<i32, CommandSyntaxError> {
    let scoreboard = context.scoreboard()?;
    let slot = context.scoreboard_slot("slot")?;
    let objective = context.objective("objective")?;
    if scoreboard
        .display_objective(slot)
        .is_some_and(|current| current.name() == objective.name())
    {
        return Err(error(
            &translations::COMMANDS_SCOREBOARD_OBJECTIVES_DISPLAY_ALREADY_SET,
        ));
    }
    scoreboard_result(scoreboard.set_display_objective(slot, Some(objective.name())))?;
    context.source().send_success(
        &translations::COMMANDS_SCOREBOARD_OBJECTIVES_DISPLAY_SET
            .message([
                TextComponent::plain(slot.name()),
                objective.display_name().clone(),
            ])
            .component(),
        true,
    );
    Ok(0)
}

/// Vanilla `setDisplayName`.
fn set_display_name(context: &Context) -> Result<i32, CommandSyntaxError> {
    let objective = context.objective("objective")?;
    let display_name = resolved(context, "displayName")?;
    if objective.display_name() != &display_name {
        modify(
            context,
            &objective,
            ObjectiveProperty::DisplayName(display_name),
        )?;
        let objective = context.objective("objective")?;
        context.source().send_success(
            &translations::COMMANDS_SCOREBOARD_OBJECTIVES_MODIFY_DISPLAYNAME
                .message([
                    TextComponent::plain(objective.name().to_owned()),
                    objective.formatted_display_name(),
                ])
                .component(),
            true,
        );
    }
    Ok(0)
}

/// Vanilla `setDisplayAutoUpdate`.
fn set_display_auto_update(context: &Context) -> Result<i32, CommandSyntaxError> {
    let objective = context.objective("objective")?;
    let value = context.boolean("value")?;
    if objective.display_auto_update() != value {
        modify(
            context,
            &objective,
            ObjectiveProperty::DisplayAutoUpdate(value),
        )?;
        let message = if value {
            &translations::COMMANDS_SCOREBOARD_OBJECTIVES_MODIFY_DISPLAY_AUTO_UPDATE_ENABLE
        } else {
            &translations::COMMANDS_SCOREBOARD_OBJECTIVES_MODIFY_DISPLAY_AUTO_UPDATE_DISABLE
        };
        // Vanilla also passes the formatted display name, which the
        // translation never shows.
        context.source().send_success(
            &message
                .message([TextComponent::plain(objective.name().to_owned())])
                .component(),
            true,
        );
    }
    Ok(0)
}

/// Vanilla `setObjectiveFormat`.
fn set_objective_format(
    context: &Context,
    number_format: Option<NumberFormat>,
) -> Result<i32, CommandSyntaxError> {
    let objective = context.objective("objective")?;
    let message = if number_format.is_some() {
        &translations::COMMANDS_SCOREBOARD_OBJECTIVES_MODIFY_OBJECTIVE_FORMAT_SET
    } else {
        &translations::COMMANDS_SCOREBOARD_OBJECTIVES_MODIFY_OBJECTIVE_FORMAT_CLEAR
    };
    modify(
        context,
        &objective,
        ObjectiveProperty::NumberFormat(number_format),
    )?;
    context.source().send_success(
        &message
            .message([TextComponent::plain(objective.name().to_owned())])
            .component(),
        true,
    );
    Ok(0)
}

/// Vanilla `setRenderType`.
fn set_render_type(
    context: &Context,
    render_type: ObjectiveRenderType,
) -> Result<i32, CommandSyntaxError> {
    let objective = context.objective("objective")?;
    if objective.render_type() != render_type {
        modify(
            context,
            &objective,
            ObjectiveProperty::RenderType(render_type),
        )?;
        context.source().send_success(
            &translations::COMMANDS_SCOREBOARD_OBJECTIVES_MODIFY_RENDERTYPE
                .message([objective.formatted_display_name()])
                .component(),
            true,
        );
    }
    Ok(0)
}
