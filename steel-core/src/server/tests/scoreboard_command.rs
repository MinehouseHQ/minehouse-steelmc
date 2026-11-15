//! `/scoreboard` run through the server's dispatcher against a domain
//! scoreboard, checking results the way vanilla's command returns them.

use std::sync::Arc;

use steel_utils::locks::SyncMutex;
use tokio::runtime::Builder;

use super::super::Server;
use super::{
    chat_packets, fresh_test_world, insert_ready_full_chunk, test_player_with_packets, test_server,
};
use crate::behavior::init_behaviors;
use crate::command::execution::{
    CommandExecutionContext, CommandResultCallback, CommandSource, ExecutionCommandSource as _,
    ExecutionStop,
};
use crate::command::sender::CommandSender;
use crate::entity::damage::DamageSource;
use crate::entity::selector::EntitySelector;
use crate::entity::{Entity as _, LivingEntity as _};
use crate::permission::PermissionSubjectIndex;
use crate::player::{Player, ResetReason};
use crate::scoreboard::{
    DisplaySlot, ScoreHolder, Scoreboard, ScoreboardObjective, TeamColor, TeamProperty,
};
use crate::world::World;
use steel_protocol::packets::game::TeamVisibility;
use steel_registry::packets::play::C_SYSTEM_CHAT;
use steel_registry::vanilla_blocks;
use steel_registry::{vanilla_custom_stats, vanilla_damage_types};
use steel_utils::text::DisplayResolutor;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos};
use text_components::TextComponent;

/// Runs `command` from the console and returns its result, or `None` when
/// the command failed.
fn run(server: &Arc<Server>, command: &str) -> Option<i32> {
    run_as(server, CommandSender::Console, command)
}

/// Runs `command` from `sender` and returns its result, or `None` when the
/// command failed.
fn run_as(server: &Arc<Server>, sender: CommandSender, command: &str) -> Option<i32> {
    let outcome = Arc::new(SyncMutex::new(None));
    let callback = {
        let outcome = Arc::clone(&outcome);
        CommandResultCallback::new(move |success, result| {
            *outcome.lock() = Some(success.then_some(result));
        })
    };
    let source = CommandSource::new(sender, Arc::clone(server)).with_callback(callback);
    let chain = {
        let dispatcher = server.command_dispatcher.read();
        let parse = dispatcher.parse(command, source.clone());
        dispatcher.context_chain(parse)
    };
    let chain = match chain {
        Ok(chain) => chain,
        Err(error) => panic!("{command} should parse: {error}"),
    };
    let mut execution = CommandExecutionContext::for_source(&source);
    execution.queue_initial_command(chain, source, CommandResultCallback::empty());
    // Suspended commands finish on a later poll; these tests never wait on
    // chunk loading, so one poll is enough.
    let mut stop = execution.run();
    if matches!(stop, ExecutionStop::Suspended) {
        stop = execution.poll_suspension();
    }
    assert!(matches!(stop, ExecutionStop::Completed));
    let result = *outcome.lock();
    result.unwrap_or_else(|| panic!("{command} should report a result"))
}

fn scoreboard<'a>(server: &'a Server, domain: &str) -> &'a Scoreboard {
    server
        .scoreboards
        .get(domain)
        .expect("every domain has a scoreboard")
}

fn objective(scoreboard: &Scoreboard, name: &str) -> ScoreboardObjective {
    scoreboard
        .objective(name)
        .unwrap_or_else(|| panic!("objective {name} should exist"))
}

fn score(scoreboard: &Scoreboard, holder: &str, objective_name: &str) -> Option<i32> {
    scoreboard.score(
        &ScoreHolder::new(holder),
        &objective(scoreboard, objective_name),
    )
}

/// Runs `test` against a fresh server with one domain.
fn with_server(name: &'static str, test: impl FnOnce(&Arc<Server>, &str)) {
    with_server_world(name, |server, world| test(server, world.domain()));
}

