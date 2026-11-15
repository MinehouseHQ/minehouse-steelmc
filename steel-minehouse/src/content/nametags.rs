//! Every player chooses whether they see the names over other players' heads (the Fabric
//! `Nametags`).
//!
//! The vanilla client hides a name when the target's team says `nametagVisibility=never`,
//! and each client is told about teams on its own connection. So the scoreboard does not
//! change: for a player who hides names, every team packet sent to them is rewritten to
//! `never` (prefixes, colors and the rest stay), and players who are in no team are put,
//! only on that client, into a private team [`FAKE_TEAM`] that also says `never`.
//!
//! The client must never get a "leave team" for a team it does not have the player in, or
//! it drops the connection. That holds because real team packets are passed on unchanged
//! apart from visibility, and the private team only ever gets "join" packets: when a player
//! joins a real team the client moves them out of the private team by itself, and when they
//! leave it they end up in no team, after which the tick adds them back.
//!
//! The choice is kept per UUID in the data directory, in the format of the Fabric
//! `minehouse-nametags.properties`. Whoever has not chosen yet gets a vanilla dialog shortly
//! after joining; `/nametags` toggles it later.

use std::{
    collections::BTreeMap,
    fs, io, iter,
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::Arc,
};

use rustc_hash::{FxHashMap, FxHashSet};
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_core::{
    command::{
        CommandContext, CommandError, CommandNode, CommandRegistration, CommandRegistrationError,
        CommandRegistry, literal,
    },
    player::Player,
    scoreboard::{ScoreHolder, TeamPacketObserver},
    server::{
        Server,
        jobs::{JobPoll, ServerJob, ServerJobContext},
    },
};
use steel_protocol::packets::game::{
    CSetPlayerTeam, CShowDialog, TeamCollisionRule, TeamMethod, TeamParameters, TeamVisibility,
};
use steel_utils::{Identifier, locks::SyncMutex};
use text_components::TextComponent;
use uuid::Uuid;

use crate::{config::data_path, moderation::format::render};

/// The file with the choices, in the data directory.
const STATE_FILE: &str = "nametags.properties";
/// Not a valid scoreboard team name on the server, so it can't clash.
const FAKE_TEAM: &str = "!minehouse_names";
const DIALOG_DELAY_TICKS: u64 = 40;
const SYNC_EVERY_TICKS: u64 = 5;
const BODY_WIDTH: i32 = 250;
const BUTTON_WIDTH: i32 = 120;

const fn show_action() -> Identifier {
    Identifier::new_static("minehouse", "nametags/show")
}

const fn hide_action() -> Identifier {
    Identifier::new_static("minehouse", "nametags/hide")
}

/// What is known about the players' choices and their clients.
#[derive(Default)]
struct State {
    /// `true` hides names, `false` shows them, absent has not chosen yet.
    choices: BTreeMap<Uuid, bool>,
    /// Online players who hide names: the names their client has in the private team.
    hiding: FxHashMap<Uuid, FxHashSet<String>>,
    file: Option<PathBuf>,
}

static STATE: SyncMutex<Option<State>> = SyncMutex::new(None);

fn with<T>(run: impl FnOnce(&mut State) -> T) -> T {
    run(STATE.lock().get_or_insert_with(State::default))
}

/// Reads the choices from the text of the file: `uuid=hide` or `uuid=show` a line. Lines
/// that are not that (comments, hand edits) are skipped.
fn parse(text: &str) -> BTreeMap<Uuid, bool> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            let id = Uuid::from_str(key.trim()).ok()?;
            Some((id, value.trim().eq_ignore_ascii_case("hide")))
        })
        .collect()
}

fn format(choices: &BTreeMap<Uuid, bool>) -> String {
    let header = "#Names over players' heads: hide or show, per player UUID (/nametags)";
    let lines = choices
        .iter()
        .map(|(id, hide)| format!("{id}={}", if *hide { "hide" } else { "show" }));
    let mut text = iter::once(header.to_owned())
        .chain(lines)
        .collect::<Vec<_>>()
        .join("\n");
    text.push('\n');
    text
}

fn save(file: &Path, choices: &BTreeMap<Uuid, bool>) -> io::Result<()> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = file.with_extension("properties.tmp");
    fs::write(&tmp, format(choices))?;
    fs::rename(&tmp, file)
}

/// Whether team packets to this player get their name tags turned off.
fn hides(id: Uuid) -> bool {
    with(|state| state.choices.get(&id).copied().unwrap_or(false))
}

