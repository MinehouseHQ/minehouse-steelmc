//! `/co`: `inspect`, `lookup`, `near`, `page`, `rollback`, `restore`, `status`, `purge`,
//! `import`. For the console and
//! players with `minehouse.command.co`. Answers come from the log's own threads and finish on
//! the server tick.

use std::{sync::Arc, thread};

use steel_core::command::{
    CommandArgument, CommandContext, CommandError, CommandNode, CommandRegistration,
    CommandRegistrationError, CommandRegistry, CommandSource, DetachedCommandSource,
    SuspendedCommand, SuspendedCommandPoll, argument, literal,
};
use steel_utils::Identifier;
use text_components::TextComponent;
use tokio::sync::oneshot::{self, error::TryRecvError};
use uuid::Uuid;

use super::{
    BlockLog, Lookup,
    format::ago,
    import as history, now_ms, params,
    rollback::{MAX_ROWS, Run},
    store::{Query, Row},
};
use crate::{
    config,
    moderation::format::{parse_duration, render},
};

/// Purging less than this would be a mistake more often than a wish.
const MIN_PURGE_S: u64 = 30 * 86400;

/// A command waiting for the log's threads.
struct Pending {
    source: DetachedCommandSource,
    answer: oneshot::Receiver<Vec<TextComponent>>,
}

impl SuspendedCommand for Pending {
    fn poll(&mut self) -> SuspendedCommandPoll {
        match self.answer.try_recv() {
            Ok(lines) => {
                let source = self.source.source();
                for line in &lines {
                    source.send_success(line, false);
                }
                SuspendedCommandPoll::Ready(Ok(1))
            }
            Err(TryRecvError::Empty) => SuspendedCommandPoll::Pending,
            Err(TryRecvError::Closed) => SuspendedCommandPoll::Ready(Err(CommandError::from(
                "Журнал не ответил, попробуй ещё раз.",
            ))),
        }
    }

    fn cancel(&mut self) {}
}

/// A rollback or a restore: first waiting for its rows, then applying them tick by tick.
struct Rewind {
    source: DetachedCommandSource,
    log: Arc<BlockLog>,
    undo: bool,
    rows: Option<oneshot::Receiver<Result<Vec<Row>, String>>>,
    run: Option<Run>,
    capped: bool,
}

impl SuspendedCommand for Rewind {
    fn poll(&mut self) -> SuspendedCommandPoll {
        if let Some(rows) = &mut self.rows {
            match rows.try_recv() {
                Ok(Ok(rows)) => {
                    self.capped = rows.len() >= MAX_ROWS as usize;
                    self.run = Some(Run::new(rows, self.undo));
                    self.rows = None;
                }
                Ok(Err(error)) => {
                    return SuspendedCommandPoll::Ready(Err(CommandError::from(format!(
                        "Журнал не ответил: {error}"
                    ))));
                }
                Err(TryRecvError::Empty) => return SuspendedCommandPoll::Pending,
                Err(TryRecvError::Closed) => {
                    return SuspendedCommandPoll::Ready(Err(CommandError::from(
                        "Журнал не ответил, попробуй ещё раз.",
                    )));
                }
            }
        }
        let source = self.source.source();
        let Some(run) = &mut self.run else {
            return SuspendedCommandPoll::Ready(Ok(0));
        };
        if !run.step(source.server()) {
            return SuspendedCommandPoll::Pending;
        }
        self.finish();
        SuspendedCommandPoll::Ready(Ok(1))
    }

    /// Whatever was already applied is marked, so the log never disagrees with the world.
    fn cancel(&mut self) {
        if let Some(run) = self.run.take() {
            let (_, blocks, containers) = run.finish();
            self.log.store.mark(blocks, containers, self.undo);
        }
    }
}

impl Rewind {
    fn finish(&mut self) {
        let Some(run) = self.run.take() else {
            return;
        };
        let (outcome, blocks, containers) = run.finish();
        self.log.store.mark(blocks, containers, self.undo);
        let source = self.source.source();
        let title = if self.undo {
            "Откат"
        } else {
            "Восстановление"
        };
        source.send_success(
            &render(
                "&3{title} &8· &7блоков: &f{blocks}&7, записей о предметах: &f{items}",
                &[
                    ("title", title),
                    ("blocks", &outcome.blocks.to_string()),
                    ("items", &outcome.items.to_string()),
                ],
            ),
            true,
        );
        if outcome.unloaded > 0 {
            source.send_success(
                &render(
                    "&7Пропущено в незагруженных чанках: &f{rows}&7. Подойди туда и повтори команду.",
                    &[("rows", &outcome.unloaded.to_string())],
                ),
                false,
            );
        }
        if outcome.failed > 0 {
            source.send_success(
                &render(
                    "&7Не получилось: &f{rows} &7(нет сундука, нет места или предметов, неизвестный блок).",
                    &[("rows", &outcome.failed.to_string())],
                ),
                false,
            );
        }
        if self.capped {
            source.send_success(
                &render(
                    "&7Это не всё: за один раз берётся &f{rows} &7записей. Повтори команду.",
                    &[("rows", &MAX_ROWS.to_string())],
                ),
                false,
            );
        }
    }
}

