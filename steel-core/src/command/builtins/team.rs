//! Vanilla `TeamCommand`.

use steel_protocol::packets::game::{TeamCollisionRule, TeamVisibility};
use steel_utils::{Identifier, translations};
use text_components::format::Color;
use text_components::translation::Translation;
use text_components::{Modifier as _, TextComponent};

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, CommandTextResolver, ElementType, ResponseTracker, ScoreHolderWildcard,
        SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument, entity_holder,
        join_components, literal,
    },
    registration::CommandRegistration,
};
use crate::scoreboard::{PlayerTeam, ScoreHolder, ScoreboardError, TeamColor, TeamProperty};

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;
type Context = SteelCommandContext<CommandSource>;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("team"), |_| command())
}

fn team() -> Builder {
    argument("team", SteelArgumentType::team())
}

fn members() -> Builder {
    argument("members", SteelArgumentType::score_holders())
}

fn command() -> Builder {
    literal("team")
        .then(
            literal("list")
                .executes(list_teams)
                .then(team().executes(list_members)),
        )
        .then(
            literal("add").then(
                argument("team", ArgumentType::word())
                    .executes(|context| {
                        let name = context.string("team")?;
                        create_team(context, TextComponent::plain(name.to_owned()))
                    })
                    .then(
                        argument("displayName", SteelArgumentType::component()).executes(
                            |context| create_team(context, resolved(context, "displayName")?),
                        ),
                    ),
            ),
        )
        .then(literal("remove").then(team().executes(delete_team)))
        .then(literal("empty").then(team().executes(empty_team)))
        .then(
            literal("join").then(
                team()
                    .executes(|context| {
                        let Some(entity) = context.source().entity() else {
                            return Err(error(&translations::PERMISSIONS_REQUIRES_ENTITY));
                        };
                        join_team(context, vec![entity_holder(&**entity)])
                    })
                    .then(
                        members().executes(|context| join_team(context, member_holders(context)?)),
                    ),
            ),
        )
        .then(literal("leave").then(members().executes(leave_team)))
        .then(literal("modify").then(modify_command()))
}

fn modify_command() -> Builder {
    let mut name_tags = literal("nametagVisibility");
    let mut death_messages = literal("deathMessageVisibility");
    for visibility in VISIBILITY_ORDER {
        name_tags = name_tags.then(
            literal(visibility.name())
                .executes(move |context| set_name_tag_visibility(context, visibility)),
        );
        death_messages = death_messages.then(
            literal(visibility.name())
                .executes(move |context| set_death_message_visibility(context, visibility)),
        );
    }
    let mut collision = literal("collisionRule");
    for rule in COLLISION_ORDER {
        collision = collision
            .then(literal(rule.name()).executes(move |context| set_collision(context, rule)));
    }
    team()
        .then(literal("displayName").then(
            argument("displayName", SteelArgumentType::component()).executes(set_display_name),
        ))
        .then(
            literal("color")
                .then(literal("reset").executes(clear_color))
                .then(argument("value", SteelArgumentType::team_color()).executes(set_color)),
        )
        .then(
            literal("friendlyFire")
                .then(argument("allowed", ArgumentType::bool()).executes(set_friendly_fire)),
        )
        .then(
            literal("seeFriendlyInvisibles")
                .then(argument("allowed", ArgumentType::bool()).executes(set_friendly_sight)),
        )
        .then(name_tags)
        .then(death_messages)
        .then(collision)
        .then(
            literal("prefix")
                .then(argument("prefix", SteelArgumentType::component()).executes(set_prefix)),
        )
        .then(
            literal("suffix")
                .then(argument("suffix", SteelArgumentType::component()).executes(set_suffix)),
        )
}

/// The `nametagVisibility` and `deathMessageVisibility` literals in
/// vanilla's registration order.
const VISIBILITY_ORDER: [TeamVisibility; 4] = [
    TeamVisibility::Never,
    TeamVisibility::HideForOtherTeams,
    TeamVisibility::HideForOwnTeam,
    TeamVisibility::Always,
];

