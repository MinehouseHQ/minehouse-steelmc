//! `/show` (the Fabric `ShowCommands`): `optout` for every player, the rest for whoever has
//! `minehouse.command.show`.

use std::sync::Arc;

use serde_json::json;
use steel_core::{
    command::{
        CommandArgument, CommandArgumentParser, CommandContext, CommandError, CommandNode,
        CommandParserSource, CommandReader, CommandRegistration, CommandRegistrationError,
        CommandRegistry, CommandSource, CommandSuggestionContext, CommandSuggestions, argument,
        literal,
    },
    entity::Entity as _,
    player::Player,
};
use steel_protocol::packets::game::{ArgumentStringTypeBehavior, ArgumentType, SuggestionType};
use steel_utils::{DowncastType, DowncastTypeKey, Identifier};

use super::{
    SHOWS, Shows,
    camera_path::{CameraPath, Point},
    may_run_shows,
    runner::Mode,
    script::{ShowScript, Stage, snap},
    time,
};
use crate::moderation::format::render;

/// The console and admins.
fn admin(source: CommandSource<'_>) -> bool {
    source.player().is_none_or(|player| may_run_shows(player))
}

pub(super) fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "show"), tree).default_access(),
    )?;
    Ok(())
}

/// The id of a show: the shows that loaded are offered.
#[derive(Debug, PartialEq, Eq)]
struct ShowParser;

// SAFETY: This key is used for `ShowParser` alone.
unsafe impl DowncastType for ShowParser {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("minehouse:command/parser/show");
}

/// The value of a [`ShowParser`] argument.
#[derive(Debug)]
struct ShowId(String);

// SAFETY: This key is used for `ShowId` alone.
unsafe impl DowncastType for ShowId {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("minehouse:command/value/show");
}

impl CommandArgumentParser for ShowParser {
    type Value = ShowId;

    fn parse(
        &self,
        reader: &mut CommandReader<'_, '_>,
        _source: CommandParserSource<'_>,
    ) -> Result<Self::Value, CommandError> {
        let word = reader.read_unquoted_string();
        if word.is_empty() {
            return Err(reader.error("Укажи шоу (/show list)."));
        }
        Ok(ShowId(word.to_owned()))
    }

