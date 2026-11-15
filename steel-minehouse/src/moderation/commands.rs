//! The command trees. Every command calls the site from the Tokio runtime and finishes on the
//! server tick, so a slow site never holds the tick.

use std::{future::Future, sync::Arc};

use steel_core::command::{
    CommandArgument, CommandArgumentParser, CommandContext, CommandError, CommandNode,
    CommandParserSource, CommandReader, CommandRegistration, CommandRegistrationError,
    CommandRegistry, CommandSource, CommandSuggestionContext, CommandSuggestions,
    DetachedCommandSource, SuspendedCommand, SuspendedCommandPoll, argument, literal,
};
use steel_protocol::packets::game::{ArgumentStringTypeBehavior, ArgumentType, SuggestionType};
use steel_utils::{DowncastType, DowncastTypeKey, Identifier};
use text_components::{Modifier, TextComponent, format::Color};
use tokio::{
    sync::oneshot::{self, error::TryRecvError},
    task::AbortHandle,
};
use uuid::Uuid;

use super::{
    Moderation, announce_ban,
    api::{Actor, Unban, Unwarn},
    broadcast,
    format::{duration_between, format_time, parse_duration, render},
    settings::BansConfig,
    wipe::{self, WipeReport},
};
use crate::site::SiteError;

const NAMESPACE: &str = "minehouse";
const WIPE_FLAG: &str = "--wipe";
const DURATION_HINTS: [&str; 6] = ["1h", "12h", "1d", "3d", "7d", "30d"];

/// The end of a command, run on the server tick once the site answered.
type Completion = Box<dyn for<'a> FnOnce(CommandSource<'a>) -> Result<i32, CommandError> + Send>;

/// Pins the closure's signature for inference.
fn completion(
    finish: impl for<'a> FnOnce(CommandSource<'a>) -> Result<i32, CommandError> + Send + 'static,
) -> Completion {
    Box::new(finish)
}

/// A command waiting for the site.
struct SiteCall {
    source: DetachedCommandSource,
    answer: oneshot::Receiver<Completion>,
    task: AbortHandle,
}

impl SuspendedCommand for SiteCall {
    fn poll(&mut self) -> SuspendedCommandPoll {
        match self.answer.try_recv() {
            Ok(finish) => SuspendedCommandPoll::Ready(finish(self.source.source())),
            Err(TryRecvError::Empty) => SuspendedCommandPoll::Pending,
            Err(TryRecvError::Closed) => SuspendedCommandPoll::Ready(Err(CommandError::from(
                "Команда прервана, попробуй ещё раз.",
            ))),
        }
    }

    fn cancel(&mut self) {
        self.task.abort();
    }
}

impl Moderation {
    /// Runs `work` on the Tokio runtime; what it returns finishes the command on the tick.
    fn call(
        &self,
        source: CommandSource<'_>,
        work: impl Future<Output = Result<Completion, SiteError>> + Send + 'static,
    ) -> SiteCall {
        let (sender, answer) = oneshot::channel();
        let task = self.runtime.spawn(async move {
            let finish = work.await.unwrap_or_else(|error| {
                log::warn!("Moderation API call failed: {error}");
                completion(move |_| {
                    Err(CommandError::from(format!(
                        "Сайт не ответил, попробуй ещё раз. ({error})"
                    )))
                })
            });
            // The command was cancelled: nobody waits for the answer.
            let _ = sender.send(finish);
        });
        SiteCall {
            source: source.detach(),
            answer,
            task: task.abort_handle(),
        }
    }
}

/// What a single-word argument completes to.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WordParser {
    /// A nickname: online players are offered.
    Player,
    /// A ban length.
    Duration,
}

// SAFETY: This key is used for `WordParser` alone.
unsafe impl DowncastType for WordParser {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("minehouse:command/parser/word");
}

/// The value of a [`WordParser`] argument.
#[derive(Debug)]
pub(crate) struct Word(String);

// SAFETY: This key is used for `Word` alone.
unsafe impl DowncastType for Word {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("minehouse:command/value/word");
}

impl CommandArgumentParser for WordParser {
    type Value = Word;