/// The `collisionRule` literals in vanilla's registration order.
const COLLISION_ORDER: [TeamCollisionRule; 4] = [
    TeamCollisionRule::Never,
    TeamCollisionRule::PushOwnTeam,
    TeamCollisionRule::PushOtherTeams,
    TeamCollisionRule::Always,
];

/// `ScoreHolderArgument.getNamesWithDefaultWildcard`.
fn member_holders(context: &Context) -> Result<Vec<ScoreHolder>, CommandSyntaxError> {
    context.score_holders("members", ScoreHolderWildcard::Tracked)
}

/// Vanilla `ComponentArgument.getResolvedComponent`.
fn resolved(context: &Context, name: &str) -> Result<TextComponent, CommandSyntaxError> {
    context
        .text_component(name)?
        .try_resolve(&CommandTextResolver::for_source(context.source()))
}

fn error(message: &'static Translation<0>) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(message))
}

fn count(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn plain(value: impl ToString) -> TextComponent {
    TextComponent::plain(value.to_string())
}

/// Vanilla `ComponentUtils.formatList` with its gray comma separator.
fn format_list(values: impl IntoIterator<Item = TextComponent>) -> TextComponent {
    join_components(values, &TextComponent::plain(", ").color(Color::Gray))
}

/// A team change the command already validated.
fn modify(
    context: &Context,
    team: &PlayerTeam,
    property: TeamProperty,
) -> Result<(), CommandSyntaxError> {
    scoreboard_result(context.scoreboard()?.modify_team(team.name(), property))
}

fn scoreboard_result<T>(result: Result<T, ScoreboardError>) -> Result<T, CommandSyntaxError> {
    result.map_err(|error| CommandSyntaxError::dynamic(error.to_string()))
}

/// Vanilla `Team.Visibility.getDisplayName`.
const fn visibility_name(visibility: TeamVisibility) -> &'static Translation<0> {
    match visibility {
        TeamVisibility::Always => &translations::TEAM_VISIBILITY_ALWAYS,
        TeamVisibility::Never => &translations::TEAM_VISIBILITY_NEVER,
        TeamVisibility::HideForOtherTeams => &translations::TEAM_VISIBILITY_HIDE_FOR_OTHER_TEAMS,
        TeamVisibility::HideForOwnTeam => &translations::TEAM_VISIBILITY_HIDE_FOR_OWN_TEAM,
    }
}

/// Vanilla `Team.CollisionRule.getDisplayName`.
const fn collision_name(rule: TeamCollisionRule) -> &'static Translation<0> {
    match rule {
        TeamCollisionRule::Always => &translations::TEAM_COLLISION_ALWAYS,
        TeamCollisionRule::Never => &translations::TEAM_COLLISION_NEVER,
        TeamCollisionRule::PushOtherTeams => &translations::TEAM_COLLISION_PUSH_OTHER_TEAMS,
        TeamCollisionRule::PushOwnTeam => &translations::TEAM_COLLISION_PUSH_OWN_TEAM,
    }
}

/// Vanilla `leaveTeam`.
fn leave_team(context: &Context) -> Result<i32, CommandSyntaxError> {
    let scoreboard = context.scoreboard()?;
    let mut tracker = ResponseTracker::default();
    for member in member_holders(context)? {
        let left = scoreboard.remove_holder_from_team(member.name());
        tracker.track(&member, i32::from(left));
    }
    tracker.send_feedback(
        context.source(),
        true,
        ElementType::NonZero,
        None,
        |member: &ScoreHolder, _| {
            translations::COMMANDS_TEAM_LEAVE_SUCCESS_SINGLE
                .message([member.feedback_display_name()])
                .component()
        },
        |members, _| {
            translations::COMMANDS_TEAM_LEAVE_SUCCESS_MULTIPLE
                .message([plain(members)])
                .component()
        },
    )
}

