//! `MineHouse` Events (the Fabric mod of the same name): shows and cutscenes played from
//! scripts in `config/minehouse/events/shows/`. A script is a timeline of cues: the camera
//! flies, actors (display entities) appear and move, the sky and the weather change, sounds
//! and titles play. Everything a show does is seen only by its viewers and is undone when
//! it ends; where each viewer stood and in what game mode is kept in a journal on disk, so
//! they are put back even if the server dies mid-show.
//!
//! `/show` runs them: `list`, `reload`, `stage <show>` (the place the script's coordinates
//! are counted from), `start <show> [in]` (now, or after a countdown on a boss bar),
//! `rehearse <show> [marker]` (for one admin, from a marker), `stop`, `status`, and
//! `optout` for a player who doesn't want to watch.
//!
//! Not ported yet, and refused when a script uses them: blocks placed by a show, models,
//! trains, rigs, mannequins and mobs as actors, meteors, rifts and rockets, node logic,
//! private cutscenes, recorded camera paths and the in-game editor.

mod actor;
mod camera_path;
mod commands;
mod ease;
mod journal;
mod json;
mod library;
mod runner;
mod script;
mod time;

#[cfg(test)]
mod tests;

use std::{
    str::FromStr as _,
    sync::{Arc, LazyLock},
};

use rustc_hash::FxHashSet;
use steel_core::{
    boss_event::ServerBossEvent,
    command::{CommandRegistrationError, CommandRegistry},
    permission::{PermissionExpr, PermissionKey},
    player::Player,
    server::{
        Server,
        jobs::{JobPoll, ServerJob, ServerJobContext},
    },
};
use steel_protocol::packets::game::{BossBarColor, BossBarOverlay};
use steel_utils::{Identifier, locks::SyncMutex};
use text_components::TextComponent;
use uuid::Uuid;

use self::{
    journal::Journal,
    library::Library,
    runner::{Cx, Mode, Runner, restore_viewer},
    script::ShowScript,
};
use crate::{config, downtime::Downtime, moderation::format::render};

/// Who may run shows.
const PERMISSION: &str = "minehouse.command.show";

struct Countdown {
    script: Arc<ShowScript>,
    left: u32,
    total: u32,
    bar: Arc<ServerBossEvent>,
}

/// Everything the module keeps while the server runs.
struct Shows {
    library: Library,
    journal: Journal,
    runner: Option<Runner>,
    countdown: Option<Countdown>,
    opted_out: FxHashSet<Uuid>,
    /// Who was online the tick before: whoever is new has just joined.
    online: FxHashSet<Uuid>,
    downtime: Option<Arc<Downtime>>,
}

static SHOWS: LazyLock<SyncMutex<Option<Shows>>> = LazyLock::new(|| SyncMutex::new(None));

/// Registers `/show`.
pub fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    commands::register(registry)
}

/// Reads the shows and starts playing them. `downtime` keeps shows from starting during
/// maintenance.
pub fn start(server: &Arc<Server>, downtime: Option<Arc<Downtime>>) {
    let mut library = Library::new();
    let loaded = library.reload();
    log::info!(
        "MineHouse events: {loaded} show(s) loaded from {}{}",
        library.shows_dir().display(),
        if library.errors().is_empty() {
            String::new()
        } else {
            format!(", {} with errors", library.errors().len())
        }
    );
    let mut journal = Journal::load(config::data_path("events/journal.json"));
    if let Some(show) = journal.active_show() {
        // The server stopped mid-show; its viewers are put back as they join.
        log::warn!(
            "MineHouse events: show {show} did not finish (the server stopped), {} viewer(s) \
             will be put back on join",
            journal.viewers()
        );
        journal.set_active_show(None);
        journal.flush();
    }
    *SHOWS.lock() = Some(Shows {
        library,
        journal,
        runner: None,
        countdown: None,
        opted_out: FxHashSet::default(),
        online: FxHashSet::default(),
        downtime,
    });
    server.jobs.spawn(Tick);
}

struct Tick;

impl ServerJob for Tick {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        let Some(server) = context.server() else {
            return JobPoll::Finished;
        };
        if let Some(shows) = SHOWS.lock().as_mut() {
            shows.tick(&server);
        }
        JobPoll::Pending
    }
}

fn may_run_shows(player: &Player) -> bool {
    PermissionKey::parse(PERMISSION)
        .ok()
        .map(PermissionExpr::key)
        .is_some_and(|permission| player.has_permission(&permission))
}

/// `"&6Title &fчерез &e1:30"` for the boss bar.
fn countdown_text(script: &ShowScript, left: u32) -> TextComponent {
    render(
        &format!(
            "&6{} &fчерез &e{}",
            script.countdown_name(),
            time::format(left)
        ),
        &[],
    )
}

impl Shows {
    fn tick(&mut self, server: &Arc<Server>) {
        let online = server.get_players();
        let now: FxHashSet<Uuid> = online
            .iter()
            .map(|player| player.gameprofile.id())
            .collect();
        for player in &online {
            if !self.online.contains(&player.gameprofile.id()) {
                self.player_joined(server, player);
            }
        }
        self.online = now;

        let due = self.countdown.as_mut().and_then(|countdown| {
            countdown.left = countdown.left.saturating_sub(1);
            if countdown.left == 0 {
                return Some(Arc::clone(&countdown.script));
            }
            if countdown.left % time::TICKS_PER_SECOND == 0 {
                countdown
                    .bar
                    .set_name(countdown_text(&countdown.script, countdown.left));
                countdown
                    .bar
                    .set_progress(countdown.left as f32 / countdown.total.max(1) as f32);
            }
            None
        });
        if let Some(script) = due {
            self.cancel_countdown();
            if let Err(error) = self.start_show(server, &script, None, 0) {
                notify_admins(&online, &format!("Шоу {} не началось: {error}", script.id));
            }
        }

        if let Some(runner) = &mut self.runner {
            runner.step(&mut Cx {
                server,
                online: &online,
                journal: &mut self.journal,
            });
            if runner.finished() {
                self.runner = None;
            }
        }
        self.journal.flush();
    }

