//! `/zones` (the Fabric `ZoneCommands`): the list for players, `/zones bid` from inside a
//! zone, and for admins the auctions (`auction`, `delete`, `reload`).
//!
//! `auction` also asks for `minehouse.zones.auction`, so a moderator may run auctions without
//! the rest of the admin commands.

use std::sync::Arc;

use chrono::Utc;
use serde_json::{Value, json};
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

use super::{gui, left, markup, standing_in};
use crate::bank::{Bank, bank, error, ok, ores::give_stack};

const ADMIN: &str = "minehouse.zones.admin";
const AUCTION: &str = "minehouse.zones.auction";

fn has(player: &Player, permission: &str) -> bool {
    PermissionKey::parse(permission)
        .ok()
        .map(PermissionExpr::key)
        .is_some_and(|permission| player.has_permission(&permission))
}

/// Whether the player may mark and manage zones.
pub(super) fn is_admin(player: &Player) -> bool {
    has(player, ADMIN)
}

/// The console and admins.
fn admin(source: CommandSource<'_>) -> bool {
    source.player().is_none_or(|player| is_admin(player))
}

fn auctioneer(source: CommandSource<'_>) -> bool {
    source
        .player()
        .is_none_or(|player| is_admin(player) || has(player, AUCTION))
}

/// `/zones`, for every player.
pub(crate) fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "zones"), tree).default_access(),
    )?;
    Ok(())
}