/// Vanilla `joinTeam`.
fn join_team(context: &Context, members: Vec<ScoreHolder>) -> Result<i32, CommandSyntaxError> {
    let scoreboard = context.scoreboard()?;
    let team = context.team("team")?;
    let mut tracker = ResponseTracker::default();
    for member in members {
        let joined = scoreboard_result(scoreboard.add_holder_to_team(&member, team.name()))?;
        tracker.track(&member, i32::from(joined));
    }
    let name = team.formatted_display_name();
    let multiple_name = name.clone();
    tracker.send_feedback(
        context.source(),
        true,
        ElementType::NonZero,
        None,
        |member: &ScoreHolder, _| {
            translations::COMMANDS_TEAM_JOIN_SUCCESS_SINGLE
                .message([member.feedback_display_name(), name])
                .component()
        },
        |members, _| {
            translations::COMMANDS_TEAM_JOIN_SUCCESS_MULTIPLE
                .message([plain(members), multiple_name])
                .component()
        },
    )
}

/// Sends a `/team modify` success message for `team` after a change.
fn modified(context: &Context, message: &TextComponent) -> i32 {
    context.source().send_success(message, true);
    0
}

/// The team as it is after a change, for vanilla's feedback, which reads
/// the changed team.
fn changed_team(context: &Context) -> Result<PlayerTeam, CommandSyntaxError> {
    context.team("team")
}

/// Vanilla `setNameTagVisibility`.
fn set_name_tag_visibility(
    context: &Context,
    visibility: TeamVisibility,
) -> Result<i32, CommandSyntaxError> {
    let team = context.team("team")?;
    if team.name_tag_visibility() == visibility {
        return Err(error(
            &translations::COMMANDS_TEAM_OPTION_NAMETAG_VISIBILITY_UNCHANGED,
        ));
    }
    modify(context, &team, TeamProperty::NameTagVisibility(visibility))?;
    let team = changed_team(context)?;
    Ok(modified(
        context,
        &translations::COMMANDS_TEAM_OPTION_NAMETAG_VISIBILITY_SUCCESS
            .message([
                team.formatted_display_name(),
                TextComponent::from(visibility_name(visibility)),
            ])
            .component(),
    ))
}

/// Vanilla `setDeathMessageVisibility`.
fn set_death_message_visibility(
    context: &Context,
    visibility: TeamVisibility,
) -> Result<i32, CommandSyntaxError> {
    let team = context.team("team")?;
    if team.death_message_visibility() == visibility {
        return Err(error(
            &translations::COMMANDS_TEAM_OPTION_DEATH_MESSAGE_VISIBILITY_UNCHANGED,
        ));
    }
    modify(
        context,
        &team,
        TeamProperty::DeathMessageVisibility(visibility),
    )?;
    let team = changed_team(context)?;
    Ok(modified(
        context,
        &translations::COMMANDS_TEAM_OPTION_DEATH_MESSAGE_VISIBILITY_SUCCESS
            .message([
                team.formatted_display_name(),
                TextComponent::from(visibility_name(visibility)),
            ])
            .component(),
    ))
}

/// Vanilla `setCollision`.
fn set_collision(context: &Context, rule: TeamCollisionRule) -> Result<i32, CommandSyntaxError> {
    let team = context.team("team")?;
    if team.collision_rule() == rule {
        return Err(error(
            &translations::COMMANDS_TEAM_OPTION_COLLISION_RULE_UNCHANGED,
        ));
    }
    modify(context, &team, TeamProperty::CollisionRule(rule))?;
    let team = changed_team(context)?;
    Ok(modified(
        context,
        &translations::COMMANDS_TEAM_OPTION_COLLISION_RULE_SUCCESS
            .message([
                team.formatted_display_name(),
                TextComponent::from(collision_name(rule)),
            ])
            .component(),
    ))
}