    fn parse(
        &self,
        reader: &mut CommandReader<'_, '_>,
        _source: CommandParserSource<'_>,
    ) -> Result<Self::Value, CommandError> {
        let word = reader.read_unquoted_string();
        if word.is_empty() {
            return Err(reader.error(match self {
                Self::Player => "Укажи ник.",
                Self::Duration => "Укажи срок, например 7d.",
            }));
        }
        Ok(Word(word.to_owned()))
    }

    fn list_suggestions(
        &self,
        context: CommandSuggestionContext<'_>,
        suggestions: &mut CommandSuggestions<'_, '_>,
    ) {
        let typed = suggestions.remaining().to_lowercase();
        let mut offer = |value: &str| {
            if value.to_lowercase().starts_with(&typed) {
                suggestions.suggest(value);
            }
        };
        match self {
            Self::Player => {
                for name in context.source().player_names() {
                    offer(&name);
                }
            }
            Self::Duration => DURATION_HINTS.into_iter().for_each(offer),
        }
    }

    fn protocol_argument(&self) -> (ArgumentType, Option<SuggestionType>) {
        (
            ArgumentType::String {
                behavior: ArgumentStringTypeBehavior::SingleWord,
            },
            Some(SuggestionType::AskServer),
        )
    }
}

pub(crate) fn player() -> CommandNode {
    argument("player", CommandArgument::custom(WordParser::Player))
}

fn reason() -> CommandNode {
    argument("reason", CommandArgument::greedy_string())
}

pub(crate) fn word(context: CommandContext<'_>, name: &str) -> Result<String, CommandError> {
    context.value::<Word>(name).map(|word| word.0.clone())
}

fn reason_of(context: CommandContext<'_>, given: bool) -> Result<Option<String>, CommandError> {
    if given {
        context
            .string("reason")
            .map(|reason| Some(reason.trim().to_owned()))
    } else {
        Ok(None)
    }
}

fn actor(source: CommandSource<'_>) -> Actor {
    Actor {
        uuid: source.player().map(|player| player.gameprofile.id()),
        name: source.name(),
    }
}

fn is_self(source: CommandSource<'_>, target: &str) -> bool {
    source
        .player()
        .is_some_and(|player| player.gameprofile.name().eq_ignore_ascii_case(target))
}

fn success(source: CommandSource<'_>, text: String, color: Option<Color>, broadcast: bool) {
    let message = TextComponent::plain(text);
    let message = match color {
        Some(color) => message.color(color),
        None => message,
    };
    source.send_success(&message, broadcast);
}

/// How a `/ban`-like command was typed.
#[derive(Clone, Copy)]
struct Options {
    /// `/tempban`: there is a `duration` argument.
    temporary: bool,
    /// There is a `reason` argument.
    with_reason: bool,
    /// `--wipe`: the account also starts over as a new player, see [`wipe`].
    wipe: bool,
}

