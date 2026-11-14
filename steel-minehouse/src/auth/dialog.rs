//! The «Вход на `MineHouse`» window shown while the player confirms the code.

use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_utils::Identifier;
use text_components::{Modifier, TextComponent, format::Color};

use super::settings::AuthConfig;

const BODY_WIDTH: i32 = 300;
const BUTTON_WIDTH: i32 = 140;

/// Sent by the «Отмена» button as a custom click action.
#[must_use]
pub const fn cancel_action() -> Identifier {
    Identifier::new_static("minehouse", "auth/cancel")
}

/// The dialog with `code`, as vanilla's `Dialog.DIRECT_CODEC` reads it: a `multi_action`
/// dialog that only the server closes, with «Скопировать код» and «Отмена».
#[must_use]
pub fn code_dialog(config: &AuthConfig, code: &str) -> NbtTag {
    let messages = &config.messages;
    let instructions = messages
        .instructions
        .replace("{site}", &config.site_name)
        .replace("{invite}", &config.discord_invite)
        .replace("{code}", code);

    let body = vec![
        plain_message(TextComponent::plain(messages.code_label.clone()).color(Color::Gray)),
        plain_message(
            TextComponent::plain(spaced(code))
                .color(Color::Gold)
                .bold(true),
        ),
        plain_message(TextComponent::plain(instructions)),
        plain_message(TextComponent::plain(messages.warning.clone()).color(Color::Yellow)),
        plain_message(
            TextComponent::plain(messages.waiting.clone())
                .color(Color::Gray)
                .italic(true),
        ),
    ];

    let mut copy = button(&messages.copy_button);
    copy.insert(
        "tooltip",
        TextComponent::plain(messages.copy_tooltip.clone()).to_codec_nbt(),
    );
    let mut copy_action = NbtCompound::new();
    copy_action.insert("type", "minecraft:copy_to_clipboard");
    copy_action.insert("value", code);
    copy.insert("action", NbtTag::Compound(copy_action));

    let mut cancel = button(&messages.cancel_button);
    let mut cancel_action_tag = NbtCompound::new();
    cancel_action_tag.insert("type", "minecraft:custom");
    cancel_action_tag.insert("id", cancel_action().to_string());
    cancel.insert("action", NbtTag::Compound(cancel_action_tag));

    let mut dialog = NbtCompound::new();
    dialog.insert("type", "minecraft:multi_action");
    dialog.insert(
        "title",
        TextComponent::plain(messages.dialog_title.clone()).to_codec_nbt(),
    );
    dialog.insert("can_close_with_escape", false);
    dialog.insert("pause", false);
    // `none` keeps the window open after «Скопировать код».
    dialog.insert("after_action", "none");
    dialog.insert("body", NbtTag::List(NbtList::Compound(body)));
    dialog.insert(
        "actions",
        NbtTag::List(NbtList::Compound(vec![copy, cancel])),
    );
    dialog.insert("columns", 2);
    NbtTag::Compound(dialog)
}

fn plain_message(contents: TextComponent) -> NbtCompound {
    let mut message = NbtCompound::new();
    message.insert("type", "minecraft:plain_message");
    message.insert("contents", contents.to_codec_nbt());
    message.insert("width", BODY_WIDTH);
    message
}

fn button(label: &str) -> NbtCompound {
    let mut button = NbtCompound::new();
    button.insert(
        "label",
        TextComponent::plain(label.to_owned()).to_codec_nbt(),
    );
    button.insert("width", BUTTON_WIDTH);
    button
}

/// `482913` → `482 913`, easier to read off the screen.
fn spaced(code: &str) -> String {
    if code.len() == 6 && code.is_ascii() {
        format!("{} {}", &code[..3], &code[3..])
    } else {
        code.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_has_the_code_and_both_buttons() {
        let NbtTag::Compound(dialog) = code_dialog(&AuthConfig::default(), "482913") else {
            panic!("the dialog is a compound");
        };
        assert_eq!(
            dialog.get("type"),
            Some(&NbtTag::String("minecraft:multi_action".into()))
        );
        assert_eq!(
            dialog.get("after_action"),
            Some(&NbtTag::String("none".into()))
        );

        let Some(NbtTag::List(NbtList::Compound(body))) = dialog.get("body") else {
            panic!("body is a list of compounds");
        };
        assert_eq!(body.len(), 5);
        let code = body[1].get("contents").and_then(NbtTag::compound);
        assert_eq!(
            code.and_then(|code| code.get("text")),
            Some(&NbtTag::String("482 913".into()))
        );

        let Some(NbtTag::List(NbtList::Compound(actions))) = dialog.get("actions") else {
            panic!("actions is a list of compounds");
        };
        let copy = actions[0].get("action").and_then(NbtTag::compound);
        assert_eq!(
            copy.and_then(|copy| copy.get("value")),
            Some(&NbtTag::String("482913".into()))
        );
        let cancel = actions[1].get("action").and_then(NbtTag::compound);
        assert_eq!(
            cancel.and_then(|cancel| cancel.get("id")),
            Some(&NbtTag::String("minehouse:auth/cancel".into()))
        );
    }

    #[test]
    fn only_six_digit_codes_are_spaced() {
        assert_eq!(spaced("482913"), "482 913");
        assert_eq!(spaced("1234"), "1234");
    }
}