/// Vanilla `setFriendlySight`.
fn set_friendly_sight(context: &Context) -> Result<i32, CommandSyntaxError> {
    let team = context.team("team")?;
    let allowed = context.boolean("allowed")?;
    if team.can_see_friendly_invisibles() == allowed {
        return Err(error(if allowed {
            &translations::COMMANDS_TEAM_OPTION_SEE_FRIENDLY_INVISIBLES_ALREADY_ENABLED
        } else {
            &translations::COMMANDS_TEAM_OPTION_SEE_FRIENDLY_INVISIBLES_ALREADY_DISABLED
        }));
    }
    modify(context, &team, TeamProperty::SeeFriendlyInvisibles(allowed))?;
    let message = if allowed {
        &translations::COMMANDS_TEAM_OPTION_SEE_FRIENDLY_INVISIBLES_ENABLED
    } else {
        &translations::COMMANDS_TEAM_OPTION_SEE_FRIENDLY_INVISIBLES_DISABLED
    };
    let team = changed_team(context)?;
    Ok(modified(
        context,
        &message.message([team.formatted_display_name()]).component(),
    ))
}

/// Vanilla `setFriendlyFire`.
fn set_friendly_fire(context: &Context) -> Result<i32, CommandSyntaxError> {
    let team = context.team("team")?;
    let allowed = context.boolean("allowed")?;
    if team.allows_friendly_fire() == allowed {
        return Err(error(if allowed {
            &translations::COMMANDS_TEAM_OPTION_FRIENDLYFIRE_ALREADY_ENABLED
        } else {
            &translations::COMMANDS_TEAM_OPTION_FRIENDLYFIRE_ALREADY_DISABLED
        }));
    }
    modify(context, &team, TeamProperty::AllowFriendlyFire(allowed))?;
    let message = if allowed {
        &translations::COMMANDS_TEAM_OPTION_FRIENDLYFIRE_ENABLED
    } else {
        &translations::COMMANDS_TEAM_OPTION_FRIENDLYFIRE_DISABLED
    };
    let team = changed_team(context)?;
    Ok(modified(
        context,
        &message.message([team.formatted_display_name()]).component(),
    ))
}

/// Vanilla `setDisplayName`.
fn set_display_name(context: &Context) -> Result<i32, CommandSyntaxError> {
    let team = context.team("team")?;
    let display_name = resolved(context, "displayName")?;
    if team.display_name() == &display_name {
        return Err(error(&translations::COMMANDS_TEAM_OPTION_NAME_UNCHANGED));
    }
    modify(context, &team, TeamProperty::DisplayName(display_name))?;
    let team = changed_team(context)?;
    Ok(modified(
        context,
        &translations::COMMANDS_TEAM_OPTION_NAME_SUCCESS
            .message([team.formatted_display_name()])
            .component(),
    ))
}

/// Vanilla `setColor`.
fn set_color(context: &Context) -> Result<i32, CommandSyntaxError> {
    let team = context.team("team")?;
    let color = context.team_color("value")?;
    if team.color() == Some(color) {
        return Err(error(&translations::COMMANDS_TEAM_OPTION_COLOR_UNCHANGED));
    }
    modify(context, &team, TeamProperty::Color(Some(color)))?;
    let team = changed_team(context)?;
    Ok(modified(
        context,
        &translations::COMMANDS_TEAM_OPTION_COLOR_SUCCESS
            .message([team.formatted_display_name(), plain(color.name())])
            .component(),
    ))
}

/// Vanilla `clearColor`.
fn clear_color(context: &Context) -> Result<i32, CommandSyntaxError> {
    let team = context.team("team")?;
    if team.color().is_none() {
        return Err(error(&translations::COMMANDS_TEAM_OPTION_COLOR_UNCHANGED));
    }
    modify(context, &team, TeamProperty::Color(None::<TeamColor>))?;
    let team = changed_team(context)?;
    Ok(modified(
        context,
        &translations::COMMANDS_TEAM_OPTION_COLOR_CLEAR_SUCCESS
            .message([team.formatted_display_name()])
            .component(),
    ))
}

/// Vanilla `emptyTeam`.
fn empty_team(context: &Context) -> Result<i32, CommandSyntaxError> {
    let scoreboard = context.scoreboard()?;
    let team = context.team("team")?;
    let members: Vec<String> = team.players().iter().cloned().collect();
    if members.is_empty() {
        return Err(error(&translations::COMMANDS_TEAM_EMPTY_UNCHANGED));
    }
    for member in &members {
        scoreboard.remove_holder_from_team(member);
    }
    context.source().send_success(
        &translations::COMMANDS_TEAM_EMPTY_SUCCESS
            .message([plain(members.len()), team.formatted_display_name()])
            .component(),
        true,
    );
    Ok(count(members.len()))
}