pub(super) fn register(
    registry: &mut CommandRegistry,
    log: &Arc<BlockLog>,
) -> Result<(), CommandRegistrationError> {
    let log = Arc::clone(log);
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "co"), move || tree(&log))
            .alias("blocklog"),
    )?;
    Ok(())
}

fn tree(log: &Arc<BlockLog>) -> CommandNode {
    let inspect_node = |name: &'static str| {
        let log = Arc::clone(log);
        literal(name).executes(move |context| self::inspect(&log, *context))
    };
    let lookup_node = |name: &'static str| {
        let log = Arc::clone(log);
        literal(name).then(
            argument("params", CommandArgument::greedy_string()).executes_suspended(
                move |context| {
                    let words = context.string("params")?;
                    self::lookup(&log, *context, words)
                },
            ),
        )
    };
    let page_node = |name: &'static str| {
        let log = Arc::clone(log);
        literal(name).then(
            argument("page", CommandArgument::integer(1, 1_000_000))
                .executes_suspended(move |context| self::page(&log, *context)),
        )
    };
    let rewind_node = |name: &'static str, undo: bool| {
        let log = Arc::clone(log);
        literal(name).then(
            argument("params", CommandArgument::greedy_string()).executes_suspended(
                move |context| {
                    let words = context.string("params")?;
                    self::rewind(&log, *context, words, undo)
                },
            ),
        )
    };
    let near = Arc::clone(log);
    let status = Arc::clone(log);
    let purge = Arc::clone(log);
    let importing = Arc::clone(log);
    literal("co")
        .then(inspect_node("inspect"))
        .then(inspect_node("i"))
        .then(lookup_node("lookup"))
        .then(lookup_node("l"))
        .then(page_node("page"))
        .then(page_node("p"))
        .then(rewind_node("rollback", true))
        .then(rewind_node("rb", true))
        .then(rewind_node("restore", false))
        .then(rewind_node("rs", false))
        .then(literal("near").executes_suspended(move |context| {
            let words = format!("r:{}", near.config.near_radius);
            lookup(&near, *context, &words)
        }))
        .then(
            literal("status")
                .executes_suspended(move |context| Ok(self::status(&status, *context))),
        )
        .then(
            literal("purge").then(
                argument("older", CommandArgument::word())
                    .executes_suspended(move |context| self::purge(&purge, *context)),
            ),
        )
        .then(literal("import").executes(move |context| self::import(&importing, *context)))
}

/// The session of the asker: the account of a player, one shared for the consoles.
fn asker(source: CommandSource<'_>) -> Uuid {
    source
        .player()
        .map_or_else(Uuid::nil, |player| player.gameprofile.id())
}

fn pending(source: CommandSource<'_>) -> (oneshot::Sender<Vec<TextComponent>>, Pending) {
    let (sender, answer) = oneshot::channel();
    (
        sender,
        Pending {
            source: source.detach(),
            answer,
        },
    )
}

fn inspect(log: &Arc<BlockLog>, context: CommandContext<'_>) -> Result<i32, CommandError> {
    let source = context.source();
    let Some(player) = source.player() else {
        return Err(CommandError::from("Инспектор только для игрока."));
    };
    let message = if log.toggle_inspector(player.gameprofile.id()) {
        "&3Инспектор включён. &7Ударь блок — его история; ПКМ — ещё и что клали и забирали."
    } else {
        "&3Инспектор выключен."
    };
    source.send_success(&render(message, &[]), false);
    Ok(1)
}

/// The query of `words`, asked from where `source` stands.
fn query(log: &BlockLog, source: CommandSource<'_>, words: &str) -> Result<Query, CommandError> {
    let origin = source.player().map(|_| {
        let position = source.position();
        params::Origin {
            world: source.world().key.to_string(),
            #[expect(
                clippy::cast_possible_truncation,
                reason = "world coordinates fit a block position"
            )]
            pos: (
                position.x.floor() as i32,
                position.y.floor() as i32,
                position.z.floor() as i32,
            ),
        }
    });
    params::parse(words, origin.as_ref(), log.config.max_radius, now_ms())
        .map_err(CommandError::from)
}

fn lookup(
    log: &Arc<BlockLog>,
    context: CommandContext<'_>,
    words: &str,
) -> Result<Pending, CommandError> {
    let source = context.source();
    let query = query(log, source, words)?;
    let (sender, pending) = pending(source);
    log.ask(
        asker(source),
        Lookup {
            title: words.to_owned(),
            query,
            with_place: true,
        },
        0,
        move |lines| {
            let _ = sender.send(lines);
        },
    );
    Ok(pending)
}

