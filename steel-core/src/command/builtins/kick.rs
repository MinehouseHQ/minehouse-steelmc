//! Vanilla `KickCommand`.

use std::sync::Arc;

use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use crate::entity::Entity as _;
use crate::player::Player;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("kick"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("kick").then(
        argument("targets", SteelArgumentType::players())
            .executes(|context| {
                kick_players(
                    context.source(),
                    &context.players("targets")?,
                    &TextComponent::from(&translations::MULTIPLAYER_DISCONNECT_KICKED),
                )
            })
            .then(argument("reason", SteelArgumentType::message()).executes(kick_with_reason)),
    )
}

fn kick_with_reason(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let reason = context
        .message("reason")?
        .resolve_component(context.source())?;
    kick_players(context.source(), &context.players("targets")?, &reason)
}

/// Vanilla `kickPlayers`. Steel is a dedicated server: it is always
/// published and has no singleplayer owner, so vanilla's
/// `commands.kick.singleplayer.failed` and `commands.kick.owner.failed`
/// cannot happen.
fn kick_players(
    source: &CommandSource,
    players: &[Arc<Player>],
    reason: &TextComponent,
) -> Result<i32, CommandSyntaxError> {
    for player in players {
        player.disconnect(reason.clone());
        source.send_success(
            &translations::COMMANDS_KICK_SUCCESS
                .message([player.display_name(), reason.clone()])
                .component(),
            true,
        );
    }
    i32::try_from(players.len()).map_err(|_| {
        CommandSyntaxError::dynamic("Kicked player count exceeds the command result range")
    })
}
