//! `/scoreboard` run through the server's dispatcher against a domain
//! scoreboard, checking results the way vanilla's command returns them.

use std::sync::Arc;

use steel_utils::locks::SyncMutex;
use tokio::runtime::Builder;

use super::super::Server;
use super::{fresh_test_world, test_server};
use crate::command::execution::{
    CommandExecutionContext, CommandResultCallback, CommandSource, ExecutionCommandSource as _,
    ExecutionStop,
};
use crate::command::sender::CommandSender;
use crate::permission::PermissionSubjectIndex;
use crate::scoreboard::{DisplaySlot, ScoreHolder, Scoreboard, ScoreboardObjective};

/// Runs `command` from the console and returns its result, or `None` when
/// the command failed.
fn run(server: &Arc<Server>, command: &str) -> Option<i32> {
    let outcome = Arc::new(SyncMutex::new(None));
    let callback = {
        let outcome = Arc::clone(&outcome);
        CommandResultCallback::new(move |success, result| {
            *outcome.lock() = Some(success.then_some(result));
        })
    };
    let source =
        CommandSource::new(CommandSender::Console, Arc::clone(server)).with_callback(callback);
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
    assert!(matches!(execution.run(), ExecutionStop::Completed));
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
        test(&server, world.domain());
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
