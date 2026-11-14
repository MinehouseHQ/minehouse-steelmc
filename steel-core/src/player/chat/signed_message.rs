//! Chat messages that carry their signed body: vanilla `PlayerChatMessage`,
//! `ChatType.Bound`, `SignedMessageChain.Decoder` and the per-player delivery in
//! `OutgoingChatMessage` and `ServerGamePacketListenerImpl.sendPlayerChatMessage`.

use std::borrow::Cow;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use steel_crypto::SignatureValidator;
use steel_protocol::packets::common::ChatVisibility;
use steel_protocol::packets::game::{CDisguisedChat, CPlayerChat, ChatTypeBound, FilterType};
use steel_registry::RegistryEntry;
use steel_registry::chat_type::{ChatStyle, ChatType, ChatTypeDecoration};
use steel_utils::translations;
use text_components::format::Color;
use text_components::translation::TranslatedMessage;
use text_components::{Modifier, TextComponent};
use uuid::Uuid;

use super::message_chain::{
    ChainError, MessageSignatureUpdater, SignedMessageBody, SignedMessageLink,
};
use crate::player::Player;

/// Vanilla `PlayerChatMessage.SYSTEM_SENDER`.
pub const SYSTEM_SENDER: Uuid = Uuid::nil();

/// Vanilla `PlayerChatMessage.MESSAGE_EXPIRES_AFTER_SERVER`.
const MESSAGE_EXPIRES_AFTER_SERVER: Duration = Duration::from_mins(5);

/// Vanilla's limit on messages a client may leave unacknowledged before it is
/// disconnected (`ServerGamePacketListenerImpl.sendPlayerChatMessage`).
const MAX_TRACKED_PENDING_MESSAGES: usize = 4096;

/// A chat message with its signed body and the content shown in its place.
///
/// Vanilla `PlayerChatMessage`. Steel has no text filter, so every message
/// passes through unfiltered.
#[derive(Clone, Debug)]
pub struct PlayerChatMessage {
    link: SignedMessageLink,
    signature: Option<Box<[u8; 256]>>,
    body: SignedMessageBody,
    unsigned_content: Option<TextComponent>,
}

impl PlayerChatMessage {
    /// Vanilla `PlayerChatMessage.system`: an unsigned message from no player.
    #[must_use]
    pub fn system(content: String) -> Self {
        Self::unsigned(SYSTEM_SENDER, content)
    }

    /// Vanilla `PlayerChatMessage.unsigned`.
    #[must_use]
    pub fn unsigned(sender: Uuid, content: String) -> Self {
        Self {
            link: SignedMessageLink::unsigned(sender),
            signature: None,
            body: SignedMessageBody::unsigned(content),
            unsigned_content: None,
        }
    }

    /// Vanilla `PlayerChatMessage.withUnsignedContent`: content equal to the
    /// signed text as a plain literal is not sent separately.
    #[must_use]
    pub fn with_unsigned_content(mut self, content: TextComponent) -> Self {
        self.unsigned_content =
            (content != TextComponent::plain(self.body.content.clone())).then_some(content);
        self
    }

    /// Vanilla `PlayerChatMessage.signedContent`.
    #[must_use]
    pub fn signed_content(&self) -> &str {
        &self.body.content
    }

    /// Vanilla `PlayerChatMessage.decoratedContent`.
    #[must_use]
    pub fn decorated_content(&self) -> TextComponent {
        self.unsigned_content
            .clone()
            .unwrap_or_else(|| TextComponent::plain(self.body.content.clone()))
    }

    /// Vanilla `PlayerChatMessage.sender`.
    #[must_use]
    pub const fn sender(&self) -> Uuid {
        self.link.sender
    }

    /// Vanilla `PlayerChatMessage.isSystem`.
    #[must_use]
    pub fn is_system(&self) -> bool {
        self.sender() == SYSTEM_SENDER
    }

    /// Vanilla `PlayerChatMessage.hasSignature`.
    #[must_use]
    pub const fn has_signature(&self) -> bool {
        self.signature.is_some()
    }

    /// Vanilla `PlayerChatMessage.hasExpiredServer`.
    #[must_use]
    pub fn has_expired_server(&self, now: SystemTime) -> bool {
        now > self.body.time_stamp + MESSAGE_EXPIRES_AFTER_SERVER
    }