/// Runs `test` against a fresh server with its only world.
fn with_server_world(name: &'static str, test: impl FnOnce(&Arc<Server>, &Arc<World>)) {
    let world_fixture = fresh_test_world(name);
    let world = &world_fixture.world;
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should initialize");
    runtime.block_on(async {
        let server = test_server(Arc::clone(world), PermissionSubjectIndex::new())
            .await
            .expect("test server should initialize");
        test(&server, world);
        drop(server);
    });
}

#[test]
fn objectives_are_added_listed_displayed_and_removed() {
    with_server("scoreboard_objectives", |server, domain| {
        assert_eq!(run(server, "scoreboard objectives list"), Some(0));
        assert_eq!(
            run(server, "scoreboard objectives add kills dummy"),
            Some(1)
        );
        assert_eq!(
            run(server, "scoreboard objectives add hp health \"Health\""),
            Some(2)
        );
        // Vanilla rejects a second objective with the same name.
        assert_eq!(run(server, "scoreboard objectives add kills dummy"), None);
        assert_eq!(run(server, "scoreboard objectives list"), Some(2));
        let board = scoreboard(server, domain);
        assert!(objective(board, "hp").is_read_only());
        assert_eq!(
            objective(board, "hp").render_type().name(),
            "hearts",
            "health renders as hearts by default"
        );

        assert_eq!(
            run(server, "scoreboard objectives setdisplay sidebar kills"),
            Some(0)
        );
        assert_eq!(
            board
                .display_objective(DisplaySlot::Sidebar)
                .map(|o| o.name().to_owned()),
            Some("kills".to_owned())
        );
        // Setting the same objective again and clearing an empty slot fail.
        assert_eq!(
            run(server, "scoreboard objectives setdisplay sidebar kills"),
            None
        );
        assert_eq!(run(server, "scoreboard objectives setdisplay list"), None);
        assert_eq!(
            run(server, "scoreboard objectives setdisplay sidebar"),
            Some(0)
        );
        assert!(board.display_objective(DisplaySlot::Sidebar).is_none());

        assert_eq!(
            run(
                server,
                "scoreboard objectives modify kills rendertype hearts"
            ),
            Some(0)
        );
        assert_eq!(objective(board, "kills").render_type().name(), "hearts");
        assert_eq!(
            run(
                server,
                "scoreboard objectives modify kills displayautoupdate true"
            ),
            Some(0)
        );
        assert!(objective(board, "kills").display_auto_update());

        // Remove returns how many objectives are left.
        assert_eq!(run(server, "scoreboard objectives remove hp"), Some(1));
        assert!(board.objective("hp").is_none());
        assert_eq!(run(server, "scoreboard objectives remove hp"), None);
    });
}

#[test]
fn player_scores_follow_vanilla_results() {
    with_server("scoreboard_players", |server, domain| {
        assert_eq!(
            run(server, "scoreboard objectives add points dummy"),
            Some(1)
        );
        assert_eq!(run(server, "scoreboard objectives add hp health"), Some(2));
        let board = scoreboard(server, domain);

        // A missing score cannot be read.
        assert_eq!(run(server, "scoreboard players get Alex points"), None);
        assert_eq!(run(server, "scoreboard players set Alex points 5"), Some(5));
        assert_eq!(run(server, "scoreboard players add Alex points 3"), Some(8));
        assert_eq!(
            run(server, "scoreboard players remove Alex points 10"),
            Some(-2)
        );
        assert_eq!(run(server, "scoreboard players get Alex points"), Some(-2));
        assert_eq!(score(board, "Alex", "points"), Some(-2));
        // Adding wraps like Java's int.
        assert_eq!(
            run(server, "scoreboard players set Steve points 2147483647"),
            Some(i32::MAX)
        );
        assert_eq!(
            run(server, "scoreboard players add Steve points 1"),
            Some(i32::MIN)
        );

        // Read-only criteria reject writes and create nothing.
        assert_eq!(run(server, "scoreboard players set Alex hp 3"), None);
        assert_eq!(score(board, "Alex", "hp"), None);

        assert_eq!(run(server, "scoreboard players list"), Some(2));
        assert_eq!(run(server, "scoreboard players list Alex"), Some(1));

        // Resetting one objective keeps the others; resetting all forgets
        // the holder.
        assert_eq!(run(server, "scoreboard players reset Alex points"), Some(1));
        assert_eq!(score(board, "Alex", "points"), None);
        assert_eq!(run(server, "scoreboard players reset Steve"), Some(1));
        assert!(
            board
                .tracked_holders()
                .iter()
                .all(|holder| holder.name() != "Steve")
        );
    });
}

