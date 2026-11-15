//! Vanilla `DialogCommand`.

use std::sync::Arc;

use steel_protocol::packets::common::CClearDialog;
use steel_protocol::packets::game::CShowDialog;
use steel_registry::{REGISTRY, RegistryExt as _};
use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandDialog, CommandSource, ElementType, ResponseTracker, SteelArgumentType,
        SteelCommandContext, SteelCommandRuntime, argument, literal,
    },
    registration::CommandRegistration,
};
use crate::entity::Entity as _;
use crate::player::Player;

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;
type Context = SteelCommandContext<CommandSource>;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("dialog"), |_| command())
}

fn command() -> Builder {
    literal("dialog")
        .then(
            literal("show").then(
                argument("targets", SteelArgumentType::players()).then(
                    argument("dialog", SteelArgumentType::dialog())
                        .executes(|context| show_dialog(context, context.dialog("dialog")?)),
                ),
            ),
        )
        .then(
            literal("clear")
                .then(argument("targets", SteelArgumentType::players()).executes(clear_dialog)),
        )
}

/// Vanilla `showDialog`, with `ServerPlayer.openDialog` sending the dialog's
/// holder: a registry dialog by id, an inline one as its NBT.
fn show_dialog(context: &Context, dialog: CommandDialog) -> Result<i32, CommandSyntaxError> {
    let players = context.players("targets")?;
    let packet = match dialog {
        CommandDialog::Reference(dialog) => {
            let Some(id) = REGISTRY
                .dialogs
                .id_from_key(&dialog.key)
                .and_then(|id| i32::try_from(id).ok())
            else {
                unreachable!("parsed dialogs come from the registry")
            };
            CShowDialog::reference(id)
        }
        CommandDialog::Direct(dialog) => CShowDialog::new(dialog),
    };
    let mut tracker = ResponseTracker::default();
    for player in &players {
        player.send_packet(packet.clone());
        tracker.track_one(player);
    }
    tracker.send_feedback(
        context.source(),
        true,
        ElementType::NonZero,
        None,
        |player: &Arc<Player>, _| {
            translations::COMMANDS_DIALOG_SHOW_SINGLE
                .message([player.display_name()])
                .component()
        },
        |count, _| {
            translations::COMMANDS_DIALOG_SHOW_MULTIPLE
                .message([plain(count)])
                .component()
        },
    )
}

/// Vanilla `clearDialog`.
fn clear_dialog(context: &Context) -> Result<i32, CommandSyntaxError> {
    let players = context.players("targets")?;
    let mut tracker = ResponseTracker::default();
    for player in &players {
        player.send_packet(CClearDialog {});
        tracker.track_one(player);
    }
    tracker.send_feedback(
        context.source(),
        true,
        ElementType::NonZero,
        None,
        |player: &Arc<Player>, _| {
            translations::COMMANDS_DIALOG_CLEAR_SINGLE
                .message([player.display_name()])
                .component()
        },
        |count, _| {
            translations::COMMANDS_DIALOG_CLEAR_MULTIPLE
                .message([plain(count)])
                .component()
        },
    )
}

fn plain(value: i32) -> TextComponent {
    TextComponent::plain(value.to_string())
}
