//! `/g`, `/l`, `/msg` (`/tell`, `/w`, `/m`), `/reply` (`/r`), `/ignore`: open to every
//! player. `/chat reload`, `/mute`, `/tempmute`, `/unmute`, `/mutelist`: the console, or a
//! player with `minehouse.command.<name>`.

use std::sync::Arc;

use steel_core::{
    command::{
        CommandArgument, CommandContext, CommandError, CommandNode, CommandRegistration,
        CommandRegistrationError, CommandRegistry, CommandSource, argument, literal,
    },
    player::Player,
    server::Server,
};
use steel_utils::Identifier;
use uuid::Uuid;

use super::{Chat, mute_text, now_millis, store::Mute};
use crate::moderation::{
    commands::{WordParser, player, word},
    format::{parse_duration, render},
};

const NAMESPACE: &str = "minehouse";

type Handler = fn(&Arc<Chat>, CommandContext<'_>) -> Result<i32, CommandError>;

pub(super) fn register(
    registry: &mut CommandRegistry,
    chat: &Arc<Chat>,
) -> Result<(), CommandRegistrationError> {
    let command = |name: &'static str, tree: fn(Arc<Chat>) -> CommandNode| {
        let chat = Arc::clone(chat);
        CommandRegistration::new(Identifier::new(NAMESPACE, name), move || tree(chat))
    };
    registry
        .register(command("g", |chat| channel_tree(chat, "g", true)).default_access())?
        .register(command("l", |chat| channel_tree(chat, "l", false)).default_access())?
        // Vanilla's /msg, /tell and /w would bypass our formats, ignores and mutes.
        .register(
            command("msg", msg_tree)
                .alias("tell")
                .alias("w")
                .alias("m")
                .default_access(),
        )?
        .register(command("reply", reply_tree).alias("r").default_access())?
        .register(command("ignore", ignore_tree).default_access())?
        .register(command("chat", chat_tree))?
        .register(command("mute", mute_tree))?
        .register(command("tempmute", tempmute_tree))?
        .register(command("unmute", unmute_tree))?
        .register(command("mutelist", mutelist_tree))?;
    Ok(())
}

fn run(
    chat: &Arc<Chat>,
    handler: Handler,
) -> impl for<'context> Fn(&CommandContext<'context>) -> Result<i32, CommandError> + Send + Sync + 'static
{
    let chat = Arc::clone(chat);
    move |context| handler(&chat, *context)
}

fn message() -> CommandNode {
    argument("message", CommandArgument::greedy_string())
}

fn reason() -> CommandNode {
    argument("reason", CommandArgument::greedy_string())
}

fn channel_tree(chat: Arc<Chat>, name: &'static str, global: bool) -> CommandNode {
    let (switch, send): (Handler, Handler) = if global {
        (
            |chat, context| switch_channel(chat, context, true),
            |chat, context| send_to_channel(chat, context, true),
        )
    } else {
        (
            |chat, context| switch_channel(chat, context, false),
            |chat, context| send_to_channel(chat, context, false),
        )
    };
    literal(name)
        .executes(run(&chat, switch))
        .then(message().executes(run(&chat, send)))
}

fn msg_tree(chat: Arc<Chat>) -> CommandNode {
    literal("msg").then(player().then(message().executes(run(&chat, private_message))))
}

fn reply_tree(chat: Arc<Chat>) -> CommandNode {
    literal("reply").then(message().executes(run(&chat, reply)))
}

fn ignore_tree(chat: Arc<Chat>) -> CommandNode {
    literal("ignore")
        .executes(run(&chat, |chat, context| {
            chat.list_ignores(sender(context)?);
            Ok(1)
        }))
        .then(player().executes(run(&chat, toggle_ignore)))
}

fn chat_tree(chat: Arc<Chat>) -> CommandNode {
    literal("chat").then(literal("reload").executes(run(&chat, |chat, context| {
        Ok(reload(chat, context.source()))
    })))
}

fn mute_tree(chat: Arc<Chat>) -> CommandNode {
    literal("mute").then(
        player()
            .executes(run(&chat, |chat, context| mute(chat, context, None, "")))
            .then(reason().executes(run(&chat, |chat, context| {
                mute(chat, context, None, context.string("reason")?)
            }))),
    )
}