#[test]
fn operations_combine_swap_and_reject_zero_divisors() {
    with_server("scoreboard_operation", |server, domain| {
        assert_eq!(run(server, "scoreboard objectives add a dummy"), Some(1));
        let board = scoreboard(server, domain);
        assert_eq!(run(server, "scoreboard players set x a 7"), Some(7));
        assert_eq!(run(server, "scoreboard players set y a -2"), Some(-2));

        // Flooring division, like vanilla's `Math.floorDiv`.
        assert_eq!(
            run(server, "scoreboard players operation x a /= y a"),
            Some(-4)
        );
        assert_eq!(score(board, "x", "a"), Some(-4));
        assert_eq!(
            run(server, "scoreboard players operation x a >< y a"),
            Some(-2)
        );
        assert_eq!(score(board, "x", "a"), Some(-2));
        assert_eq!(score(board, "y", "a"), Some(-4));

        // A source score that did not exist is created as zero, and the
        // division by it fails.
        assert_eq!(
            run(server, "scoreboard players operation x a %= zero a"),
            None
        );
        assert_eq!(score(board, "zero", "a"), Some(0));
        assert_eq!(score(board, "x", "a"), Some(-2));

        // A missing target counts as zero.
        assert_eq!(
            run(server, "scoreboard players operation fresh a -= x a"),
            Some(2)
        );
    });
}

#[test]
fn enable_unlocks_only_trigger_objectives() {
    with_server("scoreboard_enable", |server, domain| {
        assert_eq!(
            run(server, "scoreboard objectives add vote trigger"),
            Some(1)
        );
        assert_eq!(
            run(server, "scoreboard objectives add plain dummy"),
            Some(2)
        );
        let board = scoreboard(server, domain);

        let locked = |holder: &str| {
            board
                .score_entry(&ScoreHolder::new(holder), &objective(board, "vote"))
                .map(|score| score.is_locked())
        };
        assert_eq!(run(server, "scoreboard players enable Alex vote"), Some(1));
        assert_eq!(locked("Alex"), Some(false));
        // Already enabled: nothing changes, so vanilla fails.
        assert_eq!(run(server, "scoreboard players enable Alex vote"), None);
        // Only trigger objectives can be enabled.
        assert_eq!(run(server, "scoreboard players enable Alex plain"), None);
    });
}

#[test]
fn stat_criteria_follow_awarded_and_reset_stats() {
    with_server_world("scoreboard_stat_criteria", |server, world| {
        assert_eq!(
            run(
                server,
                "scoreboard objectives add jumps minecraft.custom:minecraft.jump"
            ),
            Some(1)
        );
        let (player, _) = test_player_with_packets(server, Arc::clone(world), "Jumper", 1);
        let board = scoreboard(server, world.domain());

        player.award_custom_stat(&vanilla_custom_stats::JUMP);
        player.award_custom_stat_with_count(&vanilla_custom_stats::JUMP, 4);
        assert_eq!(score(board, "Jumper", "jumps"), Some(5));
        // Other stats leave the objective alone.
        player.award_custom_stat(&vanilla_custom_stats::DROP);
        assert_eq!(score(board, "Jumper", "jumps"), Some(5));

        player.reset_custom_stat(&vanilla_custom_stats::JUMP);
        assert_eq!(score(board, "Jumper", "jumps"), None);
    });
}

