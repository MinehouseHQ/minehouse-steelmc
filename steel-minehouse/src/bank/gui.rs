//! The bank's windows: vanilla chest windows with a background of our own (the Fabric `Gui`).
//!
//! The background is a glyph of the font `minehouse-bank:gui` in the title of the window: a
//! shift left to the edge of the window, a picture 176 px wide, a shift back to where the
//! title starts. The Fabric mod left the glyph out for a player without the server pack; here
//! the pack is required, so it is always there.

use steel_core::{entity::Entity as _, player::Player};
use steel_protocol::packets::game::SoundSource;
use steel_registry::{
    data_components::vanilla_components::{ITEM_MODEL, ITEM_NAME, ItemLore, LORE},
    item_stack::ItemStack,
    items::ItemRef,
    sound_event::SoundEvent,
    sound_events,
};
use steel_utils::Identifier;
use text_components::{Modifier as _, TextComponent, format::Color};

use crate::moderation::format::render;

/// The background of the ATM.
pub const ATM: char = '\u{E000}';
/// The background of the amount picker.
pub const AMOUNT: char = '\u{E001}';
/// The background of the terminal's setup.
pub const SETUP: char = '\u{E002}';
/// The background of the payment window.
pub const PAY: char = '\u{E003}';
/// The ATM before the player has an account: only «Открыть счёт» and «Закрыть».
pub const WELCOME: char = '\u{E004}';
/// A shift of −8 px.
const BACK: char = '\u{F000}';
/// A shift of −169 px: the 177 px of the picture back to x = 8.
const FORTH: char = '\u{F001}';
const FONT: &str = "minehouse-bank:gui";

/// The title of a window: the background and the text over it.
#[must_use]
pub fn title(background: char, text: &str) -> TextComponent {
    let mut picture =
        TextComponent::plain(format!("{BACK}{background}{FORTH}")).color(Color::Rgb(255, 255, 255));
    picture.format = picture.format.font(FONT);
    TextComponent::plain("")
        .add_child(picture)
        .add_child(TextComponent::plain(text.to_owned()))
}

/// A line of a lore: upright, in `color`.
#[must_use]
pub fn line(text: &str, color: Color) -> TextComponent {
    let mut line = TextComponent::plain(text.to_owned()).color(color);
    line.format = line.format.italic(false);
    line
}

/// A button: a plain item with a white name and grey lore lines.
#[must_use]
pub fn button(item: ItemRef, name: &str, lore: &[&str]) -> ItemStack {
    let lines = lore.iter().map(|text| line(text, Color::Gray)).collect();
    button_with(item, name, lines)
}

/// A button with lore lines of its own colors.
#[must_use]
pub fn button_with(item: ItemRef, name: &str, lore: Vec<TextComponent>) -> ItemStack {
    let mut stack = ItemStack::new(item);
    stack.set(ITEM_NAME, line(name, Color::White));
    if !lore.is_empty()
        && let Ok(lore) = ItemLore::new(lore)
    {
        stack.set(LORE, lore);
    }
    stack
}

/// Draws `stack` with the item model `model` of the server pack.
pub fn model(stack: &mut ItemStack, model: &'static str) {
    if let Some((namespace, path)) = model.split_once(':') {
        stack.set(ITEM_MODEL, Identifier::new_static(namespace, path));
    }
}

fn sound(player: &Player, sound: &'static SoundEvent, volume: f32, pitch: f32) {
    player.get_world().play_sound_at(
        sound,
        SoundSource::Players,
        player.position(),
        volume,
        pitch,
        None,
    );
}

/// The click of a button.
pub fn click(player: &Player) {
    sound(player, &sound_events::UI_BUTTON_CLICK, 0.4, 1.0);
}

/// The chime of money that went through.
pub fn ding(player: &Player) {
    sound(player, &sound_events::BLOCK_NOTE_BLOCK_CHIME, 0.6, 1.4);
}

/// A refusal: a red line and a low note.
pub fn error(player: &Player, text: &str) {
    player.send_message(&render("&c{text}", &[("text", text)]));
    sound(player, &sound_events::BLOCK_NOTE_BLOCK_BASS, 0.6, 0.7);
}

/// A green line.
pub fn info(player: &Player, text: &str) {
    player.send_message(&render("&a{text}", &[("text", text)]));
}

/// What an error code of the site means, in Russian.
#[must_use]
pub fn error_text(code: &str) -> String {
    match code {
        "no_player" => "Ник не привязан к аккаунту на сайте.",
        "no_account" => "Счёта ещё нет: открой его в банкомате.",
        "frozen" => "Счёт заморожен администрацией.",
        "card_not_yours" => "Это не твоя карта.",
        "card_blocked" | "no_card" => "Карта заблокирована. Новую выдаст банкомат.",
        "limit" => "Дневной лимит карты исчерпан.",
        "no_money" => "На счёте не хватает АР.",
        "no_recipient" => "Такого счёта нет.",
        "own_terminal" => "Нельзя платить на свой же счёт.",
        "own_account" => "Это тот же счёт, с которого переводишь.",
        "bad_amount" => "Такую сумму перевести нельзя.",
        "recipient_frozen" => "Счёт получателя заморожен.",
        "bad_number" => "Номер счёта выглядит как MH-1234-5678.",
        "op_conflict" => "Операция уже проведена.",
        "too_many" => "Дополнительных счетов можно открыть не больше трёх.",
        "bad_title" => "Название счёта: от 1 до 40 символов.",
        _ => return format!("Банк не ответил ({code}). Попробуй ещё раз."),
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use steel_registry::vanilla_items;
    use text_components::resolving::NoResolutor;

    use super::*;

    #[test]
    fn a_title_is_the_background_in_our_font_and_then_the_text() {
        let title = title(ATM, "Банкомат");
        assert_eq!(
            title.to_plain(&NoResolutor),
            "\u{F000}\u{E000}\u{F001}Банкомат"
        );
    }

    #[test]
    fn a_button_has_its_name_and_its_lore() {
        steel_registry::init_vanilla_registry();
        let button = button(&vanilla_items::BARRIER, "Закрыть", &["раз", "два"]);
        assert!(button.is(&vanilla_items::BARRIER));
        assert_eq!(
            button
                .get(ITEM_NAME)
                .map(|name| name.to_plain(&NoResolutor)),
            Some("Закрыть".to_owned())
        );
        assert_eq!(button.get(LORE).map(|lore| lore.lines().len()), Some(2));
    }

    #[test]
    fn error_codes_have_their_russian_lines() {
        assert_eq!(error_text("no_money"), "На счёте не хватает АР.");
        assert_eq!(error_text("no_card"), error_text("card_blocked"));
        assert_eq!(
            error_text("offline"),
            "Банк не ответил (offline). Попробуй ещё раз."
        );
    }
}