/// `parameters` with the name tags off, or `None` if they already are.
fn hidden(parameters: &TeamParameters) -> Option<TeamParameters> {
    (parameters.name_tag_visibility != TeamVisibility::Never).then(|| TeamParameters {
        name_tag_visibility: TeamVisibility::Never,
        ..parameters.clone()
    })
}

/// The packet a player who hides names gets instead of `packet`, if it has to differ.
fn rewritten(packet: &CSetPlayerTeam) -> Option<CSetPlayerTeam> {
    let method = match &packet.method {
        TeamMethod::Add(parameters, players) => {
            TeamMethod::Add(hidden(parameters)?, players.clone())
        }
        TeamMethod::Change(parameters) => TeamMethod::Change(hidden(parameters)?),
        TeamMethod::Remove | TeamMethod::Join(_) | TeamMethod::Leave(_) => return None,
    };
    Some(CSetPlayerTeam {
        name: packet.name.clone(),
        method,
    })
}

/// Rewrites the team packets of players who hide names.
pub struct Nametags;

/// The one instance, installed at start.
pub static NAMETAGS: Nametags = Nametags;

impl TeamPacketObserver for Nametags {
    fn rewrite(&self, viewer: &Player, packet: &CSetPlayerTeam) -> Option<CSetPlayerTeam> {
        let id = viewer.gameprofile.id();
        if packet.name == FAKE_TEAM || !hides(id) {
            return None;
        }
        if let TeamMethod::Add(_, players) | TeamMethod::Join(players) = &packet.method {
            // The client moves them out of the private team by itself.
            with(|state| {
                if let Some(known) = state.hiding.get_mut(&id) {
                    for player in players {
                        known.remove(player);
                    }
                }
            });
        }
        rewritten(packet)
    }
}

/// Online players other than the viewer who are in no team of the viewer's scoreboard and
/// not yet in the viewer's private team.
fn teamless(server: &Server, viewer: &Player, players: &[Arc<Player>]) -> Vec<String> {
    let world = viewer.get_world();
    let Some(scoreboard) = server.scoreboards.get(world.domain()) else {
        return Vec::new();
    };
    let id = viewer.gameprofile.id();
    let candidates: Vec<String> = players
        .iter()
        .filter(|other| other.gameprofile.id() != id)
        .map(|other| other.gameprofile.name().to_owned())
        .filter(|name| {
            scoreboard
                .holder_team_name(&ScoreHolder::new(name.as_str()))
                .is_none()
        })
        .collect();
    with(|state| {
        let Some(known) = state.hiding.get_mut(&id) else {
            return Vec::new();
        };
        let missing: Vec<String> = candidates
            .into_iter()
            .filter(|name| !known.contains(name))
            .collect();
        known.extend(missing.iter().cloned());
        missing
    })
}

fn fake_team(method: TeamMethod) -> CSetPlayerTeam {
    CSetPlayerTeam {
        name: FAKE_TEAM.to_owned(),
        method,
    }
}

/// Creates the private team on the player's client with everyone who is in no team.
fn start_hiding(server: &Server, player: &Player) {
    with(|state| {
        state
            .hiding
            .insert(player.gameprofile.id(), FxHashSet::default());
    });
    let names = teamless(server, player, &server.get_players());
    let parameters = TeamParameters {
        display_name: TextComponent::new(),
        player_prefix: TextComponent::new(),
        player_suffix: TextComponent::new(),
        name_tag_visibility: TeamVisibility::Never,
        collision_rule: TeamCollisionRule::Always,
        color: None,
        options: 0,
    };
    player.send_packet(fake_team(TeamMethod::Add(parameters, names)));
}

/// Sends every team's settings again; the observer decides the visibility for this player.
fn resend_teams(server: &Server, player: &Player) {
    let world = player.get_world();
    if let Some(scoreboard) = server.scoreboards.get(world.domain()) {
        scoreboard.resend_team_settings(player);
    }
}

/// Sets the player's choice, tells their client and says so in chat.
fn set(player: &Player, hide: bool) {
    let Some(server) = player.try_server() else {
        return;
    };
    let id = player.gameprofile.id();
    let (changed, active, file, choices) = with(|state| {
        let before = state.choices.insert(id, hide);
        (
            before != Some(hide),
            state.hiding.contains_key(&id),
            state.file.clone(),
            state.choices.clone(),
        )
    });
    if changed
        && let Some(file) = file
        && let Err(error) = save(&file, &choices)
    {
        log::error!(
            "MineHouse nametags: could not save {}: {error}",
            file.display()
        );
    }
    if hide && !active {
        start_hiding(&server, player);
        resend_teams(&server, player);
    } else if !hide && active {
        with(|state| state.hiding.remove(&id));
        player.send_packet(fake_team(TeamMethod::Remove));
        resend_teams(&server, player);
    }
    player.send_message(&render(
        if hide {
            "&7Ники над игроками &cскрыты&7. Вернуть: &f/nametags"
        } else {
            "&7Ники над игроками &aпоказываются&7. Скрыть: &f/nametags"
        },
        &[],
    ));
}