#[test]
fn read_only_criteria_record_changed_player_values() {
    with_server_world("scoreboard_recorded_criteria", |server, world| {
        assert_eq!(run(server, "scoreboard objectives add hp health"), Some(1));
        assert_eq!(
            run(server, "scoreboard objectives add hunger food"),
            Some(2)
        );
        assert_eq!(run(server, "scoreboard objectives add lvl level"), Some(3));
        let (player, _) = test_player_with_packets(server, Arc::clone(world), "Recorded", 1);
        let board = scoreboard(server, world.domain());

        player.update_recorded_criteria();
        assert_eq!(score(board, "Recorded", "hp"), Some(20));
        assert_eq!(score(board, "Recorded", "hunger"), Some(20));
        assert_eq!(score(board, "Recorded", "lvl"), Some(0));

        // Health and absorption are summed and rounded up.
        player.set_health(7.25);
        player.update_recorded_criteria();
        assert_eq!(score(board, "Recorded", "hp"), Some(8));

        // A value that did not change is not written again, so a score
        // removed meanwhile stays removed until the value changes.
        assert_eq!(
            run(server, "scoreboard players reset Recorded hunger"),
            Some(1)
        );
        player.update_recorded_criteria();
        assert_eq!(score(board, "Recorded", "hunger"), None);
        assert_eq!(score(board, "Recorded", "hp"), Some(8));
    });
}

#[test]
fn kills_count_for_the_killer_and_by_team_color() {
    with_server_world("scoreboard_kill_criteria", |server, world| {
        for command in [
            "scoreboard objectives add kills totalKillCount",
            "scoreboard objectives add pvp playerKillCount",
            "scoreboard objectives add red_kills teamkill.red",
            "scoreboard objectives add by_blue killedByTeam.blue",
        ] {
            assert!(run(server, command).is_some(), "{command} should succeed");
        }
        let board = scoreboard(server, world.domain());
        board.add_team("reds").expect("a new team");
        board.add_team("blues").expect("a new team");
        board
            .modify_team("reds", TeamProperty::Color(Some(TeamColor::Red)))
            .expect("the team exists");
        board
            .modify_team("blues", TeamProperty::Color(Some(TeamColor::Blue)))
            .expect("the team exists");
        let (killer, _) = test_player_with_packets(server, Arc::clone(world), "Killer", 1);
        let (victim, _) = test_player_with_packets(server, Arc::clone(world), "Victim", 2);
        board
            .add_holder_to_team(&ScoreHolder::new("Victim"), "reds")
            .expect("the team exists");
        board
            .add_holder_to_team(&ScoreHolder::new("Killer"), "blues")
            .expect("the team exists");

        let source = DamageSource::environment(&vanilla_damage_types::GENERIC);
        killer.award_kill_score(victim.as_ref(), &source);
        assert_eq!(score(board, "Killer", "kills"), Some(1));
        assert_eq!(score(board, "Killer", "pvp"), Some(1));
        // The killer killed a red player; the victim was killed by a blue.
        assert_eq!(score(board, "Killer", "red_kills"), Some(1));
        assert_eq!(score(board, "Victim", "by_blue"), Some(1));
        assert_eq!(score(board, "Killer", "by_blue"), None);

        // Killing yourself scores nothing.
        killer.award_kill_score(killer.as_ref(), &source);
        assert_eq!(score(board, "Killer", "kills"), Some(1));
    });
}

