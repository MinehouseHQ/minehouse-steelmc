//! Vanilla `SpectateCommand`.

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
use crate::entity::{Entity as _, SharedEntity};
use crate::player::Player;

type Context = SteelCommandContext<CommandSource>;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("spectate"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("spectate")
        .executes(|context| spectate(context, None, &source_player(context)?))
        .then(
            argument("target", SteelArgumentType::entity())
                .executes(|context| {
                    let target = context.entity("target")?;
                    spectate(context, Some(target), &source_player(context)?)
                })
                .then(
                    argument("player", SteelArgumentType::player()).executes(|context| {
                        let target = context.entity("target")?;
                        spectate(context, Some(target), &context.player("player")?)
                    }),
                ),
        )
}

/// Vanilla `CommandSourceStack.getPlayerOrException`.
fn source_player(context: &Context) -> Result<Arc<Player>, CommandSyntaxError> {
    context.source().player().cloned().ok_or_else(|| {
        CommandSyntaxError::dynamic(TextComponent::from(
            &translations::PERMISSIONS_REQUIRES_PLAYER,
        ))
    })
}

/// Vanilla `SpectateCommand.spectate`.
fn spectate(
    context: &Context,
    target: Option<SharedEntity>,
    player: &Arc<Player>,
) -> Result<i32, CommandSyntaxError> {
    if target
        .as_ref()
        .is_some_and(|target| target.id() == player.id())
    {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_SPECTATE_SELF,
        )));
    }
    if !player.is_spectator() {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_SPECTATE_NOT_SPECTATOR
                .message([player.display_name()])
                .component(),
        ));
    }
    if let Some(target) = &target
        && target.entity_type().client_tracking_range == 0
    {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_SPECTATE_CANNOT_SPECTATE
                .message([target.display_name()])
                .component(),
        ));
    }
    let message = match &target {
        Some(target) => translations::COMMANDS_SPECTATE_SUCCESS_STARTED
            .message([target.display_name()])
            .component(),
        None => TextComponent::from(&translations::COMMANDS_SPECTATE_SUCCESS_STOPPED),
    };
    player.set_camera(target);
    context.source().send_success(&message, false);
    Ok(1)
}