/// Starts a rollback (`undo`) or a restore. A time is a must, and so is a player or a
/// radius: nobody means to rewind the whole server.
fn rewind(
    log: &Arc<BlockLog>,
    context: CommandContext<'_>,
    words: &str,
    undo: bool,
) -> Result<Rewind, CommandError> {
    let source = context.source();
    let mut query = query(log, source, words)?;
    if query.since.is_none() {
        return Err(CommandError::from(
            "Укажи срок: t:2h откатит всё за последние два часа.",
        ));
    }
    if query.actors.is_empty() && query.around.is_none() {
        return Err(CommandError::from(
            "Укажи игрока (u:Ник) или радиус (r:20), иначе затронет весь сервер.",
        ));
    }
    // A rollback takes what is still in force, a restore what was rolled back.
    query.rolled_back = Some(!undo);
    query.limit = MAX_ROWS;
    let (sender, rows) = oneshot::channel();
    // Newest first to undo, oldest first to redo: the same block may have changed twice.
    log.store.changes(query, !undo, move |answer| {
        let _ = sender.send(answer);
    });
    Ok(Rewind {
        source: source.detach(),
        log: Arc::clone(log),
        undo,
        rows: Some(rows),
        run: None,
        capped: false,
    })
}

fn page(log: &Arc<BlockLog>, context: CommandContext<'_>) -> Result<Pending, CommandError> {
    let source = context.source();
    let asker = asker(source);
    let last = log
        .sessions
        .lock()
        .get(&asker)
        .and_then(|session| session.last.clone())
        .ok_or_else(|| CommandError::from("Сначала поиск: /co lookup или инспектор."))?;
    let page = u32::try_from(context.integer("page")?).unwrap_or(1).max(1);
    let (sender, pending) = pending(source);
    log.ask(
        asker,
        last,
        (page - 1).saturating_mul(log.config.page_size.max(1)),
        move |lines| {
            let _ = sender.send(lines);
        },
    );
    Ok(pending)
}

fn status(log: &Arc<BlockLog>, context: CommandContext<'_>) -> Pending {
    let (sender, pending) = pending(context.source());
    let keep_days = log.config.keep_days;
    log.store.stats(move |answer| {
        let _ = sender.send(match answer {
            Ok(stats) => vec![render(
                "&3Журнал &8· &7блоков: &f{blocks}&7, сундуков: &f{containers}&7, самая старая запись: &f{oldest}&7, хранится: &f{keep}",
                &[
                    ("blocks", &stats.blocks.to_string()),
                    ("containers", &stats.containers.to_string()),
                    (
                        "oldest",
                        &stats
                            .oldest
                            .map_or_else(|| "нет".to_owned(), |oldest| ago(now_ms(), oldest)),
                    ),
                    (
                        "keep",
                        &if keep_days == 0 {
                            "всё".to_owned()
                        } else {
                            format!("{keep_days} д")
                        },
                    ),
                ],
            )],
            Err(error) => vec![TextComponent::plain(format!("Журнал не ответил: {error}"))],
        });
    });
    pending
}

fn purge(log: &Arc<BlockLog>, context: CommandContext<'_>) -> Result<Pending, CommandError> {
    let older = context.string("older")?;
    let seconds = parse_duration(older)
        .ok_or_else(|| CommandError::from("Укажи срок, например 90d: удалится всё, что старше."))?;
    if seconds < MIN_PURGE_S {
        return Err(CommandError::from(
            "Удалять можно только то, что старше 30 дней.",
        ));
    }
    let before = now_ms() - i64::try_from(seconds).unwrap_or(i64::MAX / 1000) * 1000;
    let (sender, pending) = pending(context.source());
    log.store.purge(before, move |answer| {
        let _ = sender.send(vec![match answer {
            Ok(rows) => render(
                "&3Журнал &8· &7удалено записей: &f{rows}",
                &[("rows", &rows.to_string())],
            ),
            Err(error) => TextComponent::plain(format!("Не удалось: {error}")),
        }]);
    });
    Ok(pending)
}

/// Starts the import on its own thread: a big history takes minutes.
fn import(log: &Arc<BlockLog>, context: CommandContext<'_>) -> Result<i32, CommandError> {
    let source = context.source();
    let folder = config::data_path("coreprotect-export");
    if !folder.is_dir() {
        return Err(CommandError::from(format!(
            "Нет папки {}. Положи туда blocks.csv и containers.csv.",
            folder.display()
        )));
    }
    if !log.claim_import() {
        return Err(CommandError::from("Импорт уже идёт."));
    }
    let player = source.player().cloned();
    let log_for_thread = Arc::clone(log);
    let started = thread::Builder::new()
        .name("minehouse-blocklog-import".to_owned())
        .spawn(move || {
            let log = log_for_thread;
            let result = history::run(&log.store, &folder, &log.config.import_worlds);
            log.release_import();
            let message = match result {
                Ok(report) => format!(
                    "Импорт журнала готов: блоков {}, сундуков {}, пропущено строк {}.",
                    report.blocks, report.containers, report.skipped
                ),
                Err(error) => format!("Импорт журнала не удался: {error}"),
            };
            log::info!("{message}");
            if let Some(player) = player {
                player.send_message(&TextComponent::plain(message));
            }
        });
    if let Err(error) = started {
        log.release_import();
        return Err(CommandError::from(format!(
            "Не удалось начать импорт: {error}"
        )));
    }
    source.send_success(
        &render(
            "&3Импорт журнала начат. &7Итог придёт сюда и в лог сервера.",
            &[],
        ),
        true,
    );
    Ok(1)
}
