//! `/bank banker`: puts a banker where the admin stands, facing the admin, or takes the nearest
//! one away. For `minehouse.bank.banker`.

use std::sync::Arc;

use steel_core::{
    command::{
        CommandArgument, CommandContext, CommandError, CommandNode, CommandRegistration,
        CommandRegistrationError, CommandRegistry, CommandSource, argument, literal,
    },
    entity::Entity as _,
    permission::{PermissionExpr, PermissionKey},
    player::Player,
};
use steel_utils::Identifier;
use text_components::{Modifier as _, TextComponent, format::Color};

use super::{Banker, Skin, skin};
use crate::bank::{Bank, bank, gui};

const PERMISSION: &str = "minehouse.bank.banker";

/// `/bank`, for those who may put bankers.
pub(crate) fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "bank"), tree).default_access(),
    )?;
    Ok(())
}

fn allowed(source: CommandSource<'_>) -> bool {
    let Some(player) = source.player() else {
        return false;
    };
    PermissionKey::parse(PERMISSION)
        .ok()
        .map(PermissionExpr::key)
        .is_some_and(|permission| player.has_permission(&permission))
}

fn tree() -> CommandNode {
    let banker = literal("banker")
        .executes(|context| put(*context, Skin::default_skin()))
        .then(
            literal("url").then(
                argument("link", CommandArgument::greedy_string())
                    .executes(|context| from_url(*context, context.string("link")?)),
            ),
        )
        .then(literal("remove").executes(|context| remove(*context)))
        .then(
            argument("skin", CommandArgument::word()).executes(|context| {
                let name = context.string("skin")?.to_owned();
                put(*context, Skin::Name { name })
            }),
        );
    literal("bank").requires(allowed).then(banker)
}

fn player(context: CommandContext<'_>) -> Result<&Arc<Player>, CommandError> {
    context
        .source()
        .player()
        .ok_or_else(|| CommandError::from("Только для игроков"))
}

fn running() -> Result<&'static Arc<Bank>, CommandError> {
    bank().ok_or_else(|| CommandError::from("Банк пока закрыт."))
}

/// A banker where the player stands, turned to face them.
fn standing(player: &Player, skin: Skin) -> Banker {
    let position = player.position();
    let mut yaw = (player.rotation().0 + 180.0).rem_euclid(360.0);
    if yaw >= 180.0 {
        yaw -= 360.0;
    }
    Banker {
        world: player.get_world().key.to_string(),
        x: position.x,
        y: position.y,
        z: position.z,
        yaw,
        skin,
    }
}

fn put(context: CommandContext<'_>, skin: Skin) -> Result<i32, CommandError> {
    let player = player(context)?;
    running()?.bankers.place(standing(player, skin));
    let text = "Банкир поставлен. ПКМ по нему — разговор.";
    context
        .source()
        .send_success(&TextComponent::plain(text).color(Color::Green), true);
    Ok(1)
}

/// `/bank banker url <link>`: the skin PNG at the link; the banker appears where the admin
/// stood when the skin is ready.
fn from_url(context: CommandContext<'_>, link: &str) -> Result<i32, CommandError> {
    let player = player(context)?;
    let bank = running()?;
    let url = link.trim().to_owned();
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(CommandError::from(
            "Нужна ссылка на PNG скина, например https://s.namemc.com/i/….png",
        ));
    }
    let uuid = player.gameprofile.id();
    let place = standing(player, Skin::default_skin());
    context
        .source()
        .send_success(&TextComponent::plain("Загружаю скин…"), false);
    let later = Arc::clone(bank);
    bank.runtime.spawn(async move {
        let loaded = skin::from_url(url.clone()).await;
        let bank = Arc::clone(&later);
        later.later(move |server| {
            let admin = server
                .get_players()
                .into_iter()
                .find(|player| player.gameprofile.id() == uuid);
            match loaded {
                Ok(skin) => {
                    bank.bankers.place(Banker { skin, ..place });
                    if let Some(admin) = admin {
                        gui::info(&admin, "Банкир поставлен. ПКМ по нему — разговор.");
                    }
                }
                Err(error) => {
                    log::warn!("Bank: the banker's skin from {url} failed: {error}");
                    if let Some(admin) = admin {
                        gui::error(&admin, &format!("Скин не загрузился: {error}"));
                    }
                }
            }
        });
    });
    Ok(1)
}

fn remove(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let player = player(context)?;
    let world = player.get_world().key.to_string();
    if !running()?.bankers.remove_near(&world, player.position()) {
        return Err(CommandError::from(
            "Рядом нет банкира: подойди к нему ближе.",
        ));
    }
    context.source().send_success(
        &TextComponent::plain("Банкир убран.").color(Color::Green),
        true,
    );
    Ok(1)
}
