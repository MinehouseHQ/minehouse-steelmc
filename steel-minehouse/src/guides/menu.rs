//! «Меню сервера» (the Fabric `ServerMenu`): a vanilla dialog. It is in the dialog tag
//! `minecraft:pause_screen_additions`, so every client shows a «Меню сервера» button in the
//! pause screen with no mod of ours, and `/menu` opens the same dialog. A module adds its
//! lines to [`ENTRIES`]; the dialog shows them as buttons under the logo of the server (a
//! picture of the font `minehouse-guides:menu` of the server pack).
//!
//! The copy in the pause screen goes out with the registries when a player joins, before
//! anything is known of them, so it has every line; `/menu` shows the lines that work now. A
//! click on a line that does not work says so.

use std::sync::LazyLock;

use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_core::{
    command::{
        CommandError, CommandNode, CommandRegistration, CommandRegistrationError, CommandRegistry,
        literal,
    },
    entity::Entity as _,
    inventory::container::Container as _,
    player::{Player, player_inventory::Prediction},
};
use steel_protocol::packets::game::{CShowDialog, SoundSource};
use steel_registry::{
    Registry, TaggedRegistryExt as _,
    dialog::{Dialog, DialogVariant, ExitAction},
    sound_event::SoundEvent,
    sound_events,
};
use steel_utils::Identifier;
use text_components::{Modifier as _, TextComponent, format::Color};

use super::book;
use crate::{
    bank::{bank, transfer, zones},
    moderation::format::render,
};

/// The name of the menu, in the pause screen and over the dialog.
pub const TITLE: &str = "Меню сервера";
const NAMESPACE: &str = "minehouse-guides";
/// The dialog of the pause screen, as on the Fabric server.
const KEY: &str = "server_menu";
/// What comes before the id of a line in the id of its click.
const ACTION: &str = "menu/";
const FONT: &str = "minehouse-guides:menu";
/// The picture of the logo: 192 by 48 from the top of the line down; the empty lines
/// after it make room.
const LOGO: &str = "\u{E002}";
const LOGO_LINES: usize = 5;
const BUTTON_WIDTH: i32 = 150;
const BODY_WIDTH: i32 = 200;

/// A line of the menu.
pub struct Entry {
    /// What the click carries.
    pub id: &'static str,
    /// The words on the button.
    pub name: &'static str,
    /// The words under the pointer.
    pub lore: &'static str,
    /// Whether the line works now.
    pub visible: fn() -> bool,
    /// What a click does.
    pub action: fn(&Player),
}

fn bank_open() -> bool {
    bank().is_some()
}

fn zones_open() -> bool {
    bank().is_some_and(|bank| bank.zones_open())
}

/// The lines of the menu, in order.
pub static ENTRIES: &[Entry] = &[
    Entry {
        id: "guide-book",
        name: "Книга гайдов",
        lore: "Все гайды MineHouse в одной книге.\nНажми, чтобы получить её.",
        visible: || true,
        action: give_book,
    },
    Entry {
        id: "bank-transfer",
        name: "Переводы",
        lore: "Перевести АР со своего счёта\nна счёт другого игрока по номеру.",
        visible: bank_open,
        action: transfer::open,
    },
    Entry {
        id: "spawn-zones",
        name: "Зоны спавна",
        lore: "Аукцион и аренда земли\nна острове спавна.",
        visible: zones_open,
        action: zones::gui::open,
    },
];

fn text(text: &str) -> NbtTag {
    TextComponent::plain(text.to_owned()).to_codec_nbt()
}

fn message(contents: TextComponent) -> NbtCompound {
    let mut message = NbtCompound::new();
    message.insert("type", "minecraft:plain_message");
    message.insert("contents", contents.to_codec_nbt());
    message.insert("width", BODY_WIDTH);
    message
}

