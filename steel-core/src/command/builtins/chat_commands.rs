//! Vanilla chat commands: `MsgCommand` (with its `tell` and `w` redirects),
//! `EmoteCommands` and `SayCommand`.

use std::time::SystemTime;

use steel_registry::{chat_type::ChatType, vanilla_chat_types};
use steel_utils::Identifier;
use steel_utils::text::DisplayResolutor;

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

fn me_command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("me").then(
        argument("action", SteelArgumentType::message()).executes(|context| {
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
            let message = context
                .message("message")?
                .resolve_chat_message("message", context.source())?;
            broadcast_chat_message(context.source(), &message, &vanilla_chat_types::SAY_COMMAND);
            Ok(1)
        }),
    )
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