fn tempmute_tree(chat: Arc<Chat>) -> CommandNode {
    let duration = argument("duration", CommandArgument::custom(WordParser::Duration));
    literal("tempmute").then(
        player().then(
            duration
                .executes(run(&chat, |chat, context| tempmute(chat, context, "")))
                .then(reason().executes(run(&chat, |chat, context| {
                    tempmute(chat, context, context.string("reason")?)
                }))),
        ),
    )
}

fn unmute_tree(chat: Arc<Chat>) -> CommandNode {
    literal("unmute").then(player().executes(run(&chat, unmute)))
}

fn mutelist_tree(chat: Arc<Chat>) -> CommandNode {
    literal("mutelist").executes(run(&chat, |chat, context| {
        Ok(mute_list(chat, context.source()))
    }))
}

fn fail(text: impl Into<String>) -> CommandError {
    CommandError::from(text.into())
}

/// The player who typed the command; the console has no chat channel to switch.
fn sender(context: CommandContext<'_>) -> Result<&Arc<Player>, CommandError> {
    context
        .source()
        .player()
        .ok_or_else(|| fail("Эта команда только для игроков."))
}

fn online(server: &Server, name: &str) -> Option<Arc<Player>> {
    server
        .get_players()
        .into_iter()
        .find(|player| player.gameprofile.name().eq_ignore_ascii_case(name))
}

/// A config text with `&` colors as a command error.
fn config_error(chat: &Chat, key: &str, values: &[(&str, &str)]) -> CommandError {
    CommandError::new(render(chat.config().text(key), values))
}

fn switch_channel(
    chat: &Arc<Chat>,
    context: CommandContext<'_>,
    global: bool,
) -> Result<i32, CommandError> {
    chat.set_global_mode(sender(context)?, global);
    Ok(1)
}

fn send_to_channel(
    chat: &Arc<Chat>,
    context: CommandContext<'_>,
    global: bool,
) -> Result<i32, CommandError> {
    let text = context.string("message")?.trim();
    if !text.is_empty() {
        chat.send(context.source().server(), sender(context)?, text, global);
    }
    Ok(1)
}

fn private_message(chat: &Arc<Chat>, context: CommandContext<'_>) -> Result<i32, CommandError> {
    let from = sender(context)?;
    let name = word(context, "player")?;
    let server = context.source().server();
    let Some(target) = online(server, &name) else {
        return Err(config_error(chat, "pm.not_found", &[("target", &name)]));
    };
    chat.private_message(server, from, &target, context.string("message")?.trim());
    Ok(1)
}

fn reply(chat: &Arc<Chat>, context: CommandContext<'_>) -> Result<i32, CommandError> {
    let text = context.string("message")?.trim();
    chat.reply(context.source().server(), sender(context)?, text);
    Ok(1)
}

fn toggle_ignore(chat: &Arc<Chat>, context: CommandContext<'_>) -> Result<i32, CommandError> {
    let from = sender(context)?;
    let name = word(context, "player")?;
    let Some(target) = online(context.source().server(), &name) else {
        return Err(config_error(chat, "pm.not_found", &[("target", &name)]));
    };
    chat.toggle_ignore(from, &target);
    Ok(1)
}

fn reload(chat: &Arc<Chat>, source: CommandSource<'_>) -> i32 {
    match chat.reload() {
        Ok(()) => {
            source.send_success(&render(chat.config().text("reload.done"), &[]), true);
            1
        }
        Err(error) => {
            log::error!("Could not reload the chat config: {error}");
            source.send_failure(render(
                chat.config().text("reload.failed"),
                &[("error", &error)],
            ));
            0
        }
    }
}

/// An online player, or one who has played here before: their UUID and nickname.
fn find(server: &Server, name: &str) -> Option<(Uuid, String)> {
    if let Some(player) = online(server, name) {
        return Some((
            player.gameprofile.id(),
            player.gameprofile.name().to_owned(),
        ));
    }
    server
        .known_players()
        .entries()
        .iter()
        .find(|known| known.last_known_name().eq_ignore_ascii_case(name))
        .map(|known| (known.uuid(), known.last_known_name().to_owned()))
}