fn button(entry: &Entry) -> NbtCompound {
    let mut click = NbtCompound::new();
    click.insert("type", "minecraft:custom");
    click.insert("id", format!("{NAMESPACE}:{ACTION}{}", entry.id));
    let mut button = NbtCompound::new();
    button.insert("label", text(entry.name));
    if !entry.lore.is_empty() {
        button.insert("tooltip", text(entry.lore));
    }
    button.insert("width", BUTTON_WIDTH);
    button.insert("action", NbtTag::Compound(click));
    button
}

fn close_button() -> NbtCompound {
    let mut button = NbtCompound::new();
    button.insert("label", text("Закрыть"));
    button.insert("width", BUTTON_WIDTH);
    button
}

/// The menu with the lines `entries`, as vanilla's `Dialog.DIRECT_CODEC` reads it: a
/// `multi_action` dialog in one column.
fn dialog<'a>(entries: impl Iterator<Item = &'a Entry>) -> NbtTag {
    let mut logo = TextComponent::plain(format!("{LOGO}{}", "\n".repeat(LOGO_LINES)))
        .color(Color::Rgb(255, 255, 255));
    logo.format = logo.format.font(FONT.to_owned());
    let mut body = vec![message(logo)];
    let mut actions: Vec<NbtCompound> = entries.map(button).collect();
    if actions.is_empty() {
        body.push(message(
            TextComponent::plain("Пока пусто").color(Color::Gray),
        ));
        actions.push(close_button());
    }

    let mut dialog = NbtCompound::new();
    dialog.insert("type", "minecraft:multi_action");
    dialog.insert("title", text(TITLE));
    dialog.insert("external_title", text(TITLE));
    dialog.insert("can_close_with_escape", true);
    dialog.insert("pause", false);
    dialog.insert("after_action", "close");
    dialog.insert("body", NbtTag::List(NbtList::Compound(body)));
    dialog.insert("actions", NbtTag::List(NbtList::Compound(actions)));
    dialog.insert("exit_action", NbtTag::Compound(close_button()));
    dialog.insert("columns", 1_i32);
    NbtTag::Compound(dialog)
}

fn pause_screen() -> NbtTag {
    dialog(ENTRIES.iter())
}

/// The entry of the dialog registry: what every client keeps for its pause screen.
static PAUSE_DIALOG: LazyLock<Dialog> = LazyLock::new(|| Dialog {
    key: Identifier::new(NAMESPACE, KEY),
    button_width: BUTTON_WIDTH,
    columns: 1,
    exit_action: ExitAction {
        label: TextComponent::plain("Закрыть"),
        width: BUTTON_WIDTH,
    },
    external_title: TextComponent::plain(TITLE),
    title: TextComponent::plain(TITLE),
    variant: DialogVariant::Custom { nbt: pause_screen },
});

/// Adds the menu to the dialogs and to the pause screen. Given to
/// [`steel_registry::extend_registry`].
pub(crate) fn register_dialog(registry: &mut Registry) {
    registry.dialogs.register(&PAUSE_DIALOG);
    let tag = Identifier::vanilla_static("pause_screen_additions");
    registry.dialogs.modify_tag(&tag, |mut keys| {
        keys.push(PAUSE_DIALOG.key.clone());
        keys
    });
}

