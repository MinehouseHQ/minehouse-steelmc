//! Vanilla chat commands: `MsgCommand` (with its `tell` and `w` redirects),
//! `TeamMsgCommand` (with `tm`), `EmoteCommands` and `SayCommand`.

use std::time::SystemTime;

use steel_registry::{chat_type::ChatType, vanilla_chat_types};
use steel_utils::text::DisplayResolutor;
use steel_utils::{Identifier, translations};
use text_components::interactivity::{ClickEvent, HoverEvent};
use text_components::{Modifier as _, TextComponent};

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use crate::entity::Entity as _;
use crate::player::chat::{BoundChatType, PlayerChatMessage};

pub(super) fn msg_registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("msg"), |_| msg_command())
        .alias("tell")
        .alias("w")
        .default_access()
}

pub(super) fn teammsg_registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("teammsg"), |_| teammsg_command())
        .alias("tm")
        .default_access()
}

pub(super) fn me_registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("me"), |_| me_command()).default_access()
}

pub(super) fn say_registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("say"), |_| say_command())
}

fn msg_command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("msg").then(
        argument("targets", SteelArgumentType::players())
            .then(argument("message", SteelArgumentType::message()).executes(send_private)),
    )
}

fn teammsg_command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("teammsg")
        .then(argument("message", SteelArgumentType::message()).executes(send_team_message))
}

fn me_command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("me").then(
        argument("action", SteelArgumentType::message()).executes(|context| {
            if !may_send_command_message(context.source()) {
                return Ok(0);
            }
            let message = context
                .message("action")?
                .resolve_chat_message("action", context.source())?;
            broadcast_chat_message(
                context.source(),
                &message,
                &vanilla_chat_types::EMOTE_COMMAND,
            );
            Ok(1)
        }),
    )
}

fn say_command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("say").then(
        argument("message", SteelArgumentType::message()).executes(|context| {
            if !may_send_command_message(context.source()) {
                return Ok(0);
            }
            let message = context
                .message("message")?
                .resolve_chat_message("message", context.source())?;
            broadcast_chat_message(context.source(), &message, &vanilla_chat_types::SAY_COMMAND);
            Ok(1)
        }),
    )
}

/// An installed chat interceptor may keep a player's `/me` and `/say` from going out.
fn may_send_command_message(source: &CommandSource) -> bool {
    let Some(player) = source.player() else {
        return true;
    };
    source
        .server()
        .chat_interceptor()
        .is_none_or(|interceptor| interceptor.allow_command_message(player))
}

/// Vanilla `MsgCommand.sendMessage`: the sender sees an outgoing line per
/// target, each target the incoming message.
fn send_private(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let targets = context.players("targets")?;
    let result = i32::try_from(targets.len()).map_err(|_| {
        CommandSyntaxError::dynamic("Target player count exceeds the command result range")
    })?;
    if targets.is_empty() {
        return Ok(result);
    }
    let source = context.source();
    let message = context
        .message("message")?
        .resolve_chat_message("message", source)?;
    let incoming = BoundChatType::new(
        &vanilla_chat_types::MSG_COMMAND_INCOMING,
        source.display_name(),
    );
    for target in &targets {
        let outgoing = BoundChatType::new(
            &vanilla_chat_types::MSG_COMMAND_OUTGOING,
            source.display_name(),
        )
        .with_target_name(target.display_name());
        source.send_chat_message(&message, &outgoing);
        target.send_chat_message(&message, &incoming);
    }
    Ok(result)
}

/// Vanilla `PlayerList.broadcastChatMessage` for a command source: every
/// online player gets the message, and the server log shows it decorated.
fn broadcast_chat_message(
    source: &CommandSource,
    message: &PlayerChatMessage,
    chat_type: &'static ChatType,
) {
    let bound = BoundChatType::new(chat_type, source.display_name());
    log_chat_message(message, &bound);
    for player in source.server().get_players() {
        player.send_chat_message(message, &bound);
    }
}

/// Vanilla `MinecraftServer.logChatMessage` with `PlayerList.verifyChatTrusted`.
fn log_chat_message(message: &PlayerChatMessage, bound: &BoundChatType) {
    let decorated = bound
        .decorate(message.decorated_content())
        .to_plain(&DisplayResolutor);
    let trusted = message.has_signature() && !message.has_expired_server(SystemTime::now());
    if trusted {
        log::info!("{decorated}");
    } else {
        log::info!("[Not Secure] {decorated}");
    }
}

/// Vanilla `TeamMsgCommand`: the message goes to every online player on the
/// source entity's team, the source itself seeing the outgoing form.
fn send_team_message(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let Some(entity) = source.entity() else {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::PERMISSIONS_REQUIRES_ENTITY,
        )));
    };
    let (Some(team), Some(player_team)) = (entity.team(), entity.player_team()) else {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_TEAMMSG_FAILED_NOTEAM,
        )));
    };
    let receivers: Vec<_> = source
        .server()
        .get_players()
        .into_iter()
        .filter(|receiver| {
            receiver.id() == entity.id()
                || receiver
                    .team()
                    .is_some_and(|other| team.is_same_team(&other))
        })
        .collect();
    if receivers.is_empty() {
        return Ok(0);
    }
    let message = context
        .message("message")?
        .resolve_chat_message("message", source)?;
    let team_name = player_team
        .formatted_display_name()
        .hover_event(HoverEvent::show_text(TextComponent::from(
            &translations::CHAT_TYPE_TEAM_HOVER,
        )))
        .click_event(ClickEvent::suggest_command("/teammsg "));
    let incoming = BoundChatType::new(
        &vanilla_chat_types::TEAM_MSG_COMMAND_INCOMING,
        source.display_name(),
    )
    .with_target_name(team_name.clone());
    let outgoing = BoundChatType::new(
        &vanilla_chat_types::TEAM_MSG_COMMAND_OUTGOING,
        source.display_name(),
    )
    .with_target_name(team_name);
    for receiver in &receivers {
        let chat_type = if receiver.id() == entity.id() {
            &outgoing
        } else {
            &incoming
        };
        receiver.send_chat_message(&message, chat_type);
    }
    i32::try_from(receivers.len()).map_err(|_| {
        CommandSyntaxError::dynamic("Team message receiver count exceeds the command result range")
    })
}
