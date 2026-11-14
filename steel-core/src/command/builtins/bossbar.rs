//! Vanilla `BossBarCommands`.

use std::sync::Arc;

use steel_protocol::packets::game::{BossBarColor, BossBarOverlay};
use steel_utils::{Identifier, translations};
use text_components::format::Color;
use text_components::translation::Translation;
use text_components::{Modifier as _, TextComponent};

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError, SuggestionsBuilder},
    execution::{
        CommandSource, CommandTextResolver, SteelArgumentType, SteelCommandContext,
        SteelCommandRuntime, SteelSuggestionContext, argument, join_components, literal,
        suggest_resources,
    },
    registration::CommandRegistration,
};
use crate::boss_event::custom::{CustomBossEvent, CustomBossEvents};
use crate::entity::Entity as _;
use crate::player::Player;

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;

const COLORS: [(&str, BossBarColor); 7] = [
    ("pink", BossBarColor::Pink),
    ("blue", BossBarColor::Blue),
    ("red", BossBarColor::Red),
    ("green", BossBarColor::Green),
    ("yellow", BossBarColor::Yellow),
    ("purple", BossBarColor::Purple),
    ("white", BossBarColor::White),
];

const STYLES: [(&str, BossBarOverlay); 5] = [
    ("progress", BossBarOverlay::Progress),
    ("notched_6", BossBarOverlay::Notched6),
    ("notched_10", BossBarOverlay::Notched10),
    ("notched_12", BossBarOverlay::Notched12),
    ("notched_20", BossBarOverlay::Notched20),
];

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("bossbar"), |_| command())
}

/// Vanilla `SUGGEST_BOSS_BAR`: the ids of the source domain's bars.
pub(super) fn suggest_boss_bars(
    context: &SteelSuggestionContext<'_, CommandSource>,
    builder: &mut SuggestionsBuilder<'_>,
) {
    if let Some(events) = boss_bars(context.source()) {
        suggest_resources(events.ids().iter(), builder);
    }
}

/// A bar id argument that suggests the existing bars.
pub(super) fn boss_bar_id() -> Builder {
    argument("id", SteelArgumentType::identifier()).suggests(suggest_boss_bars)
}

fn command() -> Builder {
    literal("bossbar")
        .then(
            literal("add").then(
                argument("id", SteelArgumentType::identifier())
                    .then(argument("name", SteelArgumentType::component()).executes(create_bar)),
            ),
        )
        .then(literal("remove").then(boss_bar_id().executes(remove_bar)))
        .then(literal("list").executes(list_bars))
        .then(literal("set").then(set_command()))
        .then(
            literal("get").then(
                boss_bar_id()
                    .then(literal("value").executes(get_value))
                    .then(literal("max").executes(get_max))
                    .then(literal("visible").executes(get_visible))
                    .then(literal("players").executes(get_players)),
            ),
        )
}

fn set_command() -> Builder {
    let mut color = literal("color");
    for (name, value) in COLORS {
        color = color.then(literal(name).executes(move |context| set_color(context, value)));
    }
    let mut style = literal("style");
    for (name, value) in STYLES {
        style = style.then(literal(name).executes(move |context| set_style(context, value)));
    }
    boss_bar_id()
        .then(
            literal("name")
                .then(argument("name", SteelArgumentType::component()).executes(set_name)),
        )
        .then(color)
        .then(style)
        .then(
            literal("value")
                .then(argument("value", ArgumentType::integer(0, i32::MAX)).executes(set_value)),
        )
        .then(
            literal("max")
                .then(argument("max", ArgumentType::integer(1, i32::MAX)).executes(set_max)),
        )
        .then(
            literal("visible")
                .then(argument("visible", ArgumentType::bool()).executes(set_visible)),
        )
        .then(
            literal("players")
                .executes(|context| set_players(context, &[]))
                .then(
                    argument("targets", SteelArgumentType::players()).executes(|context| {
                        set_players(context, &context.optional_players("targets")?)
                    }),
                ),
        )
}

/// The source domain's bars.
fn boss_bars(source: &CommandSource) -> Option<&CustomBossEvents> {
    source
        .server()
        .custom_boss_events
        .get(source.world().domain())
}

fn domain_boss_bars(source: &CommandSource) -> Result<&CustomBossEvents, CommandSyntaxError> {
    boss_bars(source).ok_or_else(|| {
        CommandSyntaxError::dynamic(format!(
            "Domain '{}' has no boss bars",
            source.world().domain()
        ))
    })
}

