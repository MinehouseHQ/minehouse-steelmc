//! Vanilla `WaypointCommand`.

use steel_protocol::packets::game::{DEFAULT_WAYPOINT_STYLE, WaypointIcon};
use steel_utils::{Identifier, translations};
use text_components::{
    Modifier as _, TextComponent,
    format::Color,
    interactivity::{ClickEvent, HoverEvent},
};

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        join_components, literal,
    },
    registration::CommandRegistration,
};
use crate::entity::SharedEntity;
use crate::scoreboard::TeamColor;

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;
type Context = SteelCommandContext<CommandSource>;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("waypoint"), |_| command())
}

fn command() -> Builder {
    literal("waypoint")
        .then(literal("list").executes(|context| Ok(list_waypoints(context))))
        .then(
            literal("modify").then(
                argument("waypoint", SteelArgumentType::entity())
                    .then(color_tree())
                    .then(style_tree()),
            ),
        )
}

/// `color <team color> | color hex <hex> | color reset`.
fn color_tree() -> Builder {
    literal("color")
        .then(
            argument("color", SteelArgumentType::team_color()).executes(|context| {
                let color = context.team_color("color")?;
                set_team_color(context, color)
            }),
        )
        .then(
            literal("hex").then(argument("color", SteelArgumentType::hex_color()).executes(
                |context| {
                    let color = context.hex_color("color")?;
                    set_hex_color(context, color)
                },
            )),
        )
        .then(literal("reset").executes(reset_color))
}

/// `style reset | style set <style>`.
fn style_tree() -> Builder {
    literal("style")
        .then(literal("reset").executes(|context| set_style(context, DEFAULT_WAYPOINT_STYLE)))
        .then(
            literal("set").then(argument("style", SteelArgumentType::identifier()).executes(
                |context| {
                    let style = context.identifier("style")?.clone();
                    set_style(context, style)
                },
            )),
        )
}

/// Vanilla `WaypointArgument.getWaypoint`: a single living entity.
fn waypoint(context: &Context) -> Result<SharedEntity, CommandSyntaxError> {
    let entity = context.entity("waypoint")?;
    if entity.as_living_entity().is_none() {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::ARGUMENT_WAYPOINT_INVALID,
        )));
    }
    Ok(entity)
}

/// Vanilla `WaypointCommand.mutateIcon`: untrack, change, track again so every
/// receiver sees the new icon.
fn mutate_icon(
    context: &Context,
    mutate: impl FnOnce(&mut WaypointIcon),
) -> Result<(), CommandSyntaxError> {
    let entity = waypoint(context)?;
    let Some(living) = entity.as_living_entity() else {
        return Ok(());
    };
    let manager = context.source().world().waypoint_manager();
    manager.untrack_waypoint(entity.id());
    let mut icon = living.waypoint_icon();
    mutate(&mut icon);
    living.set_waypoint_icon(icon);
    manager.track_waypoint(&entity);
    Ok(())
}

fn set_style(context: &Context, style: Identifier) -> Result<i32, CommandSyntaxError> {
    mutate_icon(context, |icon| icon.style = style)?;
    context.source().send_success(
        &TextComponent::from(&translations::COMMANDS_WAYPOINT_MODIFY_STYLE),
        false,
    );
    Ok(0)
}

fn set_team_color(context: &Context, color: TeamColor) -> Result<i32, CommandSyntaxError> {
    mutate_icon(context, |icon| icon.color = Some(color.rgb()))?;
    let name = TextComponent::plain(color.name()).color(color.text_color());
    context.source().send_success(
        &translations::COMMANDS_WAYPOINT_MODIFY_COLOR
            .message([name])
            .component(),
        false,
    );
    Ok(0)
}

fn set_hex_color(context: &Context, color: i32) -> Result<i32, CommandSyntaxError> {
    mutate_icon(context, |icon| icon.color = Some(color))?;
    let name = TextComponent::plain(format!("{:06X}", color & 0x00FF_FFFF)).color(rgb(color));
    context.source().send_success(
        &translations::COMMANDS_WAYPOINT_MODIFY_COLOR
            .message([name])
            .component(),
        false,
    );
    Ok(0)
}

fn reset_color(context: &Context) -> Result<i32, CommandSyntaxError> {
    mutate_icon(context, |icon| icon.color = None)?;
    context.source().send_success(
        &TextComponent::from(&translations::COMMANDS_WAYPOINT_MODIFY_COLOR_RESET),
        false,
    );
    Ok(0)
}

/// Vanilla `WaypointCommand.listWaypoints`.
fn list_waypoints(context: &Context) -> i32 {
    let world = context.source().world();
    let waypoints = world.waypoint_manager().transmitters();
    let dimension = world.key.to_string();
    if waypoints.is_empty() {
        context.source().send_success(
            &translations::COMMANDS_WAYPOINT_LIST_EMPTY
                .message([dimension])
                .component(),
            false,
        );
        return 0;
    }
    let names = waypoints
        .iter()
        .map(|waypoint| waypoint_name(waypoint, &dimension));
    let names = join_components(names, &TextComponent::plain(", "));
    let count = waypoints.len();
    context.source().send_success(
        &translations::COMMANDS_WAYPOINT_LIST_SUCCESS
            .message([
                TextComponent::from(count.to_string()),
                TextComponent::from(dimension),
                names,
            ])
            .component(),
        false,
    );
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// One clickable name in the waypoint list: suggests a teleport there.
fn waypoint_name(waypoint: &SharedEntity, dimension: &str) -> TextComponent {
    let pos = waypoint.block_position();
    let color = waypoint
        .as_living_entity()
        .and_then(|living| living.waypoint_icon().color)
        .unwrap_or(-1);
    waypoint
        .display_name()
        .click_event(ClickEvent::suggest_command(format!(
            "/execute in {dimension} run tp @s {} {} {}",
            pos.x(),
            pos.y(),
            pos.z()
        )))
        .hover_event(HoverEvent::show_text(
            &translations::CHAT_COORDINATES_TOOLTIP,
        ))
        .color(rgb(color))
}

/// A text color from `0xRRGGBB`, ignoring alpha.
const fn rgb(color: i32) -> Color {
    let [_, red, green, blue] = color.to_be_bytes();
    Color::Rgb(red, green, blue)
}