    fn timestamp_millis(&self) -> i64 {
        self.body
            .time_stamp
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
            .unwrap_or_default()
    }
}

/// The decoded signed arguments of one chat command, by argument name.
///
/// Vanilla `CommandSigningContext.SignedArguments`.
#[derive(Clone, Debug, Default)]
pub struct SignedArguments(Vec<(Box<str>, PlayerChatMessage)>);

impl SignedArguments {
    /// Vanilla `CommandSigningContext.getArgument`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&PlayerChatMessage> {
        self.0
            .iter()
            .find(|(argument, _)| argument.as_ref() == name)
            .map(|(_, message)| message)
    }

    /// Whether an argument with this name was decoded.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Stores a decoded argument, replacing an earlier one of the same name
    /// like vanilla's map does.
    pub fn insert(&mut self, name: Box<str>, message: PlayerChatMessage) {
        match self.0.iter_mut().find(|(argument, _)| *argument == name) {
            Some((_, existing)) => *existing = message,
            None => self.0.push((name, message)),
        }
    }
}

/// A chat type with the names it is shown with. Vanilla `ChatType.Bound`.
#[derive(Clone, Debug)]
pub struct BoundChatType {
    chat_type: &'static ChatType,
    name: TextComponent,
    target_name: Option<TextComponent>,
}

impl BoundChatType {
    /// Vanilla `ChatType.bind`.
    #[must_use]
    pub const fn new(chat_type: &'static ChatType, name: TextComponent) -> Self {
        Self {
            chat_type,
            name,
            target_name: None,
        }
    }

    /// Vanilla `ChatType.Bound.withTargetName`.
    #[must_use]
    pub fn with_target_name(mut self, target_name: TextComponent) -> Self {
        self.target_name = Some(target_name);
        self
    }

    /// Vanilla `ChatType.Bound.decorate`: the message as the chat type shows
    /// it, for sources that only take system messages.
    #[must_use]
    pub fn decorate(&self, content: TextComponent) -> TextComponent {
        decorate(&self.chat_type.chat, self, content)
    }

    /// The bound chat type as it goes on the wire.
    #[must_use]
    pub fn to_protocol(&self) -> ChatTypeBound {
        ChatTypeBound {
            registry_id: i32::try_from(self.chat_type.id()).unwrap_or_default(),
            sender_name: self.name.clone(),
            target_name: self.target_name.clone(),
        }
    }
}

/// Vanilla `ChatTypeDecoration.decorate`.
fn decorate(
    decoration: &ChatTypeDecoration,
    bound: &BoundChatType,
    content: TextComponent,
) -> TextComponent {
    let arguments: Vec<TextComponent> = decoration
        .parameters
        .iter()
        .map(|parameter| match *parameter {
            "sender" => bound.name.clone(),
            "target" => bound.target_name.clone().unwrap_or_default(),
            _ => content.clone(),
        })
        .collect();
    let decorated = TextComponent::translated(TranslatedMessage {
        key: Cow::Borrowed(decoration.translation_key),
        args: Some(arguments.into_boxed_slice()),
        fallback: None,
    });
    match &decoration.style {
        Some(style) => apply_style(decorated, style),
        None => decorated,
    }
}

fn apply_style(mut component: TextComponent, style: &ChatStyle) -> TextComponent {
    if let Some(color) = style.color.and_then(named_color) {
        component = component.color(color);
    }
    if let Some(bold) = style.bold {
        component = component.bold(bold);
    }
    if let Some(italic) = style.italic {
        component = component.italic(italic);
    }
    if let Some(underlined) = style.underlined {
        component = component.underlined(underlined);
    }
    if let Some(strikethrough) = style.strikethrough {
        component = component.strikethrough(strikethrough);
    }
    if let Some(obfuscated) = style.obfuscated {
        component = component.obfuscated(obfuscated);
    }
    component
}

/// Vanilla `TextColor.parseColor` for the named `ChatFormatting` colors and
/// `#rrggbb` values.
fn named_color(name: &str) -> Option<Color> {
    Some(match name {
        "black" => Color::Black,
        "dark_blue" => Color::DarkBlue,
        "dark_green" => Color::DarkGreen,
        "dark_aqua" => Color::DarkAqua,
        "dark_red" => Color::DarkRed,
        "dark_purple" => Color::DarkPurple,
        "gold" => Color::Gold,
        "gray" => Color::Gray,
        "dark_gray" => Color::DarkGray,
        "blue" => Color::Blue,
        "green" => Color::Green,
        "aqua" => Color::Aqua,
        "red" => Color::Red,
        "light_purple" => Color::LightPurple,
        "yellow" => Color::Yellow,
        "white" => Color::White,
        _ => return Color::from_hex(name),
    })
}