#[test]
fn teams_are_created_joined_modified_and_emptied() {
    with_server("team_command", |server, domain| {
        assert_eq!(run(server, "team list"), Some(0));
        assert_eq!(run(server, "team add reds \"Red Team\""), Some(1));
        assert_eq!(run(server, "team add reds"), None);
        assert_eq!(run(server, "team add blues"), Some(2));
        let board = scoreboard(server, domain);
        let team = |name: &str| board.team(name).expect("the team exists");

        // Joining counts only holders that changed team.
        // Vanilla parses a single holder token, a name or a selector.
        assert_eq!(run(server, "team join reds Alex"), Some(1));
        assert_eq!(run(server, "team join reds Steve"), Some(1));
        assert_eq!(run(server, "team join blues Alex"), Some(1));
        assert_eq!(
            board.holder_team_name(&ScoreHolder::new("Alex")),
            Some("blues".to_owned())
        );
        assert_eq!(run(server, "team list reds"), Some(1));
        // Joining without members needs an entity source.
        assert_eq!(run(server, "team join reds"), None);

        assert_eq!(run(server, "team modify reds color red"), Some(0));
        assert_eq!(team("reds").color(), Some(TeamColor::Red));
        assert_eq!(run(server, "team modify reds color red"), None);
        assert_eq!(run(server, "team modify reds color reset"), Some(0));
        assert_eq!(run(server, "team modify reds color reset"), None);
        assert_eq!(run(server, "team modify reds friendlyFire false"), Some(0));
        assert!(!team("reds").allows_friendly_fire());
        assert_eq!(run(server, "team modify reds friendlyFire false"), None);
        assert_eq!(
            run(server, "team modify reds nametagVisibility hideForOwnTeam"),
            Some(0)
        );
        assert_eq!(
            team("reds").name_tag_visibility(),
            TeamVisibility::HideForOwnTeam
        );
        assert_eq!(
            run(server, "team modify reds collisionRule pushOwnTeam"),
            Some(0)
        );
        assert_eq!(run(server, "team modify reds prefix \"[R] \""), Some(1));
        assert_eq!(run(server, "team modify missing color red"), None);

        // Leaving counts the holders that were on a team.
        assert_eq!(run(server, "team leave Alex"), Some(1));
        // Vanilla reports zero without failing when nobody left.
        assert_eq!(run(server, "team leave Nobody"), Some(0));
        assert_eq!(run(server, "team empty reds"), Some(1));
        assert_eq!(run(server, "team empty reds"), None);
        assert_eq!(run(server, "team remove reds"), Some(1));
        assert!(board.team("reds").is_none());
    });
}

#[test]
fn trigger_needs_an_enabled_trigger_objective() {
    with_server_world("trigger_command", |server, world| {
        assert_eq!(
            run(server, "scoreboard objectives add vote trigger"),
            Some(1)
        );
        assert_eq!(
            run(server, "scoreboard objectives add plain dummy"),
            Some(2)
        );
        let (player, _) = test_player_with_packets(server, Arc::clone(world), "Voter", 1);
        assert!(server.online_players.insert(Arc::clone(&player)));
        assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));
        let _ = player.mark_joined_world();
        let player_run =
            |command: &str| run_as(server, CommandSender::Player(Arc::clone(&player)), command);
        let board = scoreboard(server, world.domain());

        // Not enabled yet, and not a trigger objective.
        assert_eq!(player_run("trigger vote"), None);
        assert_eq!(player_run("trigger plain"), None);
        // The console is not a player.
        assert_eq!(run(server, "trigger vote"), None);

        assert_eq!(run(server, "scoreboard players enable Voter vote"), Some(1));
        assert_eq!(player_run("trigger vote add 5"), Some(5));
        // Each trigger locks the score again.
        assert_eq!(player_run("trigger vote"), None);
        assert_eq!(run(server, "scoreboard players enable Voter vote"), Some(1));
        assert_eq!(player_run("trigger vote"), Some(6));
        assert_eq!(run(server, "scoreboard players enable Voter vote"), Some(1));
        assert_eq!(player_run("trigger vote set -3"), Some(-3));
        assert_eq!(score(board, "Voter", "vote"), Some(-3));
        assert!(
            board
                .score_entry(&ScoreHolder::new("Voter"), &objective(board, "vote"))
                .is_some_and(|score| score.is_locked())
        );
    });
}