    fn player_joined(&mut self, server: &Arc<Server>, player: &Arc<Player>) {
        if let Some(countdown) = &self.countdown {
            countdown.bar.add_player(player);
        }
        let id = player.gameprofile.id();
        if let Some(runner) = &mut self.runner {
            let watching = runner.is_viewer(id);
            if watching || (runner.mode() == Mode::Live && !self.opted_out.contains(&id)) {
                // Back mid-show (or new): into the show; their pre-show spot stays the one
                // saved first.
                runner.rejoined(id);
                runner.add_viewer(player, &mut self.journal);
                return;
            }
        }
        if self.journal.viewer(id).is_some() {
            // The show ended (or the server fell) while they were away.
            restore_viewer(server, player, &mut self.journal);
        }
    }

    /// Starts a show now, for everyone or as a rehearsal for one admin.
    fn start_show(
        &mut self,
        server: &Arc<Server>,
        script: &Arc<ShowScript>,
        rehearsal_for: Option<&Arc<Player>>,
        from_tick: u32,
    ) -> Result<(), String> {
        if let Some(runner) = &self.runner {
            return Err(format!("уже идёт шоу {} (/show stop)", runner.script().id));
        }
        let Some(stage) = &script.stage else {
            return Err(format!(
                "у шоу нет сцены: встаньте на место и выполните /show stage {}",
                script.id
            ));
        };
        let world = Identifier::from_str(&stage.world)
            .ok()
            .and_then(|key| server.worlds.get(&key).cloned())
            .ok_or_else(|| format!("нет мира {}", stage.world))?;
        let maintenance = self
            .downtime
            .as_ref()
            .is_some_and(|downtime| downtime.in_maintenance());
        if rehearsal_for.is_none() && maintenance {
            return Err("на сервере режим техработ".to_owned());
        }

        let online = server.get_players();
        let (mode, audience) = match rehearsal_for {
            Some(player) => (Mode::Rehearsal, vec![Arc::clone(player)]),
            None => (
                Mode::Live,
                online
                    .iter()
                    .filter(|player| !self.opted_out.contains(&player.gameprofile.id()))
                    .cloned()
                    .collect(),
            ),
        };
        let mut runner = Runner::new(Arc::clone(script), world, stage.pos, mode);
        runner.start(
            &audience,
            from_tick,
            &mut Cx {
                server,
                online: &online,
                journal: &mut self.journal,
            },
        );
        log::info!(
            "MineHouse events: show {} started{} for {} viewer(s)",
            script.id,
            if mode == Mode::Rehearsal {
                " (rehearsal)"
            } else {
                ""
            },
            audience.len()
        );
        self.runner = Some(runner);
        Ok(())
    }

    fn start_countdown(&mut self, server: &Arc<Server>, script: &Arc<ShowScript>, ticks: u32) {
        self.cancel_countdown();
        let bar = Arc::new(ServerBossEvent::new(
            Uuid::new_v4(),
            countdown_text(script, ticks),
            BossBarColor::Purple,
            BossBarOverlay::Notched10,
        ));
        for player in server.get_players() {
            bar.add_player(&player);
        }
        self.countdown = Some(Countdown {
            script: Arc::clone(script),
            left: ticks,
            total: ticks,
            bar,
        });
        let announce = script.countdown.as_ref().map_or_else(
            || format!("&6&l{} &r&fначнётся через &e", script.title),
            |countdown| format!("&6&l{countdown} &r&fчерез &e"),
        );
        server.broadcast_system_chat(
            &render(
                &format!(
                    "{announce}{}&f. Закончите дела: во время шоу вы будете неуязвимы и потом \
                     вернётесь на своё место. Не хотите смотреть: &7/show optout",
                    time::format(ticks)
                ),
                &[],
            ),
            None,
        );
    }

    fn cancel_countdown(&mut self) -> bool {
        let Some(countdown) = self.countdown.take() else {
            return false;
        };
        countdown.bar.remove_all_players();
        true
    }

    /// Stops the running show, putting everything back.
    fn stop(&mut self, server: &Arc<Server>) -> bool {
        let Some(mut runner) = self.runner.take() else {
            return false;
        };
        runner.finish(&mut Cx {
            server,
            online: &server.get_players(),
            journal: &mut self.journal,
        });
        true
    }

    /// Returns true if the player now skips shows.
    fn toggle_opt_out(&mut self, server: &Arc<Server>, player: &Arc<Player>) -> bool {
        let id = player.gameprofile.id();
        if self.opted_out.remove(&id) {
            return false;
        }
        self.opted_out.insert(id);
        if let Some(runner) = &mut self.runner {
            runner.drop_viewer(
                player,
                &mut Cx {
                    server,
                    online: &server.get_players(),
                    journal: &mut self.journal,
                },
            );
        }
        true
    }
}

fn notify_admins(online: &[Arc<Player>], message: &str) {
    log::info!("MineHouse events: {message}");
    let text = TextComponent::plain(message.to_owned());
    for player in online {
        if may_run_shows(player) {
            player.send_message(&text);
        }
    }
}