type Handler = fn(&Arc<Moderation>, CommandContext<'_>, Options) -> Result<SiteCall, CommandError>;

/// `<node> [--wipe] [причина]`.
fn with_reason_and_wipe(
    node: CommandNode,
    moderation: &Arc<Moderation>,
    handler: Handler,
    temporary: bool,
) -> CommandNode {
    let run = |with_reason: bool, wipe: bool| {
        let moderation = Arc::clone(moderation);
        let options = Options {
            temporary,
            with_reason,
            wipe,
        };
        move |context: &CommandContext<'_>| handler(&moderation, *context, options)
    };
    node.executes_suspended(run(false, false))
        .then(
            literal(WIPE_FLAG)
                .executes_suspended(run(false, true))
                .then(reason().executes_suspended(run(true, true))),
        )
        .then(reason().executes_suspended(run(true, false)))
}

/// Resets the account when the command had `--wipe`. Runs off the tick.
async fn wipe_if_asked(
    moderation: &Moderation,
    asked: bool,
    uuids: &[Uuid],
    kick_message: Option<&str>,
) -> Option<WipeReport> {
    if !asked || uuids.is_empty() {
        return None;
    }
    let server = moderation.server()?;
    // A ban kicks with its own text; the reset then finds nobody online.
    if let Some(message) = kick_message {
        moderation.kick(&server, uuids, Some(message));
    }
    Some(wipe::wipe_players(&server, uuids, wipe::WIPED_MESSAGE).await)
}

/// Tells the moderator what `--wipe` did.
fn report_wipe(
    source: CommandSource<'_>,
    asked: bool,
    nickname: &str,
    nicknames: usize,
    report: Option<&WipeReport>,
) {
    if !asked {
        return;
    }
    let Some(report) = report else {
        source.send_failure(TextComponent::plain(format!(
            "Сайт не вернул UUID ников {nickname}, прогресс не сброшен."
        )));
        return;
    };
    for error in &report.errors {
        source.send_failure(TextComponent::plain(format!("Не удалось сбросить {error}")));
    }
    if report.wiped == 0 {
        if report.errors.is_empty() {
            success(
                source,
                format!("У {nickname} нет сохранённого прогресса, сбрасывать нечего."),
                Some(Color::Green),
                true,
            );
        }
        return;
    }
    let count = if nicknames > 1 {
        format!(" (ников: {})", report.wiped)
    } else {
        String::new()
    };
    success(
        source,
        format!(
            "Прогресс {nickname} сброшен начисто, как у нового игрока: позиция, точка \
             возрождения, инвентарь, эндер-сундук, опыт, эффекты, статистика и достижения{count}."
        ),
        Some(Color::Green),
        true,
    );
}

/// Registers every command of the module.
pub(super) fn register(
    registry: &mut CommandRegistry,
    moderation: &Arc<Moderation>,
) -> Result<(), CommandRegistrationError> {
    let command = |name: &'static str, tree: fn(Arc<Moderation>) -> CommandNode| {
        let moderation = Arc::clone(moderation);
        CommandRegistration::new(Identifier::new(NAMESPACE, name), move || tree(moderation))
    };
    registry
        .register(command("ban", ban_tree))?
        .register(command("tempban", tempban_tree))?
        .register(command("unban", unban_tree))?
        .register(command("banlist", banlist_tree))?
        .register(command("warn", warn_tree))?
        .register(command("unwarn", unwarn_tree))?
        .register(command("warns", warns_tree))?
        .register(command("whitelist", whitelist_tree))?;
    Ok(())
}

fn ban_tree(moderation: Arc<Moderation>) -> CommandNode {
    literal("ban").then(with_reason_and_wipe(player(), &moderation, ban, false))
}

fn tempban_tree(moderation: Arc<Moderation>) -> CommandNode {
    let duration = argument("duration", CommandArgument::custom(WordParser::Duration));
    literal("tempban").then(player().then(with_reason_and_wipe(duration, &moderation, ban, true)))
}

fn unban_tree(moderation: Arc<Moderation>) -> CommandNode {
    literal("unban").then(with_reason_and_wipe(player(), &moderation, unban, false))
}

fn banlist_tree(moderation: Arc<Moderation>) -> CommandNode {
    literal("banlist").executes_suspended(move |context| Ok(banlist(&moderation, *context)))
}

fn warn_tree(moderation: Arc<Moderation>) -> CommandNode {
    literal("warn").then(with_reason_and_wipe(player(), &moderation, warn, false))
}

fn unwarn_tree(moderation: Arc<Moderation>) -> CommandNode {
    literal("unwarn")
        .then(player().executes_suspended(move |context| unwarn(&moderation, *context)))
}

fn warns_tree(moderation: Arc<Moderation>) -> CommandNode {
    literal("warns").then(player().executes_suspended(move |context| warns(&moderation, *context)))
}

fn whitelist_tree(moderation: Arc<Moderation>) -> CommandNode {
    let adding = Arc::clone(&moderation);
    let target = || argument("target", CommandArgument::custom(WordParser::Player));
    literal("whitelist")
        .then(
            literal("add").then(
                target().executes_suspended(move |context| whitelist(&adding, *context, true)),
            ),
        )
        .then(literal("remove").then(
            target().executes_suspended(move |context| whitelist(&moderation, *context, false)),
        ))
}

