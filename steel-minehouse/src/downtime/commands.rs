//! `/maintenance` and `/restart`. Console may run both; players need
//! `minehouse.command.maintenance` or `minehouse.command.restart`.

use std::sync::Arc;

use steel_core::command::{
    CommandArgument, CommandContext, CommandError, CommandNode, CommandRegistration,
    CommandRegistrationError, CommandRegistry, CommandSource, argument, literal,
};
use steel_utils::Identifier;
use text_components::TextComponent;

use super::{
    Downtime,
    time::{describe, parse_seconds},
};
use crate::moderation::format::render;

const NAMESPACE: &str = "minehouse";

type Handler = fn(&Arc<Downtime>, CommandContext<'_>) -> Result<i32, CommandError>;

pub(super) fn register(
    registry: &mut CommandRegistry,
    downtime: &Arc<Downtime>,
) -> Result<(), CommandRegistrationError> {
    let command = |name: &'static str, tree: fn(Arc<Downtime>) -> CommandNode| {
        let downtime = Arc::clone(downtime);
        CommandRegistration::new(Identifier::new(NAMESPACE, name), move || tree(downtime))
    };
    registry
        .register(command("maintenance", maintenance_tree))?
        .register(command("restart", restart_tree))?;
    Ok(())
}

fn run(
    downtime: &Arc<Downtime>,
    handler: Handler,
) -> impl for<'context> Fn(&CommandContext<'context>) -> Result<i32, CommandError> + Send + Sync + 'static
{
    let downtime = Arc::clone(downtime);
    move |context| handler(&downtime, *context)
}

fn reason() -> CommandNode {
    argument("reason", CommandArgument::greedy_string())
}

fn maintenance_tree(downtime: Arc<Downtime>) -> CommandNode {
    literal("maintenance")
        .executes(run(&downtime, |downtime, context| {
            Ok(maintenance_status(downtime, context))
        }))
        .then(
            literal("on")
                .executes(run(&downtime, |downtime, context| {
                    maintenance_on(downtime, context.source(), "")
                }))
                .then(reason().executes(run(&downtime, |downtime, context| {
                    maintenance_on(downtime, context.source(), context.string("reason")?)
                }))),
        )
        .then(literal("cancel").executes(run(&downtime, maintenance_cancel)))
        .then(literal("off").executes(run(&downtime, |downtime, context| {
            Ok(maintenance_off(downtime, context))
        })))
}

fn restart_tree(downtime: Arc<Downtime>) -> CommandNode {
    literal("restart")
        .executes(run(&downtime, |downtime, context| {
            Ok(restart_now(downtime, context))
        }))
        .then(literal("now").executes(run(&downtime, |downtime, context| {
            Ok(restart_now(downtime, context))
        })))
        .then(
            literal("status").executes(run(&downtime, |downtime, context| {
                Ok(restart_status(downtime, context))
            })),
        )
        .then(literal("cancel").executes(run(&downtime, restart_cancel)))
        .then(
            argument("time", CommandArgument::word())
                .executes(run(&downtime, |downtime, context| {
                    restart_in(downtime, context, "")
                }))
                .then(reason().executes(run(&downtime, |downtime, context| {
                    restart_in(downtime, context, context.string("reason")?)
                }))),
        )
}

/// A message with `&` colors; `values` go in as plain text.
fn say(source: CommandSource<'_>, format: &str, values: &[(&str, &str)], broadcast: bool) {
    source.send_success(&render(format, values), broadcast);
}

fn fail(text: impl Into<String>) -> CommandError {
    CommandError::from(text.into())
}

fn maintenance_status(downtime: &Arc<Downtime>, context: CommandContext<'_>) -> i32 {
    let source = context.source();
    let state = downtime.maintenance();
    if state.enabled {
        let format = if state.reason.is_empty() {
            "&cТехработы включены: заходят только админы."
        } else {
            "&cТехработы включены: заходят только админы. &7Причина: &f{reason}"
        };
        say(source, format, &[("reason", &state.reason)], false);
        return 1;
    }
    match downtime.countdown_info() {
        Some(countdown) if countdown.to_maintenance => say(
            source,
            "&eТехработы включатся через &f{time}&e. Отменить: &f/maintenance cancel",
            &[("time", &describe(countdown.seconds_left))],
            false,
        ),
        _ => say(
            source,
            "&aТехработы выключены, сервер открыт для всех.",
            &[],
            false,
        ),
    }
    0
}

/// `15m`, `1h30m`, `10с` as the first word of the reason starts a countdown. A bare number
/// stays part of the reason (`/maintenance on 2 часа обновления`).
fn countdown_seconds(first_word: &str) -> Option<u64> {
    if first_word.is_empty()
        || first_word
            .chars()
            .all(|character| character.is_ascii_digit())
    {
        return None;
    }
    parse_seconds(first_word).filter(|&seconds| seconds > 0)
}