/// Why a signed message body could not be accepted.
///
/// Vanilla `SignedMessageChain.DecodeException`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatDecodeError {
    /// The message carries no signature, or secure chat is enforced and the
    /// player has no chat session.
    MissingProfileKey,
    /// An earlier message broke the chain.
    ChainBroken,
    /// The player's profile key has expired.
    ExpiredProfileKey,
    /// The signature does not match the body.
    InvalidSignature,
    /// The message is older than the previous one in the chain.
    OutOfOrderChat,
    /// The signed command arguments differ from the parsed ones.
    InvalidCommandSignature,
}

impl ChatDecodeError {
    /// The message vanilla shows the player for this failure.
    #[must_use]
    pub fn message(self) -> TextComponent {
        TextComponent::from(match self {
            Self::MissingProfileKey => &translations::CHAT_DISABLED_MISSING_PROFILE_KEY,
            Self::ChainBroken => &translations::CHAT_DISABLED_CHAIN_BROKEN,
            Self::ExpiredProfileKey => &translations::CHAT_DISABLED_EXPIRED_PROFILE_KEY,
            Self::InvalidSignature => &translations::CHAT_DISABLED_INVALID_SIGNATURE,
            Self::OutOfOrderChat => &translations::CHAT_DISABLED_OUT_OF_ORDER_CHAT,
            Self::InvalidCommandSignature => &translations::CHAT_DISABLED_INVALID_COMMAND_SIGNATURE,
        })
    }
}

impl Player {
    /// Vanilla `signedMessageDecoder.unpack`: checks `signature` against this
    /// player's chat session and advances the message chain.
    ///
    /// Without a chat session the message is unsigned, unless the server
    /// enforces secure chat.
    ///
    /// # Errors
    /// Returns the vanilla decode failure; invalid signatures and out of order
    /// messages also break the chain.
    pub fn unpack_signed_message(
        &self,
        signature: Option<&[u8; 256]>,
        body: SignedMessageBody,
    ) -> Result<PlayerChatMessage, ChatDecodeError> {
        let mut chat = self.chat().lock();
        let Some(session) = chat.chat_session.clone() else {
            drop(chat);
            if self.server().enforces_secure_chat() {
                return Err(ChatDecodeError::MissingProfileKey);
            }
            return Ok(PlayerChatMessage::unsigned(
                self.gameprofile.id(),
                body.content,
            ));
        };
        let Some(signature) = signature else {
            return Err(ChatDecodeError::MissingProfileKey);
        };
        if session.profile_public_key.data().has_expired() {
            return Err(ChatDecodeError::ExpiredProfileKey);
        }
        let chain = chat
            .message_chain
            .as_mut()
            .ok_or(ChatDecodeError::ChainBroken)?;
        let link = chain
            .validate_and_advance(&body)
            .map_err(|error| match error {
                ChainError::OutOfOrderChat => ChatDecodeError::OutOfOrderChat,
                _ => ChatDecodeError::ChainBroken,
            })?;
        let validator = session.profile_public_key.create_signature_validator();
        let updater = MessageSignatureUpdater::new(&link, &body);
        if !SignatureValidator::validate(&validator, &updater, signature).unwrap_or(false) {
            chain.break_chain();
            return Err(ChatDecodeError::InvalidSignature);
        }
        drop(chat);

        let message = PlayerChatMessage {
            link,
            signature: Some(Box::new(*signature)),
            body,
            unsigned_content: None,
        };
        if message.has_expired_server(SystemTime::now()) {
            log::warn!(
                "Received expired chat: '{}'. Is the client/server system time unsynchronized?",
                message.signed_content()
            );
        }
        Ok(message)
    }

    /// Vanilla `signedMessageDecoder.setChainBroken`.
    pub fn break_message_chain(&self) {
        if let Some(chain) = self.chat().lock().message_chain.as_mut() {
            chain.break_chain();
        }
    }