fn ban(
    moderation: &Arc<Moderation>,
    context: CommandContext<'_>,
    options: Options,
) -> Result<SiteCall, CommandError> {
    let source = context.source();
    let target = word(context, "player")?;
    let seconds = if options.temporary {
        let raw = word(context, "duration")?;
        Some(parse_duration(&raw).ok_or_else(|| {
            CommandError::from(format!(
                "Не понял срок «{raw}». Примеры: 30m, 12h, 7d, 2w, 1mo, 1d12h."
            ))
        })?)
    } else {
        None
    };
    let reason = reason_of(context, options.with_reason)?;
    if is_self(source, &target) {
        return Err(CommandError::from("Себя забанить нельзя."));
    }

    let by = actor(source);
    let moderation_task = Arc::clone(moderation);
    Ok(moderation.call(source, async move {
        let moderation = moderation_task;
        let config = BansConfig::load();
        let reason = reason.unwrap_or_else(|| config.default_reason.clone());
        let result = moderation.site.ban(&target, &reason, seconds, &by).await?;
        let wiped = match &result {
            Some(result) => {
                let message = result
                    .message
                    .as_deref()
                    .unwrap_or(&moderation.banned_message);
                wipe_if_asked(&moderation, options.wipe, &result.uuids, Some(message)).await
            }
            None => None,
        };

        Ok(completion(move |source| {
            let Some(result) = result else {
                return Err(CommandError::from(format!(
                    "Игрок {target} не найден: ник не привязан к Discord."
                )));
            };
            let until = result.expires_at.as_deref().map_or_else(
                || "навсегда".to_owned(),
                |time| format!("до {} МСК", format_time(time)),
            );
            success(
                source,
                format!("{} заблокирован {until}: {reason}", result.nickname),
                Some(Color::Red),
                true,
            );

            let server = source.server();
            announce_ban(
                server,
                &config,
                &result.nickname,
                Some(&reason),
                seconds,
                result.expires_at.as_deref(),
                Some(&by.name),
            );
            moderation.kick(server, &result.uuids, result.message.as_deref());
            report_wipe(
                source,
                options.wipe,
                &result.nickname,
                result.uuids.len(),
                wiped.as_ref(),
            );
            Ok(1)
        }))
    }))
}

fn unban(
    moderation: &Arc<Moderation>,
    context: CommandContext<'_>,
    options: Options,
) -> Result<SiteCall, CommandError> {
    let source = context.source();
    let target = word(context, "player")?;
    let reason = reason_of(context, options.with_reason)?.unwrap_or_default();
    let by = actor(source);
    let moderation_task = Arc::clone(moderation);

    Ok(moderation.call(source, async move {
        let moderation = moderation_task;
        let result = moderation.site.unban(&target, &reason, &by).await?;
        let wiped = match &result {
            Unban::Done(_, uuids) => wipe_if_asked(&moderation, options.wipe, uuids, None).await,
            _ => None,
        };
        Ok(completion(move |source| match result {
            Unban::Done(nickname, uuids) => {
                success(
                    source,
                    format!("{nickname} разблокирован."),
                    Some(Color::Green),
                    true,
                );
                report_wipe(source, options.wipe, &nickname, uuids.len(), wiped.as_ref());
                Ok(1)
            }
            Unban::NotBanned(nickname) => Err(CommandError::from(format!(
                "{nickname} не заблокирован.{}",
                if options.wipe {
                    " Прогресс не сброшен."
                } else {
                    ""
                }
            ))),
            Unban::NotFound => Err(CommandError::from(format!("Игрок {target} не найден."))),
        }))
    }))
}

fn banlist(moderation: &Arc<Moderation>, context: CommandContext<'_>) -> SiteCall {
    let site = moderation.site.clone();
    moderation.call(context.source(), async move {
        let bans = site.bans().await?;
        Ok(completion(move |source| {
            if bans.is_empty() {
                success(
                    source,
                    "Сейчас никто не заблокирован.".to_owned(),
                    None,
                    false,
                );
                return Ok(0);
            }

            success(
                source,
                format!("Заблокированы ({}):", bans.len()),
                Some(Color::Gold),
                false,
            );
            for ban in &bans {
                let until = ban.expires_at.as_deref().map_or_else(
                    || "навсегда".to_owned(),
                    |time| format!("до {}", format_time(time)),
                );
                let by = ban.by.as_deref().map_or_default(|by| format!(", {by}"));
                let reason = ban.reason.as_deref().unwrap_or("");
                let line = TextComponent::plain(ban.nickname.clone())
                    .color(Color::Red)
                    .add_child(
                        TextComponent::plain(format!(" — {reason} ({until}{by})"))
                            .color(Color::Gray),
                    );
                source.send_success(&line, false);
            }
            Ok(i32::try_from(bans.len()).unwrap_or(i32::MAX))
        }))
    })
}

