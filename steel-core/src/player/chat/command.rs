//! Chat commands from the client: vanilla
//! `ServerGamePacketListenerImpl.handleChatCommand` and
//! `handleSignedChatCommand`.

use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use steel_protocol::packets::game::{SChatCommand, SChatCommandSigned};
use steel_utils::translations;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use super::LastSeen;
use super::message_chain::SignedMessageBody;
use super::signed_message::{ChatDecodeError, SignedArguments};
use crate::command::sender::CommandSender;
use crate::player::Player;

/// Vanilla `StringUtil.isAllowedChatCharacter`.
const fn is_allowed_chat_character(character: char) -> bool {
    character != '\u{a7}' && character >= ' ' && character != '\u{7f}'
}

/// Vanilla `ServerGamePacketListenerImpl.isChatMessageIllegal`.
fn is_chat_message_illegal(message: &str) -> bool {
    !message.chars().all(is_allowed_chat_character)
}

impl Player {
    /// Vanilla `handleChatCommand`: runs a command sent without signatures.
    ///
    /// When the server enforces secure chat, a command whose arguments the
    /// client should have signed is refused.
    pub fn handle_chat_command(self: &Arc<Self>, packet: SChatCommand) {
        if is_chat_message_illegal(&packet.command) {
            self.disconnect(translations::MULTIPLAYER_DISCONNECT_ILLEGAL_CHARACTERS.msg());
            return;
        }
        self.reset_last_action_time();
        let server = self.server();
        if server.enforces_secure_chat()
            && !server
                .signable_command_arguments(self, &packet.command)
                .is_empty()
        {
            log::error!(
                "Received unsigned command packet from {}, but the command requires signable arguments: {}",
                self.gameprofile.name(),
                packet.command
            );
            self.send_message(&TextComponent::from(
                &translations::CHAT_DISABLED_INVALID_COMMAND_SIGNATURE,
            ));
        } else {
            self.submit_chat_command(
                server.submit_command(CommandSender::Player(Arc::clone(self)), packet.command),
            );
        }
        self.detect_command_rate_spam();
    }

    /// Vanilla `handleSignedChatCommand`: decodes the signed message
    /// arguments in packet order, so they stay ordered with this player's chat
    /// in the message chain, then queues the command with them.
    pub fn handle_signed_chat_command(self: &Arc<Self>, packet: SChatCommandSigned) {
        let last_seen = self.chat().lock().message_validator.apply_update(
            packet.last_seen.acknowledged,
            packet.last_seen.offset.0,
            packet.last_seen.checksum,
        );
        let last_seen = match last_seen {
            Ok(signatures) => LastSeen::new(signatures),
            Err(error) => {
                log::error!(
                    "Failed to validate message acknowledgements from {}: {error}",
                    self.gameprofile.name()
                );
                self.disconnect(translations::MULTIPLAYER_DISCONNECT_CHAT_VALIDATION_FAILED.msg());
                return;
            }
        };
        if is_chat_message_illegal(&packet.command) {
            self.disconnect(translations::MULTIPLAYER_DISCONNECT_ILLEGAL_CHARACTERS.msg());
            return;
        }
        self.reset_last_action_time();

        let server = self.server();
        let parsed = server.signable_command_arguments(self, &packet.command);
        match self.collect_signed_arguments(&packet, &parsed, &last_seen) {
            Ok(signed_arguments) => self.submit_chat_command(server.submit_signed_command(
                Arc::clone(self),
                packet.command,
                signed_arguments,
            )),
            Err(error) => {
                let message = error.message();
                log::warn!(
                    "Failed to update secure chat state for {}: '{}'",
                    self.gameprofile.name(),
                    message.to_plain(self.as_ref())
                );
                self.send_message(&message.color(Color::Red));
            }
        }
        self.detect_command_rate_spam();
    }

    fn submit_chat_command<E>(&self, submitted: Result<(), E>) {
        if submitted.is_err() {
            self.send_message(
                &TextComponent::const_plain("Command queue is full").color(Color::Red),
            );
        }
    }

    /// Vanilla `collectSignedArguments`: every argument the client signed must
    /// be one the server parsed as signable, and the other way round.
    fn collect_signed_arguments(
        &self,
        packet: &SChatCommandSigned,
        parsed: &[(Box<str>, String)],
        last_seen: &LastSeen,
    ) -> Result<SignedArguments, ChatDecodeError> {
        let mut signed_arguments = SignedArguments::default();
        if packet.argument_signatures.is_empty() {
            // Vanilla `collectUnsignedArguments`.
            for (name, value) in parsed {
                let body = SignedMessageBody::unsigned(value.clone());
                signed_arguments.insert(name.clone(), self.unpack_signed_message(None, body)?);
            }
            return Ok(signed_arguments);
        }

        let timestamp =
            UNIX_EPOCH + Duration::from_millis(u64::try_from(packet.timestamp).unwrap_or(0));
        for client_argument in &packet.argument_signatures {
            let Some((name, value)) = parsed
                .iter()
                .find(|(name, _)| name.as_ref() == client_argument.name)
            else {
                self.break_message_chain();
                return Err(signed_argument_mismatch(packet, parsed));
            };
            let body =
                SignedMessageBody::new(value.clone(), timestamp, packet.salt, last_seen.clone());
            let message = self.unpack_signed_message(Some(&client_argument.signature), body)?;
            signed_arguments.insert(name.clone(), message);
        }
        if parsed
            .iter()
            .any(|(name, _)| !signed_arguments.contains(name))
        {
            return Err(signed_argument_mismatch(packet, parsed));
        }
        Ok(signed_arguments)
    }
}

/// Vanilla `createSignedArgumentMismatchException`.
fn signed_argument_mismatch(
    packet: &SChatCommandSigned,
    parsed: &[(Box<str>, String)],
) -> ChatDecodeError {
    let client_names = packet
        .argument_signatures
        .iter()
        .map(|argument| argument.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let expected_names = parsed
        .iter()
        .map(|(name, _)| name.as_ref())
        .collect::<Vec<_>>()
        .join(", ");
    log::error!(
        "Signed command mismatch between server and client ('{}'): got [{client_names}] from client, but expected [{expected_names}]",
        packet.command
    );
    ChatDecodeError::InvalidCommandSignature
}

#[cfg(test)]
mod tests {
    use super::is_chat_message_illegal;

    #[test]
    fn illegal_chat_characters_match_vanilla() {
        assert!(!is_chat_message_illegal("msg Alex привет :)"));
        assert!(is_chat_message_illegal("say \u{a7}cred"));
        assert!(is_chat_message_illegal("say a\nb"));
        assert!(is_chat_message_illegal("say \u{7f}"));
    }
}