fn tree() -> CommandNode {
    let delay = argument("delay", CommandArgument::integer(0, 20160))
        .executes(|context| auction(*context, true, true, true));
    let price = argument("price", CommandArgument::integer(1, 1_000_000))
        .executes(|context| auction(*context, true, true, false))
        .then(delay);
    let hours = argument("hours", CommandArgument::integer(1, 336))
        .executes(|context| auction(*context, true, false, false))
        .then(price);
    let num = argument("num", CommandArgument::integer(1, 99))
        .executes(|context| auction(*context, false, false, false))
        .then(hours);
    literal("zones")
        .executes(|context| list(*context))
        .then(literal("bid").executes(|context| bid(*context)))
        .then(literal("bossbar").executes(|context| bossbar(*context)))
        .then(
            literal("show")
                .executes(|context| show(*context, None))
                .then(
                    argument("num", CommandArgument::integer(0, 99))
                        .executes(|context| show(*context, Some(context.integer("num")?))),
                ),
        )
        .then(
            literal("wand")
                .requires(admin)
                .executes(|context| wand(*context)),
        )
        .then(
            literal("clear")
                .requires(admin)
                .executes(|context| clear(*context)),
        )
        .then(
            literal("height").requires(admin).then(
                argument("min", CommandArgument::integer(-64, 320)).then(
                    argument("max", CommandArgument::integer(-64, 320))
                        .executes(|context| height(*context)),
                ),
            ),
        )
        .then(
            literal("circle").requires(admin).then(
                argument("radius", CommandArgument::integer(3, 500))
                    .executes(|context| circle(*context)),
            ),
        )
        .then(literal("edit").requires(admin).then(
            argument("num", CommandArgument::integer(0, 99)).executes(|context| edit(*context)),
        ))
        .then(literal("save").requires(admin).then(saving(1, false)))
        .then(literal("savecommon").requires(admin).then(saving(0, true)))
        .then(literal("auction").requires(auctioneer).then(num))
        .then(literal("delete").requires(admin).then(
            argument("num", CommandArgument::integer(0, 99)).executes(|context| delete(*context)),
        ))
        .then(
            literal("reload")
                .requires(admin)
                .executes(|context| reload(*context)),
        )
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

fn list(context: CommandContext<'_>) -> Result<i32, CommandError> {
    gui::open(player(context)?);
    Ok(1)
}

/// `/zones bid`: the bid window of the zone the player stands in.
fn bid(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let player = player(context)?;
    let bank = running()?;
    if !bank.zones_open() && !is_admin(player) {
        return Err(CommandError::from("Зоны спавна пока закрыты."));
    }
    let all = bank.zones.all();
    let now = Utc::now();
    let zone = standing_in(&all, player);
    match zone {
        Some(zone) if zone.auction_open(now) => {
            gui::bid_window(bank, player, zone, all.to_vec());
            Ok(1)
        }
        Some(zone) if zone.auction_soon(now) => {
            let starts = zone
                .auction
                .as_ref()
                .map_or_default(|auction| left(auction.starts_at, now));
            Err(CommandError::from(format!(
                "Аукцион по этой зоне начнётся через {starts}."
            )))
        }
        _ => Err(CommandError::from(
            "Встань в зону, по которой идёт аукцион: ставку можно сделать только стоя в ней. \
             Список зон: /zones",
        )),
    }
}

/// `/zones bossbar`: hides the auction bars for this player, or shows them again.
fn bossbar(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let shown = running()?.zones.bars.toggle(player(context)?);
    let text = if shown {
        "Полоски аукционов снова видны."
    } else {
        "Полоски аукционов скрыты. Вернуть: /zones bossbar"
    };
    context
        .source()
        .send_success(&TextComponent::plain(text).color(Color::Green), false);
    Ok(1)
}

fn done(context: CommandContext<'_>, text: String) -> i32 {
    context
        .source()
        .send_success(&TextComponent::plain(text).color(Color::Green), false);
    1
}

/// `/zones show [num]`: the borders as particles for half a minute.
fn show(context: CommandContext<'_>, num: Option<i32>) -> Result<i32, CommandError> {
    let player = player(context)?;
    let bank = running()?;
    bank.zones.markup.show(player.gameprofile.id(), num);
    let text = num.map_or_else(
        || "Границы зон видны 30 секунд.".to_owned(),
        |num| format!("Граница зоны {num} видна 30 секунд."),
    );
    Ok(done(context, text))
}

fn wand(context: CommandContext<'_>) -> Result<i32, CommandError> {
    give_stack(player(context)?, markup::wand());
    let text = "Палка разметки в инвентаре. ПКМ по блоку — вершина, Shift+ПКМ — убрать последнюю.";
    Ok(done(context, text.to_owned()))
}

fn clear(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let player = player(context)?;
    running()?.zones.markup.clear(player.gameprofile.id());
    Ok(done(context, "Черновик очищен.".to_owned()))
}

fn height(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let player = player(context)?;
    let (min, max) = (context.integer("min")?, context.integer("max")?);
    if min >= max {
        return Err(CommandError::from(
            "Нижняя высота должна быть меньше верхней.",
        ));
    }
    running()?
        .zones
        .markup
        .edit(player.gameprofile.id(), |draft| {
            draft.y_min = min;
            draft.y_max = max;
        });
    Ok(done(context, format!("Высоты черновика: {min}–{max}.")))
}

fn circle(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let player = player(context)?;
    let radius = context.integer("radius")?;
    let at = player.block_position();
    let world = player.get_world().key.to_string();
    running()?
        .zones
        .markup
        .edit(player.gameprofile.id(), |draft| {
            draft.circle(&world, at.x(), at.z(), radius);
        });
    Ok(done(
        context,
        format!(
            "Круг радиусом {radius} вокруг тебя в черновике. Сохранить как общий: /zones \
             savecommon 0"
        ),
    ))
}

fn edit(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let player = player(context)?;
    let bank = running()?;
    let zone = bank
        .zones
        .by_num(context.integer("num")?)
        .ok_or_else(|| CommandError::from("Такой зоны нет."))?;
    bank.zones
        .markup
        .edit(player.gameprofile.id(), |draft| draft.take(&zone));
    Ok(done(
        context,
        format!(
            "{} в черновике ({} вершин). Поправь палкой и сохрани тем же номером.",
            zone.name(),
            zone.points.len()
        ),
    ))
}

/// `<num> [title]` of `save` and `savecommon`.
fn saving(least: i32, common: bool) -> CommandNode {
    argument("num", CommandArgument::integer(least, 99))
        .executes(move |context| save(*context, common, None))
        .then(
            argument("title", CommandArgument::greedy_string()).executes(move |context| {
                let title = context.string("title")?;
                save(*context, common, Some(title))
            }),
        )
}

fn save(
    context: CommandContext<'_>,
    common: bool,
    title: Option<&str>,
) -> Result<i32, CommandError> {
    let player = player(context)?;
    let draft = running()?
        .zones
        .markup
        .draft(player.gameprofile.id())
        .filter(|draft| draft.points.len() >= 3)
        .ok_or_else(|| {
            CommandError::from("Нужно хотя бы 3 вершины: /zones wand и ПКМ по блокам.")
        })?;
    let num = context.integer("num")?;
    let kind = if common {
        "Общая зона"
    } else {
        "Зона"
    };
    admin_call(
        context,
        "zone_save",
        &draft.body(num, common, title),
        format!("{kind} {num} сохранена. Черновик можно очистить: /zones clear"),
    )
}

/// What an admin command says once the site has answered.
fn answered(answer: &Value, done: &str) -> Result<String, String> {
    if ok(answer) {
        Ok(done.to_owned())
    } else {
        Err(gui::error_text(error(answer)))
    }
}

/// Sends an admin action to the site, answers the sender and reloads the list.
fn admin_call(
    context: CommandContext<'_>,
    action: &str,
    body: &Value,
    done: String,
) -> Result<i32, CommandError> {
    let bank = running()?;
    let source = context.source().detach();
    bank.call(action, body, move |_, answer| {
        match answered(&answer, &done) {
            Ok(text) => source
                .source()
                .send_success(&TextComponent::plain(text).color(Color::Green), true),
            Err(text) => source.source().send_failure(TextComponent::plain(text)),
        }
        bank.refresh_zones(Some(Box::new(|_, _| {})));
    });
    Ok(1)
}

/// What `/zones auction` says on success.
fn auction_started(hours: i32, delay: i32) -> String {
    if delay > 0 {
        format!("Аукцион объявлен: начнётся через {delay} мин, продлится {hours} ч.")
    } else {
        format!("Аукцион запущен на {hours} ч.")
    }
}

fn auction(
    context: CommandContext<'_>,
    with_hours: bool,
    with_price: bool,
    with_delay: bool,
) -> Result<i32, CommandError> {
    let num = context.integer("num")?;
    let hours = if with_hours {
        context.integer("hours")?
    } else {
        48
    };
    let mut body = json!({"num": num, "hours": hours});
    if with_price {
        body["price"] = json!(context.integer("price")?);
    }
    let delay = if with_delay {
        context.integer("delay")?
    } else {
        0
    };
    if delay > 0 {
        body["delay"] = json!(delay);
    }
    admin_call(
        context,
        "zone_auction",
        &body,
        auction_started(hours, delay),
    )
}

fn delete(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let body = json!({"num": context.integer("num")?});
    admin_call(context, "zone_delete", &body, "Зона удалена.".to_owned())
}

fn reload(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let bank = running()?;
    let source = context.source().detach();
    bank.refresh_zones(Some(Box::new(move |_, answer| {
        if ok(answer) {
            let count = bank.zones.all().len();
            let text = TextComponent::plain(format!("Зоны загружены: {count}"));
            source
                .source()
                .send_success(&text.color(Color::Green), false);
        } else {
            let text = format!("Сайт не ответил: {}", error(answer));
            source.source().send_failure(TextComponent::plain(text));
        }
    })));
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_auction_is_announced_or_started() {
        assert_eq!(auction_started(48, 0), "Аукцион запущен на 48 ч.");
        assert_eq!(
            auction_started(24, 30),
            "Аукцион объявлен: начнётся через 30 мин, продлится 24 ч."
        );
    }

    #[test]
    fn an_admin_hears_what_the_site_refused() {
        let refused = json!({"ok": false, "error": "leased"});
        assert!(answered(&refused, "Зона удалена.").is_err());
        assert!(answered(&json!({"ok": true}), "Зона удалена.").is_ok());
    }
}