/// How long the ban of the last warn is; `None` = forever.
fn warn_ban_seconds(config: &BansConfig) -> Option<u64> {
    let duration = config.warn_ban_duration.trim();
    if duration.is_empty() {
        return None;
    }
    let seconds = parse_duration(duration);
    if seconds.is_none() {
        log::warn!(
            "warn_ban_duration «{duration}» is not a duration, the warn ban will be permanent"
        );
    }
    seconds
}

fn warn(
    moderation: &Arc<Moderation>,
    context: CommandContext<'_>,
    options: Options,
) -> Result<SiteCall, CommandError> {
    let source = context.source();
    let target = word(context, "player")?;
    let reason = reason_of(context, options.with_reason)?;
    if is_self(source, &target) {
        return Err(CommandError::from("Себе предупреждение выдать нельзя."));
    }

    let by = actor(source);
    let moderation_task = Arc::clone(moderation);
    Ok(moderation.call(source, async move {
        let moderation = moderation_task;
        let config = BansConfig::load();
        let reason = reason.unwrap_or_else(|| config.default_reason.clone());
        let limit = config.warn_limit;
        let ban_seconds = warn_ban_seconds(&config);
        let ban_reason = Some(config.warn_ban_reason.trim()).filter(|reason| !reason.is_empty());
        let result = moderation
            .site
            .warn(&target, &reason, &by, limit, ban_seconds, ban_reason)
            .await?;
        let wiped = match &result {
            Some(result) => {
                // The third warn bans: then the ban text is what the player reads.
                let message = result.banned.then(|| {
                    result
                        .message
                        .as_deref()
                        .unwrap_or(&moderation.banned_message)
                });
                wipe_if_asked(&moderation, options.wipe, &result.uuids, message).await
            }
            None => None,
        };

        Ok(completion(move |source| {
            let Some(result) = result else {
                return Err(CommandError::from(format!(
                    "Игрок {target} не найден: ник не привязан к Discord."
                )));
            };
            let count = if result.banned {
                result.banned_at
            } else {
                result.count
            };
            let (count_text, limit_text) = (count.to_string(), limit.to_string());
            let values = [
                ("player", result.nickname.as_str()),
                ("reason", reason.as_str()),
                ("by", by.name.as_str()),
                ("count", count_text.as_str()),
                ("limit", limit_text.as_str()),
            ];
            success(
                source,
                format!(
                    "{} получил предупреждение ({count}/{limit}): {reason}",
                    result.nickname
                ),
                Some(Color::Gold),
                true,
            );

            let server = source.server();
            if config.enabled {
                broadcast(server, &config.warn, &values);
            }

            if result.banned {
                let now = chrono::Utc::now().to_rfc3339();
                let seconds = result
                    .expires_at
                    .as_deref()
                    .and_then(|expires| duration_between(&now, expires));
                announce_ban(
                    server,
                    &config,
                    &result.nickname,
                    result.ban_reason.as_deref(),
                    seconds,
                    result.expires_at.as_deref(),
                    Some(&by.name),
                );
                moderation.kick(server, &result.uuids, result.message.as_deref());
            } else if !config.warn_player.trim().is_empty()
                && let Some(online) = server.get_players().into_iter().find(|player| {
                    player
                        .gameprofile
                        .name()
                        .eq_ignore_ascii_case(&result.nickname)
                })
            {
                online.send_message(&render(config.warn_player.trim(), &values));
            }
            report_wipe(
                source,
                options.wipe,
                &result.nickname,
                result.uuids.len(),
                wiped.as_ref(),
            );
            Ok(1)
        }))
    }))
}