/// A dialog button of ours. `true` if `id` was one.
pub(super) fn clicked(player: &Player, id: &Identifier) -> bool {
    let hide = if *id == hide_action() {
        true
    } else if *id == show_action() {
        false
    } else {
        return false;
    };
    set(player, hide);
    true
}

fn button(label: &str, tooltip: &str, action: &Identifier) -> NbtCompound {
    let mut click = NbtCompound::new();
    click.insert("type", "minecraft:custom");
    click.insert("id", action.to_string());
    let mut button = NbtCompound::new();
    button.insert(
        "label",
        TextComponent::plain(label.to_owned()).to_codec_nbt(),
    );
    button.insert(
        "tooltip",
        TextComponent::plain(tooltip.to_owned()).to_codec_nbt(),
    );
    button.insert("width", BUTTON_WIDTH);
    button.insert("action", NbtTag::Compound(click));
    button
}

/// The question, as vanilla's `Dialog.DIRECT_CODEC` reads it: a `confirmation` dialog that
/// only a button closes.
fn dialog() -> NbtTag {
    let mut message = NbtCompound::new();
    message.insert("type", "minecraft:plain_message");
    message.insert(
        "contents",
        TextComponent::plain(
            "Показывать ники над головами других игроков?\n\n\
             Без ников мир выглядит чище, но труднее понять, кто перед тобой. \
             Передумать можно в любой момент командой /nametags."
                .to_owned(),
        )
        .to_codec_nbt(),
    );
    message.insert("width", BODY_WIDTH);

    let mut dialog = NbtCompound::new();
    dialog.insert("type", "minecraft:confirmation");
    dialog.insert(
        "title",
        TextComponent::plain("Ники над игроками".to_owned()).to_codec_nbt(),
    );
    dialog.insert("can_close_with_escape", false);
    dialog.insert("pause", false);
    dialog.insert("after_action", "close");
    dialog.insert("body", NbtTag::List(NbtList::Compound(vec![message])));
    dialog.insert(
        "yes",
        NbtTag::Compound(button(
            "Показывать",
            "Ники видны, как в обычном Minecraft",
            &show_action(),
        )),
    );
    dialog.insert(
        "no",
        NbtTag::Compound(button(
            "Скрыть",
            "Над игроками ничего не написано",
            &hide_action(),
        )),
    );
    NbtTag::Compound(dialog)
}

fn open_dialog(player: &Player) {
    player.send_packet(CShowDialog::new(dialog()));
}

/// Asks newcomers and keeps the private teams of those who hide names complete. A job of
/// the server tick, never finished.
#[derive(Default)]
struct Watch {
    online: FxHashSet<Uuid>,
    /// Players who still have to be asked, with the tick to ask at.
    ask_at: FxHashMap<Uuid, u64>,
}

impl ServerJob for Watch {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        let Some(server) = context.server() else {
            return JobPoll::Finished;
        };
        let tick = context.tick_count;
        let players = server.get_players();
        let now: FxHashSet<Uuid> = players
            .iter()
            .map(|player| player.gameprofile.id())
            .collect();

        for player in &players {
            let id = player.gameprofile.id();
            if self.online.contains(&id) {
                continue;
            }
            match with(|state| state.choices.get(&id).copied()) {
                Some(true) => start_hiding(&server, player),
                Some(false) => {}
                None => {
                    self.ask_at.insert(id, tick + DIALOG_DELAY_TICKS);
                }
            }
        }
        with(|state| state.hiding.retain(|id, _| now.contains(id)));
        self.ask_at.retain(|id, _| now.contains(id));
        self.online = now;

        self.ask_at.retain(|id, at| {
            if *at > tick {
                return true;
            }
            if with(|state| !state.choices.contains_key(id))
                && let Some(player) = players.iter().find(|player| player.gameprofile.id() == *id)
            {
                open_dialog(player);
            }
            false
        });

        if tick.is_multiple_of(SYNC_EVERY_TICKS) {
            for viewer in &players {
                let missing = teamless(&server, viewer, &players);
                if !missing.is_empty() {
                    viewer.send_packet(fake_team(TeamMethod::Join(missing)));
                }
            }
        }
        JobPoll::Pending
    }
}