fn tempmute(
    chat: &Arc<Chat>,
    context: CommandContext<'_>,
    reason: &str,
) -> Result<i32, CommandError> {
    let raw = word(context, "duration")?;
    let Some(seconds) = parse_duration(&raw) else {
        return Err(config_error(
            chat,
            "mute.bad_duration",
            &[("duration", &raw)],
        ));
    };
    mute(chat, context, Some(seconds), reason)
}

fn mute(
    chat: &Arc<Chat>,
    context: CommandContext<'_>,
    seconds: Option<u64>,
    reason: &str,
) -> Result<i32, CommandError> {
    let source = context.source();
    let server = source.server();
    let name = word(context, "player")?;
    let Some((target, target_name)) = find(server, &name) else {
        return Err(config_error(chat, "mute.not_found", &[("player", &name)]));
    };
    if source
        .player()
        .is_some_and(|actor| actor.gameprofile.id() == target)
    {
        return Err(config_error(chat, "mute.self", &[]));
    }

    let config = chat.config();
    let reason = match reason.trim() {
        "" => config.text("mute.default_reason"),
        given => given,
    };
    let now = now_millis();
    let until = seconds.map(|seconds| {
        now.saturating_add(i64::try_from(seconds).map_or(i64::MAX, |s| s.saturating_mul(1000)))
    });
    let mute = Mute {
        name: target_name,
        reason: reason.to_owned(),
        by: source.name(),
        since: now,
        until,
    };
    chat.mutes.put(target, mute.clone());

    let key = |permanent: &'static str, temporary: &'static str| {
        if until.is_some() {
            temporary
        } else {
            permanent
        }
    };
    source.send_success(
        &mute_text(&config, key("mute.done", "mute.done_temp"), &mute, now),
        true,
    );
    log::info!("{} muted {}: {}", mute.by, mute.name, mute.reason);

    let announce_key = key("mute.announce", "mute.announce_temp");
    let announce = (!config.text(announce_key).is_empty())
        .then(|| mute_text(&config, announce_key, &mute, now));
    for player in server.get_players() {
        if player.gameprofile.id() == target {
            let key = key("mute.player", "mute.player_temp");
            player.send_message(&mute_text(&config, key, &mute, now));
        } else if let Some(announce) = &announce {
            player.send_message(announce);
        }
    }
    Ok(1)
}

fn unmute(chat: &Arc<Chat>, context: CommandContext<'_>) -> Result<i32, CommandError> {
    let source = context.source();
    let server = source.server();
    let name = word(context, "player")?;
    let target = chat
        .mutes
        .find_by_name(&name)
        .or_else(|| find(server, &name).map(|(uuid, _)| uuid));
    let removed = target.and_then(|target| {
        chat.mutes
            .remove(target, now_millis())
            .map(|mute| (target, mute))
    });
    let Some((target, removed)) = removed else {
        return Err(config_error(chat, "unmute.not_muted", &[("player", &name)]));
    };

    let config = chat.config();
    let by = source.name();
    let values = [("player", removed.name.as_str()), ("by", by.as_str())];
    source.send_success(&render(config.text("unmute.done"), &values), true);
    log::info!("{by} unmuted {}", removed.name);
    if let Some(player) = server
        .get_players()
        .into_iter()
        .find(|player| player.gameprofile.id() == target)
    {
        player.send_message(&render(config.text("unmute.player"), &values));
    }
    Ok(1)
}

fn mute_list(chat: &Arc<Chat>, source: CommandSource<'_>) -> i32 {
    let config = chat.config();
    let now = now_millis();
    let mutes = chat.mutes.active(now);
    if mutes.is_empty() {
        source.send_success(&render(config.text("mutelist.empty"), &[]), false);
        return 0;
    }
    let count = mutes.len().to_string();
    source.send_success(
        &render(config.text("mutelist.header"), &[("count", &count)]),
        false,
    );
    for mute in &mutes {
        source.send_success(&mute_text(&config, "mutelist.line", mute, now), false);
    }
    i32::try_from(mutes.len()).unwrap_or(i32::MAX)
}