fn maintenance_on(
    downtime: &Arc<Downtime>,
    source: CommandSource<'_>,
    reason: &str,
) -> Result<i32, CommandError> {
    let server = source.server();
    let text = reason.trim();
    let (first, rest) = text.split_once(' ').unwrap_or((text, ""));

    if let Some(seconds) = countdown_seconds(first) {
        if downtime.maintenance().enabled {
            return Err(fail("Техработы уже включены."));
        }
        let replaced = downtime.start_countdown(server, seconds, rest, true);
        say(
            source,
            "{replaced}&eТехработы включатся через &f{time}&e. Отменить: &f/maintenance cancel",
            &[
                ("replaced", replaced_note(replaced)),
                ("time", &describe(seconds)),
            ],
            true,
        );
        return Ok(1);
    }

    if downtime
        .countdown_info()
        .is_some_and(|countdown| countdown.to_maintenance)
    {
        downtime.cancel_countdown(server);
    }
    let (kicked, saved) = downtime.turn_on(server, text);
    say(
        source,
        "&cТехработы включены. &7Кикнуто игроков: &f{kicked}&7. Зайти могут только админы.",
        &[("kicked", &kicked.to_string())],
        true,
    );
    warn_if_not_saved(source, saved);
    Ok(1)
}

fn maintenance_cancel(
    downtime: &Arc<Downtime>,
    context: CommandContext<'_>,
) -> Result<i32, CommandError> {
    let source = context.source();
    let pending = downtime
        .countdown_info()
        .is_some_and(|countdown| countdown.to_maintenance);
    if !pending || downtime.cancel_countdown(source.server()).is_none() {
        return Err(fail("Таймера техработ нет."));
    }
    say(source, "&aТаймер техработ отменён.", &[], true);
    Ok(1)
}

fn maintenance_off(downtime: &Arc<Downtime>, context: CommandContext<'_>) -> i32 {
    let source = context.source();
    let server = source.server();
    if downtime
        .countdown_info()
        .is_some_and(|countdown| countdown.to_maintenance)
    {
        downtime.cancel_countdown(server);
    }
    let saved = downtime.set_maintenance(server, false, "");
    say(
        source,
        "&aТехработы выключены, сервер снова открыт для всех.",
        &[],
        true,
    );
    warn_if_not_saved(source, saved);
    1
}

fn warn_if_not_saved(source: CommandSource<'_>, saved: bool) {
    if !saved {
        source.send_failure(TextComponent::plain(
            "Не удалось сохранить режим на диск, после перезапуска он сбросится. Подробности в логе.",
        ));
    }
}

const fn replaced_note(replaced: bool) -> &'static str {
    if replaced {
        "Прошлый таймер заменён. "
    } else {
        ""
    }
}

fn restart_status(downtime: &Arc<Downtime>, context: CommandContext<'_>) -> i32 {
    let source = context.source();
    let Some(countdown) = downtime
        .countdown_info()
        .filter(|countdown| !countdown.to_maintenance)
    else {
        say(
            source,
            "&7Перезапуск не запланирован. &f/restart&7 перезапускает сразу, &f/restart <время> [причина]&7 с отсчётом, например &f/restart 5m&7.",
            &[],
            false,
        );
        return 0;
    };
    let format = if countdown.reason.is_empty() {
        "&eПерезапуск через &f{time}&e. &7Отменить: &f/restart cancel"
    } else {
        "&eПерезапуск через &f{time}&e. &7Причина: &f{reason} &7Отменить: &f/restart cancel"
    };
    say(
        source,
        format,
        &[
            ("time", &describe(countdown.seconds_left)),
            ("reason", &countdown.reason),
        ],
        false,
    );
    i32::try_from(countdown.seconds_left).unwrap_or(i32::MAX)
}

fn restart_in(
    downtime: &Arc<Downtime>,
    context: CommandContext<'_>,
    reason: &str,
) -> Result<i32, CommandError> {
    let source = context.source();
    let text = context.string("time")?;
    let Some(seconds) = parse_seconds(text).filter(|&seconds| seconds > 0) else {
        return Err(fail(format!(
            "Не понял время «{text}». Примеры: 30s, 5m, 1h30m, 90 (секунды). Максимум 24h."
        )));
    };
    let replaced = downtime.start_countdown(source.server(), seconds, reason, false);
    say(
        source,
        "{replaced}&eСервер перезапустится через &f{time}&e. Отменить: &f/restart cancel",
        &[
            ("replaced", replaced_note(replaced)),
            ("time", &describe(seconds)),
        ],
        true,
    );
    Ok(1)
}

/// `/restart` and `/restart now`: no countdown, kick everyone and stop at once.
fn restart_now(downtime: &Arc<Downtime>, context: CommandContext<'_>) -> i32 {
    let source = context.source();
    say(source, "&eПерезапускаю сервер.", &[], true);
    downtime.restart_now(source.server());
    1
}

fn restart_cancel(
    downtime: &Arc<Downtime>,
    context: CommandContext<'_>,
) -> Result<i32, CommandError> {
    let source = context.source();
    if downtime.cancel_countdown(source.server()).is_none() {
        return Err(fail("Перезапуск и так не запланирован."));
    }
    say(source, "&aПерезапуск отменён.", &[], true);
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_word_of_the_reason_may_be_a_time() {
        assert_eq!(countdown_seconds("15m"), Some(900));
        assert_eq!(countdown_seconds("10с"), Some(10));
        assert_eq!(countdown_seconds("2"), None);
        assert_eq!(countdown_seconds("обновление"), None);
        assert_eq!(countdown_seconds(""), None);
        assert_eq!(countdown_seconds("0m"), None);
    }
}