/// Vanilla `deleteTeam`.
fn delete_team(context: &Context) -> Result<i32, CommandSyntaxError> {
    let scoreboard = context.scoreboard()?;
    let team = context.team("team")?;
    scoreboard.remove_team(team.name());
    context.source().send_success(
        &translations::COMMANDS_TEAM_REMOVE_SUCCESS
            .message([team.formatted_display_name()])
            .component(),
        true,
    );
    Ok(count(scoreboard.team_names().len()))
}

/// Vanilla `createTeam`.
fn create_team(context: &Context, display_name: TextComponent) -> Result<i32, CommandSyntaxError> {
    let scoreboard = context.scoreboard()?;
    let name = context.string("team")?;
    if scoreboard.team(name).is_some() {
        return Err(error(&translations::COMMANDS_TEAM_ADD_DUPLICATE));
    }
    scoreboard_result(scoreboard.add_team(name))?;
    scoreboard_result(scoreboard.modify_team(name, TeamProperty::DisplayName(display_name)))?;
    let team = scoreboard.team(name).ok_or_else(|| {
        CommandSyntaxError::dynamic(ScoreboardError::MissingTeam(name.to_owned()).to_string())
    })?;
    context.source().send_success(
        &translations::COMMANDS_TEAM_ADD_SUCCESS
            .message([team.formatted_display_name()])
            .component(),
        true,
    );
    Ok(count(scoreboard.team_names().len()))
}

/// Vanilla `listMembers`.
fn list_members(context: &Context) -> Result<i32, CommandSyntaxError> {
    let team = context.team("team")?;
    let members = team.players();
    let message = if members.is_empty() {
        translations::COMMANDS_TEAM_LIST_MEMBERS_EMPTY
            .message([team.formatted_display_name()])
            .component()
    } else {
        translations::COMMANDS_TEAM_LIST_MEMBERS_SUCCESS
            .message([
                team.formatted_display_name(),
                plain(members.len()),
                format_list(members.iter().map(plain)),
            ])
            .component()
    };
    context.source().send_success(&message, false);
    Ok(count(members.len()))
}

/// Vanilla `listTeams`.
fn list_teams(context: &Context) -> Result<i32, CommandSyntaxError> {
    let teams = context.scoreboard()?.teams();
    let message = if teams.is_empty() {
        TextComponent::from(&translations::COMMANDS_TEAM_LIST_TEAMS_EMPTY)
    } else {
        translations::COMMANDS_TEAM_LIST_TEAMS_SUCCESS
            .message([
                plain(teams.len()),
                format_list(teams.iter().map(PlayerTeam::formatted_display_name)),
            ])
            .component()
    };
    context.source().send_success(&message, false);
    Ok(count(teams.len()))
}

/// Vanilla `setPrefix`.
fn set_prefix(context: &Context) -> Result<i32, CommandSyntaxError> {
    let team = context.team("team")?;
    let prefix = resolved(context, "prefix")?;
    modify(context, &team, TeamProperty::PlayerPrefix(prefix.clone()))?;
    context.source().send_success(
        &translations::COMMANDS_TEAM_OPTION_PREFIX_SUCCESS
            .message([prefix])
            .component(),
        false,
    );
    Ok(1)
}

/// Vanilla `setSuffix`.
fn set_suffix(context: &Context) -> Result<i32, CommandSyntaxError> {
    let team = context.team("team")?;
    let suffix = resolved(context, "suffix")?;
    modify(context, &team, TeamProperty::PlayerSuffix(suffix.clone()))?;
    context.source().send_success(
        &translations::COMMANDS_TEAM_OPTION_SUFFIX_SUCCESS
            .message([suffix])
            .component(),
        false,
    );
    Ok(1)
}