/// Loads the choices and starts the watch.
pub(super) fn start(server: &Server) {
    let file = data_path(STATE_FILE);
    let choices = match fs::read_to_string(&file) {
        Ok(text) => parse(&text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
        Err(error) => {
            log::error!(
                "MineHouse nametags: could not read {}, everyone is asked again: {error}",
                file.display()
            );
            BTreeMap::new()
        }
    };
    log::info!("MineHouse nametags on: {} choices", choices.len());
    with(|state| {
        state.choices = choices;
        state.file = Some(file);
    });
    server.jobs.spawn(Watch::default());
}

pub(super) fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    // Every player may choose, as on the Fabric server.
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "nametags"), tree).default_access(),
    )?;
    Ok(())
}

fn tree() -> CommandNode {
    literal("nametags")
        .executes(|context| toggle(*context, None))
        .then(literal("show").executes(|context| toggle(*context, Some(false))))
        .then(literal("hide").executes(|context| toggle(*context, Some(true))))
        .then(literal("menu").executes(|context| {
            let player = player_of(*context)?;
            open_dialog(&player);
            Ok(1)
        }))
}

fn player_of(context: CommandContext<'_>) -> Result<Arc<Player>, CommandError> {
    context
        .source()
        .player()
        .cloned()
        .ok_or_else(|| CommandError::from("Только для игроков"))
}

fn toggle(context: CommandContext<'_>, hide: Option<bool>) -> Result<i32, CommandError> {
    let player = player_of(context)?;
    let hide = hide.unwrap_or_else(|| !hides(player.gameprofile.id()));
    set(&player, hide);
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parameters(visibility: TeamVisibility) -> TeamParameters {
        TeamParameters {
            display_name: TextComponent::plain("Red".to_owned()),
            player_prefix: TextComponent::plain("[R] ".to_owned()),
            player_suffix: TextComponent::new(),
            name_tag_visibility: visibility,
            collision_rule: TeamCollisionRule::Never,
            color: Some(12),
            options: 3,
        }
    }

    #[test]
    fn the_file_is_the_fabric_one() {
        let hide = Uuid::from_u128(1);
        let show = Uuid::from_u128(2);
        let text = format!("#comment\n{hide}=hide\n {show} = SHOW \nnot-a-uuid=hide\n\n");
        let choices = parse(&text);
        assert_eq!(choices.len(), 2);
        assert_eq!(choices.get(&hide), Some(&true));
        assert_eq!(choices.get(&show), Some(&false));
        assert_eq!(parse(&format(&choices)), choices);
    }

    #[test]
    fn a_team_keeps_everything_but_the_visibility() {
        let packet = CSetPlayerTeam {
            name: "red".to_owned(),
            method: TeamMethod::Add(parameters(TeamVisibility::Always), vec!["Steve".to_owned()]),
        };
        let expected = CSetPlayerTeam {
            name: "red".to_owned(),
            method: TeamMethod::Add(parameters(TeamVisibility::Never), vec!["Steve".to_owned()]),
        };
        assert_eq!(rewritten(&packet), Some(expected));

        let change = CSetPlayerTeam {
            name: "red".to_owned(),
            method: TeamMethod::Change(parameters(TeamVisibility::HideForOtherTeams)),
        };
        assert_eq!(
            rewritten(&change).map(|packet| packet.method),
            Some(TeamMethod::Change(parameters(TeamVisibility::Never)))
        );
    }

    #[test]
    fn packets_without_settings_and_hidden_teams_pass_as_they_are() {
        for method in [
            TeamMethod::Remove,
            TeamMethod::Join(vec!["Steve".to_owned()]),
            TeamMethod::Leave(vec!["Steve".to_owned()]),
            TeamMethod::Change(parameters(TeamVisibility::Never)),
        ] {
            let packet = CSetPlayerTeam {
                name: "red".to_owned(),
                method,
            };
            assert_eq!(rewritten(&packet), None);
        }
    }

    #[test]
    fn the_dialog_asks_with_two_custom_buttons() {
        let NbtTag::Compound(dialog) = dialog() else {
            panic!("a dialog is a compound");
        };
        assert_eq!(
            dialog.string("type").map(ToString::to_string).as_deref(),
            Some("minecraft:confirmation")
        );
        for (key, action) in [("yes", show_action()), ("no", hide_action())] {
            let id = dialog
                .compound(key)
                .and_then(|button| button.compound("action"))
                .and_then(|action| action.string("id"))
                .map(ToString::to_string);
            assert_eq!(id, Some(action.to_string()));
        }
    }
}