fn unwarn(
    moderation: &Arc<Moderation>,
    context: CommandContext<'_>,
) -> Result<SiteCall, CommandError> {
    let source = context.source();
    let target = word(context, "player")?;
    let by = actor(source);
    let site = moderation.site.clone();

    Ok(moderation.call(source, async move {
        let config = BansConfig::load();
        let result = site.unwarn(&target, &by).await?;
        Ok(completion(move |source| match result {
            Unwarn::Done(nickname, count) => {
                let limit = config.warn_limit;
                success(
                    source,
                    format!("С {nickname} снято предупреждение, осталось {count}/{limit}."),
                    Some(Color::Green),
                    true,
                );
                if config.enabled {
                    broadcast(
                        source.server(),
                        &config.unwarn,
                        &[
                            ("player", nickname.as_str()),
                            ("by", by.name.as_str()),
                            ("count", count.to_string().as_str()),
                            ("limit", limit.to_string().as_str()),
                        ],
                    );
                }
                Ok(1)
            }
            Unwarn::NotWarned(nickname) => Err(CommandError::from(format!(
                "У {nickname} нет предупреждений."
            ))),
            Unwarn::NotFound => Err(CommandError::from(format!("Игрок {target} не найден."))),
        }))
    }))
}

fn warns(
    moderation: &Arc<Moderation>,
    context: CommandContext<'_>,
) -> Result<SiteCall, CommandError> {
    let source = context.source();
    let target = word(context, "player")?;
    let site = moderation.site.clone();

    Ok(moderation.call(source, async move {
        let limit = BansConfig::load().warn_limit;
        let result = site.warns(&target).await?;
        Ok(completion(move |source| {
            let Some((nickname, warns)) = result else {
                return Err(CommandError::from(format!("Игрок {target} не найден.")));
            };
            if warns.is_empty() {
                success(
                    source,
                    format!("У {nickname} нет предупреждений."),
                    None,
                    false,
                );
                return Ok(0);
            }

            success(
                source,
                format!("Предупреждения {nickname} ({}/{limit}):", warns.len()),
                Some(Color::Gold),
                false,
            );
            for warn in &warns {
                let time = warn
                    .created_at
                    .as_deref()
                    .map_or_else(|| "?".to_owned(), format_time);
                let by = warn.by.as_deref().map_or_default(|by| format!(", {by}"));
                let line = TextComponent::plain(format!("• {}", warn.reason))
                    .color(Color::White)
                    .add_child(TextComponent::plain(format!(" ({time}{by})")).color(Color::Gray));
                source.send_success(&line, false);
            }
            Ok(i32::try_from(warns.len()).unwrap_or(i32::MAX))
        }))
    }))
}

fn whitelist(
    moderation: &Arc<Moderation>,
    context: CommandContext<'_>,
    add: bool,
) -> Result<SiteCall, CommandError> {
    let source = context.source();
    let target = word(context, "target")?;
    let by = actor(source);
    let site = moderation.site.clone();
    let server_id = moderation.server_id.clone();

    Ok(moderation.call(source, async move {
        let result = site.whitelist(&target, add, &server_id, &by).await?;
        Ok(completion(move |source| {
            let name = result.name.as_deref().unwrap_or(&target);
            let done = |text: String| {
                success(source, text, Some(Color::Green), true);
                Ok(1)
            };
            let nothing = |text: String| {
                success(source, text, None, false);
                Ok(0)
            };
            match result.status.as_str() {
                "added" => done(format!(
                    "{name} добавлен в вайтлист. Бот выдаст роль «Игроки» и напишет ему в ЛС."
                )),
                "removed" => done(format!(
                    "{name} убран из вайтлиста. Если он в игре, сервер кикнет его в течение полуминуты."
                )),
                "already" => nothing(format!("{name} уже в вайтлисте.")),
                "not_whitelisted" => nothing(format!("{name} и так не в вайтлисте.")),
                "nickname_added" => done(format!(
                    "Ник {name} ещё ни к кому не привязан. Кто первым привяжет его к Discord (код при входе или на сайте), тот попадёт в вайтлист."
                )),
                "nickname_already" => {
                    nothing(format!("Ник {name} уже ждёт привязки в вайтлисте."))
                }
                "nickname_removed" => done(format!("Ник {name} больше не ждёт привязки.")),
                "unavailable" => Err(CommandError::from(
                    "Discord сейчас не отвечает, попробуй ещё раз.",
                )),
                _ if add => Err(CommandError::from(format!(
                    "Не нашли {target}: укажи ник (3–16 символов: латиница, цифры, _) или Discord ID."
                ))),
                _ => Err(CommandError::from(format!(
                    "Не нашли {target} в вайтлисте."
                ))),
            }
        }))
    }))
}