/// Vanilla `getBossBar`.
pub(super) fn get_boss_bar(
    context: &SteelCommandContext<CommandSource>,
) -> Result<Arc<CustomBossEvent>, CommandSyntaxError> {
    let id = context.identifier("id")?;
    domain_boss_bars(context.source())?.get(id).ok_or_else(|| {
        CommandSyntaxError::dynamic(
            translations::COMMANDS_BOSSBAR_UNKNOWN
                .message([TextComponent::plain(id.to_string())])
                .component(),
        )
    })
}

/// Vanilla `ComponentArgument.getResolvedComponent` followed by the
/// resolution `/bossbar` repeats without an entity: one pass against the
/// source's own entity.
fn resolved_name(
    context: &SteelCommandContext<CommandSource>,
) -> Result<TextComponent, CommandSyntaxError> {
    context
        .text_component("name")?
        .try_resolve(&CommandTextResolver::for_source(context.source()))
}

fn count(value: usize) -> Result<i32, CommandSyntaxError> {
    i32::try_from(value)
        .map_err(|_| CommandSyntaxError::dynamic("Count exceeds the command result range"))
}

/// Vanilla `ComponentUtils.formatList` with its gray comma separator.
fn format_list(values: impl IntoIterator<Item = TextComponent>) -> TextComponent {
    join_components(values, &TextComponent::plain(", ").color(Color::Gray))
}

fn player_names(players: &[Arc<Player>]) -> TextComponent {
    format_list(players.iter().map(|player| player.display_name()))
}

fn unchanged(message: &'static Translation<0>) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(message))
}

/// Vanilla `createBar`.
fn create_bar(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let events = domain_boss_bars(source)?;
    let id = context.identifier("id")?;
    if events.get(id).is_some() {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_BOSSBAR_CREATE_FAILED
                .message([TextComponent::plain(id.to_string())])
                .component(),
        ));
    }
    let event = events.create(id.clone(), resolved_name(context)?);
    source.send_success(
        &translations::COMMANDS_BOSSBAR_CREATE_SUCCESS
            .message([event.display_name()])
            .component(),
        true,
    );
    count(events.events().len())
}

/// Vanilla `removeBar`.
fn remove_bar(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let events = domain_boss_bars(source)?;
    let bar = get_boss_bar(context)?;
    bar.remove_all_players();
    events.remove(&bar);
    source.send_success(
        &translations::COMMANDS_BOSSBAR_REMOVE_SUCCESS
            .message([bar.display_name()])
            .component(),
        true,
    );
    count(events.events().len())
}

/// Vanilla `listBars`.
fn list_bars(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let events = domain_boss_bars(source)?.events();
    if events.is_empty() {
        source.send_success(
            &TextComponent::from(&translations::COMMANDS_BOSSBAR_LIST_BARS_NONE),
            false,
        );
    } else {
        source.send_success(
            &translations::COMMANDS_BOSSBAR_LIST_BARS_SOME
                .message([
                    TextComponent::plain(events.len().to_string()),
                    format_list(events.iter().map(|event| event.display_name())),
                ])
                .component(),
            false,
        );
    }
    count(events.len())
}

/// Vanilla `getValue`.
fn get_value(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let bar = get_boss_bar(context)?;
    let value = bar.value();
    context.source().send_success(
        &translations::COMMANDS_BOSSBAR_GET_VALUE
            .message([bar.display_name(), TextComponent::plain(value.to_string())])
            .component(),
        true,
    );
    Ok(value)
}

/// Vanilla `getMax`.
fn get_max(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let bar = get_boss_bar(context)?;
    let max = bar.max();
    context.source().send_success(
        &translations::COMMANDS_BOSSBAR_GET_MAX
            .message([bar.display_name(), TextComponent::plain(max.to_string())])
            .component(),
        true,
    );
    Ok(max)
}

/// Vanilla `getVisible`.
fn get_visible(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let bar = get_boss_bar(context)?;
    let (message, result) = if bar.event().is_visible() {
        (&translations::COMMANDS_BOSSBAR_GET_VISIBLE_VISIBLE, 1)
    } else {
        (&translations::COMMANDS_BOSSBAR_GET_VISIBLE_HIDDEN, 0)
    };
    context
        .source()
        .send_success(&message.message([bar.display_name()]).component(), true);
    Ok(result)
}

/// Vanilla `getPlayers`: the online players seeing the bar.
fn get_players(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let bar = get_boss_bar(context)?;
    let players = bar.event().players();
    let message = if players.is_empty() {
        translations::COMMANDS_BOSSBAR_GET_PLAYERS_NONE
            .message([bar.display_name()])
            .component()
    } else {
        translations::COMMANDS_BOSSBAR_GET_PLAYERS_SOME
            .message([
                bar.display_name(),
                TextComponent::plain(players.len().to_string()),
                player_names(&players),
            ])
            .component()
    };
    context.source().send_success(&message, true);
    count(players.len())
}

