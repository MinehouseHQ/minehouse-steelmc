//! Vanilla `MessageArgument`: free chat text with embedded entity selectors,
//! signed by the client when it is the argument of a chat command.

use steel_utils::translations;
use text_components::{TextComponent, content::Resolvable};

use super::{
    CommandArgumentSource, CommandSource,
    selector::{MessageSelector, parse_message_selectors},
    text::join_components,
};
use crate::command::brigadier::{CommandSyntaxError, StringReader};
use crate::player::chat::PlayerChatMessage;

/// Vanilla's longest message argument, in UTF-16 code units.
const MAX_MESSAGE_LENGTH: usize = 256;

/// A parsed message argument. Vanilla `MessageArgument.Message`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MessageArgument {
    text: String,
    selectors: Vec<MessageSelector>,
}

impl MessageArgument {
    /// Vanilla `Message.parseText`: the rest of the input is the message, and
    /// selectors in it are only read when the source may use them.
    pub(crate) fn parse(
        reader: &mut StringReader<'_>,
        source: &dyn CommandArgumentSource,
    ) -> Result<Self, CommandSyntaxError> {
        let length = reader.remaining_length();
        if length > MAX_MESSAGE_LENGTH {
            return Err(CommandSyntaxError::dynamic(
                translations::ARGUMENT_MESSAGE_TOO_LONG
                    .message([
                        TextComponent::plain(length.to_string()),
                        TextComponent::plain(MAX_MESSAGE_LENGTH.to_string()),
                    ])
                    .component(),
            ));
        }
        let text = reader.remaining().to_owned();
        if !source.allows_entity_selectors() {
            reader.advance_bytes(text.len());
            return Ok(Self {
                text,
                selectors: Vec::new(),
            });
        }
        let selectors = parse_message_selectors(reader, source.allows_advanced_entity_selectors())?;
        Ok(Self { text, selectors })
    }

    /// Vanilla `Message.resolveComponent`: selectors become the names of the
    /// entities they find, when the source may use selectors.
    pub(crate) fn resolve_component(
        &self,
        source: &CommandSource,
    ) -> Result<TextComponent, CommandSyntaxError> {
        self.to_component(source, source.allows_entity_selectors())
    }

    /// Vanilla `MessageArgument.resolveChatMessage` for the argument `name`:
    /// the signed message when the client signed it, otherwise a system
    /// message from the typed text.
    ///
    /// Steel has no text filter and decorates chat plainly, so the message is
    /// ready to send right away.
    pub(crate) fn resolve_chat_message(
        &self,
        name: &str,
        source: &CommandSource,
    ) -> Result<PlayerChatMessage, CommandSyntaxError> {
        let formatted = self.resolve_component(source)?;
        let message = source.signed_argument(name).map_or_else(
            || PlayerChatMessage::system(self.text.clone()),
            Clone::clone,
        );
        Ok(message.with_unsigned_content(formatted))
    }

    /// Vanilla `Message.toComponent`.
    fn to_component(
        &self,
        source: &CommandSource,
        allow_selectors: bool,
    ) -> Result<TextComponent, CommandSyntaxError> {
        let Some(first) = self.selectors.first().filter(|_| allow_selectors) else {
            return Ok(TextComponent::plain(self.text.clone()));
        };
        let mut result = TextComponent::plain(self.text[..first.start].to_owned());
        let mut read_to = first.start;
        for part in &self.selectors {
            let names = part
                .selector
                .find_entities(source)?
                .iter()
                .map(|entity| entity.display_name())
                .collect::<Vec<_>>();
            let component = join_components(names, &Resolvable::entity_separator());
            if read_to < part.start {
                result.children.push(TextComponent::plain(
                    self.text[read_to..part.start].to_owned(),
                ));
            }
            result.children.push(component);
            read_to = part.end;
        }
        if read_to < self.text.len() {
            result
                .children
                .push(TextComponent::plain(self.text[read_to..].to_owned()));
        }
        Ok(result)
    }
}