#[test]
fn teams_decide_allies_friendly_fire_and_pushing() {
    with_server_world("team_effects", |server, world| {
        let (alex, _) = test_player_with_packets(server, Arc::clone(world), "Alex", 1);
        let (steve, _) = test_player_with_packets(server, Arc::clone(world), "Steve", 2);
        let (noor, _) = test_player_with_packets(server, Arc::clone(world), "Noor", 3);
        let pushable_by =
            |pusher: &Player, target: &Player| EntitySelector::pushable_by(pusher)(target);

        // Without teams nobody is allied and everyone pushes.
        assert!(!alex.is_allied_to(steve.as_ref()));
        assert!(alex.is_allied_to(alex.as_ref()));
        assert!(alex.can_harm_player(&steve));
        assert!(pushable_by(&alex, &steve));

        assert_eq!(run(server, "team add reds"), Some(1));
        assert_eq!(run(server, "team add blues"), Some(2));
        assert_eq!(run(server, "team join reds Alex"), Some(1));
        assert_eq!(run(server, "team join reds Steve"), Some(1));
        assert_eq!(run(server, "team join blues Noor"), Some(1));
        assert!(alex.is_allied_to(steve.as_ref()));
        assert!(!alex.is_allied_to(noor.as_ref()));

        // Friendly fire is on by default; turning it off spares teammates only.
        assert!(alex.can_harm_player(&steve));
        assert_eq!(run(server, "team modify reds friendlyFire false"), Some(0));
        assert!(!alex.can_harm_player(&steve));
        assert!(alex.can_harm_player(&noor));
        let attack = DamageSource::environment(&vanilla_damage_types::PLAYER_ATTACK)
            .with_causing_entity(Arc::<Player>::clone(&alex));
        assert!(!steve.hurt(world, &attack, 1.0));
        // A team removed and added again under the same name is a new team.
        let old_team = alex.team().expect("Alex is on a team");
        assert_eq!(run(server, "team remove reds"), Some(1));
        assert_eq!(run(server, "team add reds"), Some(2));
        assert_eq!(run(server, "team join reds Alex"), Some(1));
        let new_team = alex.team().expect("Alex is on a team");
        assert!(!old_team.is_same_team(&new_team));
        assert_eq!(run(server, "team join reds Steve"), Some(1));
        assert_eq!(run(server, "team modify reds friendlyFire false"), Some(0));

        // The pvp game rule stops every player attack.
        assert!(run(server, "gamerule pvp false").is_some());
        assert!(!alex.can_harm_player(&noor));
        assert!(run(server, "gamerule pvp true").is_some());

        // Collision rules, as vanilla `EntitySelector.pushableBy` reads them.
        assert!(pushable_by(&alex, &steve));
        assert_eq!(
            run(server, "team modify reds collisionRule pushOwnTeam"),
            Some(0)
        );
        assert!(!pushable_by(&alex, &steve));
        // Vanilla's pushOwnTeam stops pushing inside the team only.
        assert!(pushable_by(&alex, &noor));
        assert!(pushable_by(&noor, &alex));
        assert_eq!(
            run(server, "team modify reds collisionRule pushOtherTeams"),
            Some(0)
        );
        assert!(pushable_by(&alex, &steve));
        assert!(!pushable_by(&alex, &noor));
        assert_eq!(run(server, "team modify reds collisionRule never"), Some(0));
        assert!(!pushable_by(&alex, &steve));
        assert!(!pushable_by(&noor, &alex));
    });
}