/// Vanilla `setVisible`.
fn set_visible(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let bar = get_boss_bar(context)?;
    let visible = context.boolean("visible")?;
    if bar.event().is_visible() == visible {
        return Err(unchanged(if visible {
            &translations::COMMANDS_BOSSBAR_SET_VISIBILITY_UNCHANGED_VISIBLE
        } else {
            &translations::COMMANDS_BOSSBAR_SET_VISIBILITY_UNCHANGED_HIDDEN
        }));
    }
    bar.set_visible(visible);
    let message = if visible {
        &translations::COMMANDS_BOSSBAR_SET_VISIBLE_SUCCESS_VISIBLE
    } else {
        &translations::COMMANDS_BOSSBAR_SET_VISIBLE_SUCCESS_HIDDEN
    };
    context
        .source()
        .send_success(&message.message([bar.display_name()]).component(), true);
    Ok(0)
}

/// Vanilla `setValue`.
fn set_value(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let bar = get_boss_bar(context)?;
    let value = context.integer("value")?;
    if bar.value() == value {
        return Err(unchanged(
            &translations::COMMANDS_BOSSBAR_SET_VALUE_UNCHANGED,
        ));
    }
    bar.set_value(value);
    context.source().send_success(
        &translations::COMMANDS_BOSSBAR_SET_VALUE_SUCCESS
            .message([bar.display_name(), TextComponent::plain(value.to_string())])
            .component(),
        true,
    );
    Ok(value)
}

/// Vanilla `setMax`.
fn set_max(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let bar = get_boss_bar(context)?;
    let max = context.integer("max")?;
    if bar.max() == max {
        return Err(unchanged(&translations::COMMANDS_BOSSBAR_SET_MAX_UNCHANGED));
    }
    bar.set_max(max);
    context.source().send_success(
        &translations::COMMANDS_BOSSBAR_SET_MAX_SUCCESS
            .message([bar.display_name(), TextComponent::plain(max.to_string())])
            .component(),
        true,
    );
    Ok(max)
}

/// Vanilla `setColor`.
fn set_color(
    context: &SteelCommandContext<CommandSource>,
    color: BossBarColor,
) -> Result<i32, CommandSyntaxError> {
    let bar = get_boss_bar(context)?;
    if bar.event().color() == color {
        return Err(unchanged(
            &translations::COMMANDS_BOSSBAR_SET_COLOR_UNCHANGED,
        ));
    }
    bar.set_color(color);
    context.source().send_success(
        &translations::COMMANDS_BOSSBAR_SET_COLOR_SUCCESS
            .message([bar.display_name()])
            .component(),
        true,
    );
    Ok(0)
}

/// Vanilla `setStyle`.
fn set_style(
    context: &SteelCommandContext<CommandSource>,
    style: BossBarOverlay,
) -> Result<i32, CommandSyntaxError> {
    let bar = get_boss_bar(context)?;
    if bar.event().overlay() == style {
        return Err(unchanged(
            &translations::COMMANDS_BOSSBAR_SET_STYLE_UNCHANGED,
        ));
    }
    bar.set_overlay(style);
    context.source().send_success(
        &translations::COMMANDS_BOSSBAR_SET_STYLE_SUCCESS
            .message([bar.display_name()])
            .component(),
        true,
    );
    Ok(0)
}

/// Vanilla `setName`.
fn set_name(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let bar = get_boss_bar(context)?;
    let name = resolved_name(context)?;
    if bar.event().name() == name {
        return Err(unchanged(
            &translations::COMMANDS_BOSSBAR_SET_NAME_UNCHANGED,
        ));
    }
    bar.set_name(name);
    context.source().send_success(
        &translations::COMMANDS_BOSSBAR_SET_NAME_SUCCESS
            .message([bar.display_name()])
            .component(),
        true,
    );
    Ok(0)
}

/// Vanilla `setPlayers`.
fn set_players(
    context: &SteelCommandContext<CommandSource>,
    targets: &[Arc<Player>],
) -> Result<i32, CommandSyntaxError> {
    let bar = get_boss_bar(context)?;
    if !bar.set_players(targets) {
        return Err(unchanged(
            &translations::COMMANDS_BOSSBAR_SET_PLAYERS_UNCHANGED,
        ));
    }
    let players = bar.event().players();
    let message = if players.is_empty() {
        translations::COMMANDS_BOSSBAR_SET_PLAYERS_SUCCESS_NONE
            .message([bar.display_name()])
            .component()
    } else {
        translations::COMMANDS_BOSSBAR_SET_PLAYERS_SUCCESS_SOME
            .message([
                bar.display_name(),
                TextComponent::plain(targets.len().to_string()),
                player_names(targets),
            ])
            .component()
    };
    context.source().send_success(&message, true);
    count(players.len())
}