/// Opens the menu for a player, with the lines that work now.
pub fn open(player: &Player) {
    let entries = ENTRIES.iter().filter(|entry| (entry.visible)());
    player.send_packet(CShowDialog::new(dialog(entries)));
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

/// What a click is to the menu.
enum Click {
    /// Not of the menu.
    Other,
    /// Of the menu, but of no line it has.
    Unknown,
    /// Of this line.
    Line(&'static Entry),
}

fn click_of(id: &Identifier) -> Click {
    if id.namespace != NAMESPACE {
        return Click::Other;
    }
    let Some(line) = id.path.strip_prefix(ACTION) else {
        return Click::Other;
    };
    ENTRIES
        .iter()
        .find(|entry| entry.id == line)
        .map_or(Click::Unknown, Click::Line)
}

/// A click on a button of the menu. `true` if `id` was one.
pub(crate) fn clicked(player: &Player, id: &Identifier) -> bool {
    let entry = match click_of(id) {
        Click::Other => return false,
        Click::Unknown => return true,
        Click::Line(entry) => entry,
    };
    if !(entry.visible)() {
        // The pause screen has every line, also of what is closed now.
        player.send_message(&render("&eЭтот пункт сейчас недоступен.", &[]));
        return true;
    }
    sound(player, &sound_events::UI_BUTTON_CLICK, 0.4, 1.0);
    (entry.action)(player);
    true
}

fn give_book(player: &Player) {
    let (has, room) = {
        let inventory = player.inventory.lock();
        let has =
            (0..inventory.get_container_size()).any(|slot| book::is_book(inventory.get_item(slot)));
        (has, inventory.get_free_slot().is_some())
    };
    if has {
        player.send_message(&render("&eКнига гайдов уже у тебя в инвентаре.", &[]));
        return;
    }
    if !room {
        player.send_message(&render(
            "&cОсвободи место в инвентаре, и книга поместится.",
            &[],
        ));
        return;
    }
    player.add_item_or_drop(book::create(), Prediction::ServerOnly);
    player.send_message(&render(
        "&aДержи книгу «{title}»: ПКМ, чтобы открыть.",
        &[("title", book::TITLE)],
    ));
    sound(player, &sound_events::ENTITY_ITEM_PICKUP, 0.5, 1.2);
}

/// `/menu`.
pub(super) fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "menu"), tree).default_access(),
    )?;
    Ok(())
}

fn tree() -> CommandNode {
    literal("menu").executes(|context| {
        let source = context.source();
        let Some(player) = source.player() else {
            return Err(CommandError::from("Меню открывается только игроку."));
        };
        open(player);
        Ok(1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions(dialog: &NbtTag) -> Vec<String> {
        let NbtTag::Compound(dialog) = dialog else {
            panic!("a dialog is a compound");
        };
        let Some(NbtTag::List(NbtList::Compound(actions))) = dialog.get("actions") else {
            panic!("a multi_action dialog has actions");
        };
        actions
            .iter()
            .map(|button| {
                button
                    .compound("action")
                    .and_then(|action| action.string("id"))
                    .map_or_else(String::new, ToString::to_string)
            })
            .collect()
    }

    #[test]
    fn the_pause_screen_has_every_line() {
        let dialog = pause_screen();
        assert_eq!(
            actions(&dialog),
            [
                "minehouse-guides:menu/guide-book",
                "minehouse-guides:menu/bank-transfer",
                "minehouse-guides:menu/spawn-zones",
            ]
        );
        let NbtTag::Compound(dialog) = dialog else {
            panic!("a dialog is a compound");
        };
        assert_eq!(
            dialog.string("type").map(ToString::to_string).as_deref(),
            Some("minecraft:multi_action")
        );
        assert!(dialog.contains("exit_action"));
        assert_eq!(dialog.int("columns"), Some(1));
    }

    #[test]
    fn an_empty_menu_says_so_and_can_be_closed() {
        let dialog = dialog([].iter());
        // The one action is the close button, which has no click of ours.
        assert_eq!(actions(&dialog), [String::new()]);
        assert!(format!("{dialog:?}").contains("Пока пусто"));
    }

    #[test]
    fn a_click_is_told_from_the_clicks_of_others() {
        let click = |path: &str| Identifier::new(NAMESPACE, path.to_owned());
        assert!(matches!(
            click_of(&Identifier::new("minehouse", "nametags/hide")),
            Click::Other
        ));
        assert!(matches!(click_of(&click("other")), Click::Other));
        assert!(matches!(click_of(&click("menu/nope")), Click::Unknown));
        let Click::Line(book) = click_of(&click("menu/guide-book")) else {
            panic!("the book is a line of the menu");
        };
        assert_eq!(book.name, "Книга гайдов");
    }

    #[test]
    fn the_registry_entry_goes_out_as_the_menu() {
        use simdnbt::ToNbtTag as _;
        let sent = (&*PAUSE_DIALOG).to_nbt_tag();
        assert_eq!(actions(&sent).len(), ENTRIES.len());
    }
}