#[test]
fn team_names_death_messages_and_team_chat() {
    with_server_world("team_names", |server, world| {
        let mut joined = Vec::new();
        for (id, name) in [(1, "Alex"), (2, "Steve"), (3, "Noor")] {
            let (player, packets) = test_player_with_packets(server, Arc::clone(world), name, id);
            assert!(server.online_players.insert(Arc::clone(&player)));
            assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));
            let _ = player.mark_joined_world();
            joined.push((player, packets));
        }
        let plain = |player: &Player| player.display_name().to_plain(&DisplayResolutor);
        assert_eq!(plain(&joined[0].0), "Alex");

        assert_eq!(run(server, "team add reds"), Some(1));
        assert_eq!(run(server, "team join reds Alex"), Some(1));
        assert_eq!(run(server, "team join reds Steve"), Some(1));
        assert_eq!(run(server, "team modify reds prefix \"[R] \""), Some(1));
        assert_eq!(run(server, "team modify reds suffix \"!\""), Some(1));
        // The display name carries the team prefix and suffix.
        assert_eq!(plain(&joined[0].0), "[R] Alex!");
        assert_eq!(plain(&joined[2].0), "Noor");

        let system_chats = |index: usize| {
            let count = chat_packets(&joined[index].1, C_SYSTEM_CHAT).len();
            joined[index].1.lock().clear();
            count
        };
        let death = |visibility: &str| {
            for index in 0..3 {
                system_chats(index);
            }
            assert_eq!(
                run(
                    server,
                    &format!("team modify reds deathMessageVisibility {visibility}")
                ),
                Some(0)
            );
            joined[0]
                .0
                .broadcast_death_message(world, TextComponent::plain("Alex died"));
            [system_chats(0), system_chats(1), system_chats(2)]
        };
        // Only teammates other than the dead player hear it.
        assert_eq!(death("hideForOtherTeams"), [0, 1, 0]);
        // Everyone off the team hears it.
        assert_eq!(death("hideForOwnTeam"), [0, 0, 1]);
        assert_eq!(death("never"), [0, 0, 0]);
        assert_eq!(death("always"), [1, 1, 1]);

        // Team chat reaches the sender and its teammates.
        let send = |index: usize, command: &str| {
            run_as(
                server,
                CommandSender::Player(Arc::clone(&joined[index].0)),
                command,
            )
        };
        assert_eq!(send(0, "teammsg hello"), Some(2));
        assert_eq!(send(1, "tm hi"), Some(2));
        assert_eq!(send(2, "teammsg alone"), None);
        assert_eq!(run(server, "teammsg console"), None);
    });
}

#[test]
fn spreadplayers_lands_targets_on_safe_ground_apart() {
    init_behaviors();
    with_server_world("spreadplayers", |server, world| {
        for chunk in [(-1, -1), (0, -1), (-1, 0), (0, 0)] {
            insert_ready_full_chunk(world, ChunkPos::new(chunk.0, chunk.1));
        }
        let stone = vanilla_blocks::STONE.default_state();
        for x in -4..=4 {
            for z in -4..=4 {
                assert!(world.set_block(BlockPos::new(x, 64, z), stone, UpdateFlags::UPDATE_ALL,));
            }
        }
        let mut players = Vec::new();
        for (id, name) in [(1, "Alex"), (2, "Steve"), (3, "Noor")] {
            let (player, _) = test_player_with_packets(server, Arc::clone(world), name, id);
            assert!(server.online_players.insert(Arc::clone(&player)));
            assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));
            let _ = player.mark_joined_world();
            players.push(player);
        }

        assert_eq!(run(server, "spreadplayers 0 0 1 4 false @a"), Some(3));
        for player in &players {
            let position = player.position();
            assert_eq!(position.y, 65.0, "{position} should stand on the stone");
            assert!(position.x.abs() <= 4.5 && position.z.abs() <= 4.5);
            assert_eq!(position.x.fract().abs(), 0.5);
        }

        // Respecting teams spreads one position per team.
        assert_eq!(run(server, "team add reds"), Some(1));
        assert_eq!(run(server, "team join reds Alex"), Some(1));
        assert_eq!(run(server, "team join reds Steve"), Some(1));
        assert_eq!(run(server, "spreadplayers 0 0 1 4 true @a"), Some(2));
        assert_eq!(players[0].position(), players[1].position());

        // The height limit can't sit under the world.
        assert_eq!(
            run(server, "spreadplayers 0 0 1 4 under -1000 false @a"),
            None
        );
        // Nothing in reach is safe: no stone above y 60.
        assert_eq!(run(server, "spreadplayers 0 0 1 4 under 60 false @a"), None);
    });
}