    fn list_suggestions(
        &self,
        _context: CommandSuggestionContext<'_>,
        suggestions: &mut CommandSuggestions<'_, '_>,
    ) {
        let typed = suggestions.remaining().to_lowercase();
        if let Some(shows) = SHOWS.lock().as_ref() {
            for id in shows.library.shows().keys() {
                if id.to_lowercase().starts_with(&typed) {
                    suggestions.suggest(id.as_str());
                }
            }
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

fn show_argument() -> CommandNode {
    argument("show", CommandArgument::custom(ShowParser))
}

fn show_id(context: CommandContext<'_>) -> Result<&str, CommandError> {
    context.value::<ShowId>("show").map(|id| id.0.as_str())
}

fn tree() -> CommandNode {
    literal("show")
        .then(literal("optout").executes(|context| opt_out(*context)))
        .then(
            literal("list")
                .requires(admin)
                .executes(|context| list(*context)),
        )
        .then(
            literal("reload")
                .requires(admin)
                .executes(|context| reload(*context)),
        )
        .then(
            literal("status")
                .requires(admin)
                .executes(|context| status(*context)),
        )
        .then(
            literal("stop")
                .requires(admin)
                .executes(|context| stop(*context)),
        )
        .then(
            literal("start").requires(admin).then(
                show_argument()
                    .executes(|context| start(*context, None))
                    .then(
                        argument("in", CommandArgument::word())
                            .executes(|context| start(*context, Some(context.string("in")?))),
                    ),
            ),
        )
        .then(
            literal("rehearse").requires(admin).then(
                show_argument()
                    .executes(|context| rehearse(*context, None))
                    .then(
                        argument("marker", CommandArgument::word()).executes(|context| {
                            rehearse(*context, Some(context.string("marker")?))
                        }),
                    ),
            ),
        )
        .then(
            literal("stage")
                .requires(admin)
                .then(show_argument().executes(|context| stage(*context))),
        )
        .then(
            literal("cam")
                .requires(admin)
                .then(literal("add").then(
                    show_argument().then(path_argument().executes(|context| cam_add(*context))),
                ))
                .then(literal("undo").then(
                    show_argument().then(path_argument().executes(|context| cam_undo(*context))),
                ))
                .then(literal("clear").then(
                    show_argument().then(path_argument().executes(|context| cam_clear(*context))),
                ))
                .then(
                    literal("preview").then(
                        show_argument().then(
                            path_argument()
                                .executes(|context| cam_preview(*context, "8s"))
                                .then(argument("over", CommandArgument::word()).executes(
                                    |context| cam_preview(*context, context.string("over")?),
                                )),
                        ),
                    ),
                ),
        )
}

fn path_argument() -> CommandNode {
    argument("path", CommandArgument::word())
}

fn ok(context: CommandContext<'_>, text: &str) {
    context.source().send_success(&render(text, &[]), false);
}

fn player(context: CommandContext<'_>) -> Result<&Arc<Player>, CommandError> {
    context
        .source()
        .player()
        .ok_or_else(|| CommandError::from("Только для игроков"))
}

/// Runs `action` on the shows of the running server.
fn with<T>(action: impl FnOnce(&mut Shows) -> Result<T, CommandError>) -> Result<T, CommandError> {
    let mut shows = SHOWS.lock();
    let shows = shows
        .as_mut()
        .ok_or_else(|| CommandError::from("Шоу ещё не загружены"))?;
    action(shows)
}

fn script(shows: &Shows, context: CommandContext<'_>) -> Result<Arc<ShowScript>, CommandError> {
    let id = show_id(context)?;
    shows.library.show(id).cloned().ok_or_else(|| {
        CommandError::from(shows.library.errors().get(id).map_or_else(
            || format!("Нет шоу {id} (/show list)"),
            |error| format!("Шоу {id} не загрузилось: {error}"),
        ))
    })
}

fn opt_out(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let player = player(context)?;
    let out = with(|shows| Ok(shows.toggle_opt_out(context.source().server(), player)))?;
    ok(
        context,
        if out {
            "&7Шоу вы теперь пропускаете. Вернуть: &f/show optout"
        } else {
            "&aВы снова смотрите шоу вместе со всеми."
        },
    );
    Ok(1)
}

fn list(context: CommandContext<'_>) -> Result<i32, CommandError> {
    with(|shows| {
        let library = &shows.library;
        if library.shows().is_empty() && library.errors().is_empty() {
            ok(
                context,
                &format!(
                    "&7Шоу нет. Положите сценарии в {}",
                    library.shows_dir().display()
                ),
            );
            return Ok(0);
        }
        for show in library.shows().values() {
            let stage = show.stage.as_ref().map_or_else(
                || " &cбез сцены".to_owned(),
                |stage| {
                    format!(
                        " &7сцена {} {} {}",
                        stage.pos.x.floor(),
                        stage.pos.y.floor(),
                        stage.pos.z.floor()
                    )
                },
            );
            ok(
                context,
                &format!(
                    "&6{} &f{} &7{}{stage}",
                    show.id,
                    show.title,
                    time::format(show.length)
                ),
            );
        }
        for (id, error) in library.errors() {
            ok(context, &format!("&c{id}: {error}"));
        }
        Ok(i32::try_from(library.shows().len()).unwrap_or(i32::MAX))
    })
}

fn reload(context: CommandContext<'_>) -> Result<i32, CommandError> {
    with(|shows| {
        if shows.runner.is_some() {
            return Err(CommandError::from(
                "Идёт шоу, перезагрузка после него (/show stop)",
            ));
        }
        let loaded = shows.library.reload();
        let errors = shows.library.errors().len();
        ok(
            context,
            &if errors == 0 {
                format!("&aЗагружено шоу: {loaded}")
            } else {
                format!("&aЗагружено шоу: {loaded}, &cс ошибками: {errors} (/show list)")
            },
        );
        Ok(i32::try_from(loaded).unwrap_or(i32::MAX))
    })
}

fn status(context: CommandContext<'_>) -> Result<i32, CommandError> {
    with(|shows| {
        if let Some(runner) = &shows.runner {
            let online = context.source().server().get_players();
            ok(
                context,
                &format!(
                    "&6{}{} &f{} / {}&7, зрителей онлайн: &f{}",
                    runner.script().id,
                    if runner.mode() == Mode::Rehearsal {
                        " &7(репетиция)"
                    } else {
                        ""
                    },
                    time::format(runner.tick()),
                    time::format(runner.script().length),
                    runner.viewers_online(&online).len()
                ),
            );
        } else if let Some(countdown) = &shows.countdown {
            ok(
                context,
                &format!(
                    "&7Отсчёт до шоу &6{}&7: &f{}",
                    countdown.script.id,
                    time::format(countdown.left)
                ),
            );
        } else {
            ok(context, "&7Сейчас шоу не идёт.");
        }
        Ok(1)
    })
}

fn stop(context: CommandContext<'_>) -> Result<i32, CommandError> {
    with(|shows| {
        let countdown = shows.cancel_countdown();
        let show = shows.stop(context.source().server());
        if !countdown && !show {
            return Err(CommandError::from("Шоу не идёт и не запланировано"));
        }
        ok(
            context,
            if show {
                "&eШоу остановлено, всё возвращено на место."
            } else {
                "&eОтсчёт отменён."
            },
        );
        Ok(1)
    })
}

fn start(context: CommandContext<'_>, after: Option<&str>) -> Result<i32, CommandError> {
    with(|shows| {
        let script = script(shows, context)?;
        let server = context.source().server();
        if let Some(after) = after {
            let ticks = time::parse(after).map_err(|error| {
                CommandError::from(format!("{error} (например 10m, 90s, 1:30)"))
            })?;
            if script.stage.is_none() {
                return Err(CommandError::from(format!(
                    "У шоу нет сцены: /show stage {}",
                    script.id
                )));
            }
            // What would stop the show is said now, not when the countdown runs out.
            shows
                .library
                .paths_of(&script)
                .map_err(CommandError::from)?;
            shows.start_countdown(server, &script, ticks);
            ok(
                context,
                &format!(
                    "&aОтсчёт пошёл: &6{} &aчерез {}",
                    script.title,
                    time::format(ticks)
                ),
            );
            return Ok(1);
        }
        shows
            .start_show(server, &script, None, 0)
            .map_err(|error| CommandError::from(format!("Не запустилось: {error}")))?;
        ok(
            context,
            &format!("&aШоу &6{} &aидёт. Остановить: /show stop", script.title),
        );
        Ok(1)
    })
}

fn rehearse(context: CommandContext<'_>, marker: Option<&str>) -> Result<i32, CommandError> {
    let player = player(context)?;
    with(|shows| {
        let script = script(shows, context)?;
        let from = match marker {
            None => 0,
            Some(marker) => *script.markers.get(marker).ok_or_else(|| {
                let known: Vec<&str> = script.markers.keys().map(String::as_str).collect();
                CommandError::from(format!("Нет маркера {marker} (есть: {})", known.join(", ")))
            })?,
        };
        shows
            .start_show(context.source().server(), &script, Some(player), from)
            .map_err(|error| CommandError::from(format!("Не запустилось: {error}")))?;
        ok(
            context,
            &format!(
                "&aРепетиция &6{}&a только для вас. Остановить: /show stop",
                script.title
            ),
        );
        Ok(1)
    })
}

fn stage(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let player = player(context)?;
    let id = show_id(context)?;
    let feet = player.position().floor();
    let (yaw, _) = player.rotation();
    let stage = Stage {
        world: player.get_world().key.to_string(),
        pos: feet + glam::DVec3::new(0.5, 0.0, 0.5),
        yaw: snap(f64::from(yaw)),
    };
    with(|shows| {
        if shows.runner.is_some() {
            return Err(CommandError::from(
                "Идёт шоу, сцену можно сменить после него (/show stop)",
            ));
        }
        shows
            .library
            .set_stage(id, &stage)
            .map_err(|error| CommandError::from(format!("Не сохранилось: {error}")))
    })?;
    let facing = match stage.yaw {
        90 => "запад",
        180 => "север",
        270 => "восток",
        _ => "юг",
    };
    ok(
        context,
        &format!(
            "&aСцена шоу &6{id}&a здесь: {} {} {}. Все координаты сценария теперь считаются от \
             этой точки; у шоу, которые поворачиваются вместе со сценой, «вперёд» — это {facing}.",
            feet.x, feet.y, feet.z
        ),
    );
    Ok(1)
}

/// The stage a camera path of show `id` is recorded against.
fn stage_for(shows: &Shows, id: &str) -> Result<Stage, CommandError> {
    shows
        .library
        .stage_of(id)
        .ok_or_else(|| CommandError::from(format!("Сначала задайте сцену: /show stage {id}")))
}

/// Adds where the admin stands and looks as the next point of a camera path.
fn cam_add(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let player = player(context)?;
    let id = show_id(context)?;
    let name = context.string("path")?;
    let feet = player.position();
    let eye = glam::DVec3::new(feet.x, player.get_eye_y(), feet.z);
    let (yaw, pitch) = player.rotation();
    let count = with(|shows| {
        let stage = stage_for(shows, id)?;
        let mut points = shows
            .library
            .path_or_none(id, name)
            .map_err(|error| CommandError::from(format!("Не записалось: {error}")))?
            .map_or_default(|path| path.points().to_vec());
        points.push(Point {
            pos: eye - stage.pos,
            yaw,
            pitch,
        });
        let count = points.len();
        CameraPath::new(points, None)
            .and_then(|path| shows.library.save_path(id, name, &path))
            .map_err(|error| CommandError::from(format!("Не записалось: {error}")))?;
        Ok(count)
    })?;
    ok(
        context,
        &format!(
            "&aТочка {count} пути &6{name}&a записана. Посмотреть: /show cam preview {id} {name}"
        ),
    );
    Ok(i32::try_from(count).unwrap_or(i32::MAX))
}

/// Takes the last point of a camera path back.
fn cam_undo(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let id = show_id(context)?;
    let name = context.string("path")?;
    let left = with(|shows| {
        let failed = |error: String| CommandError::from(format!("Не вышло: {error}"));
        let mut points = shows
            .library
            .path_or_none(id, name)
            .map_err(failed)?
            .map_or_default(|path| path.points().to_vec());
        if points.pop().is_none() {
            return Err(CommandError::from(format!("В пути {name} нет точек")));
        }
        let left = points.len();
        if points.is_empty() {
            shows.library.delete_path(id, name).map_err(failed)?;
        } else {
            CameraPath::new(points, None)
                .and_then(|path| shows.library.save_path(id, name, &path))
                .map_err(failed)?;
        }
        Ok(left)
    })?;
    ok(
        context,
        &format!("&eПоследняя точка убрана, осталось {left}"),
    );
    Ok(i32::try_from(left).unwrap_or(i32::MAX))
}

fn cam_clear(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let id = show_id(context)?;
    let name = context.string("path")?;
    with(|shows| {
        shows
            .library
            .delete_path(id, name)
            .map_err(|error| CommandError::from(format!("Не вышло: {error}")))
    })?;
    ok(context, &format!("&eПуть {name} удалён"));
    Ok(1)
}

/// Flies the admin alone along a recorded path, then puts them back.
fn cam_preview(context: CommandContext<'_>, over: &str) -> Result<i32, CommandError> {
    let player = player(context)?;
    let id = show_id(context)?;
    let name = context.string("path")?;
    time::parse(over)
        .map_err(|error| CommandError::from(format!("{error} (например 8s, 1:30)")))?;
    with(|shows| {
        let stage = stage_for(shows, id)?;
        // The admin watched a camera path: back to where they were afterwards.
        let preview = json!({
            "title": format!("Просмотр пути {name}"),
            "audience": {"gather": "none", "returnAfter": true},
            "actors": {"cam": {"type": "camera"}},
            "cues": [
                {"at": "0", "do": "cinematic"},
                {"at": "0", "do": "path", "path": name, "over": over},
                {"at": over, "do": "cinematic", "on": false},
            ],
            "length": over,
        });
        let script =
            ShowScript::parse(id, &preview.to_string(), Some(stage)).map_err(CommandError::from)?;
        shows
            .start_show(
                context.source().server(),
                &Arc::new(script),
                Some(player),
                0,
            )
            .map_err(|error| CommandError::from(format!("Не запустилось: {error}")))
    })?;
    Ok(1)
}