    /// Vanilla `ServerPlayer.acceptsChatMessages`.
    #[must_use]
    pub fn accepts_chat_messages(&self) -> bool {
        matches!(
            self.client_information().chat_visibility,
            ChatVisibility::Full
        )
    }

    /// Vanilla `ServerPlayer.sendChatMessage` through
    /// `OutgoingChatMessage.sendToPlayer`: a message from no player goes out
    /// as disguised chat, any other as player chat.
    pub fn send_chat_message(&self, message: &PlayerChatMessage, chat_type: &BoundChatType) {
        if !self.accepts_chat_messages() {
            return;
        }
        if message.is_system() {
            self.send_packet(CDisguisedChat::new(
                &message.decorated_content(),
                chat_type.to_protocol(),
                self,
            ));
        } else {
            self.send_player_chat_message(message, chat_type);
        }
    }

    /// Vanilla `ServerGamePacketListenerImpl.sendPlayerChatMessage`.
    fn send_player_chat_message(&self, message: &PlayerChatMessage, chat_type: &BoundChatType) {
        let global_index = self.get_and_increment_messages_received();
        let (previous_messages, too_many_pending) = {
            let mut chat = self.chat().lock();
            let previous_messages = chat
                .signature_cache
                .index_previous_messages(&message.body.last_seen);
            if let Some(signature) = &message.signature {
                chat.signature_cache
                    .push(&message.body.last_seen, Some(signature));
                chat.message_validator
                    .add_pending(Some(Box::new(**signature) as Box<[u8]>));
            }
            let too_many_pending = message.signature.is_some()
                && chat.message_validator.tracked_count() > MAX_TRACKED_PENDING_MESSAGES;
            (previous_messages, too_many_pending)
        };
        self.send_packet(CPlayerChat::new(
            global_index,
            message.sender(),
            message.link.index,
            message
                .signature
                .as_ref()
                .map(|signature| Box::new(**signature) as Box<[u8]>),
            message.body.content.clone(),
            message.timestamp_millis(),
            message.body.salt,
            previous_messages,
            message.unsigned_content.clone(),
            FilterType::PassThrough,
            chat_type.to_protocol(),
        ));
        if too_many_pending {
            self.disconnect(translations::MULTIPLAYER_DISCONNECT_TOO_MANY_PENDING_CHATS.msg());
        }
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::vanilla_chat_types;

    use super::*;

    #[test]
    fn unsigned_content_equal_to_the_signed_text_is_dropped() {
        let message = PlayerChatMessage::system("hello".to_owned());
        let plain = message
            .clone()
            .with_unsigned_content(TextComponent::plain("hello"));
        assert!(plain.unsigned_content.is_none());
        assert_eq!(plain.decorated_content(), TextComponent::plain("hello"));

        let styled = message.with_unsigned_content(TextComponent::plain("hello").bold(true));
        assert_eq!(
            styled.decorated_content(),
            TextComponent::plain("hello").bold(true)
        );
    }

    #[test]
    fn system_messages_have_the_nil_sender_and_no_signature() {
        let message = PlayerChatMessage::system("hi".to_owned());
        assert!(message.is_system());
        assert!(!message.has_signature());
        assert!(!PlayerChatMessage::unsigned(Uuid::from_u128(7), "hi".to_owned()).is_system());
    }

    #[test]
    fn decorate_fills_the_parameters_and_applies_the_style() {
        let bound = BoundChatType::new(
            &vanilla_chat_types::MSG_COMMAND_OUTGOING,
            TextComponent::plain("Server"),
        )
        .with_target_name(TextComponent::plain("Alex"));
        let decorated = bound.decorate(TextComponent::plain("psst"));
        let expected = TextComponent::translated(TranslatedMessage {
            key: Cow::Borrowed("commands.message.display.outgoing"),
            args: Some(Box::new([
                TextComponent::plain("Alex"),
                TextComponent::plain("psst"),
            ])),
            fallback: None,
        })
        .color(Color::Gray)
        .italic(true);
        assert_eq!(decorated, expected);
    }

    #[test]
    fn decode_errors_use_the_vanilla_messages() {
        assert_eq!(
            ChatDecodeError::InvalidCommandSignature.message(),
            TextComponent::from(&translations::CHAT_DISABLED_INVALID_COMMAND_SIGNATURE)
        );
    }
}
