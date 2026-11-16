//! Data pack functions loaded from a pack folder and run through `/function`
//! and the game loop's `#minecraft:load` and `#minecraft:tick` tags.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{env, fs, process};

use steel_utils::Identifier;

use super::scoreboard_command::{run, run_reported, with_server_world};
use crate::command::PendingCommandExecutionQueue;
use crate::command::execution::CommandSource;
use crate::command::sender::CommandSender;
use crate::datapack::find_packs;
use crate::server::Server;
use crate::world::World;

/// A `datapacks` folder with one pack holding `files`, by path under `data/`.
fn datapacks(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = env::temp_dir().join(format!("steel-function-command-{name}-{}", process::id()));
    let _ = fs::remove_dir_all(&root);
    let pack = root.join("pack");
    write(
        &pack.join("pack.mcmeta"),
        r#"{"pack":{"description":"","min_format":121,"max_format":121}}"#,
    );
    for (path, contents) in files {
        write(&pack.join("data").join(path), contents);
    }
    root
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("file has a parent")).expect("dirs");
    fs::write(path, contents).expect("file should be written");
}

/// Loads the pack folder into the world's domain, as server start does.
fn load(server: &Arc<Server>, world: &Arc<World>, root: &Path) {
    server.load_functions(world.domain(), world, &find_packs(root));
    let _ = fs::remove_dir_all(root);
}

fn has_function(server: &Server, world: &World, id: &str) -> bool {
    let id = id.parse::<Identifier>().expect("valid id");
    server
        .functions
        .library(world.domain())
        .is_some_and(|library| library.function(&id).is_some())
}

#[test]
fn a_function_runs_its_lines_and_returns() {
    with_server_world("function_runs", |server, world| {
        let root = datapacks(
            "runs",
            &[
                (
                    "demo/function/count.mcfunction",
                    "# three lines, a comment and a blank\n\
                     scoreboard players add n c 1\n\
                     \n\
                     scoreboard players add n c 2\n\
                     scoreboard players \\\n    add n c 3\n\
                     return run scoreboard players get n c",
                ),
                ("demo/function/five.mcfunction", "return 5\nsay unreachable"),
                (
                    "demo/function/outer.mcfunction",
                    "return run function demo:five\nreturn 1",
                ),
            ],
        );
        load(server, world, &root);
        assert_eq!(run(server, "scoreboard objectives add c dummy"), Some(1));

        assert_eq!(run(server, "function demo:count"), Some(6));
        assert_eq!(run(server, "function demo:count"), Some(12));
        assert_eq!(run(server, "function demo:five"), Some(5));
        // `return run function` returns the called function's value.
        assert_eq!(run(server, "function demo:outer"), Some(5));
        assert_eq!(run(server, "function demo:missing"), None);
    });
}

#[test]
fn macros_take_inline_and_stored_arguments() {
    with_server_world("function_macros", |server, world| {
        let root = datapacks(
            "macros",
            &[(
                "demo/function/echo.mcfunction",
                "scoreboard players set last c 0\n$return $(value)",
            )],
        );
        load(server, world, &root);
        assert_eq!(run(server, "scoreboard objectives add c dummy"), Some(1));

        assert_eq!(run(server, "function demo:echo {value: 7}"), Some(7));
        assert_eq!(run(server, "function demo:echo {value: 7b}"), Some(7));
        // A macro needs its arguments.
        assert_eq!(run(server, "function demo:echo"), None);
        assert_eq!(run(server, "function demo:echo {other: 1}"), None);
        // A value that makes the line unparsable fails to instantiate.
        assert_eq!(run(server, "function demo:echo {value: \"x\"}"), None);

        assert_eq!(
            run(
                server,
                "data modify storage demo:args nested set value {value: 9}"
            ),
            Some(1)
        );
        assert_eq!(
            run(server, "function demo:echo with storage demo:args nested"),
            Some(9)
        );
        // The path must name a compound.
        assert_eq!(
            run(
                server,
                "function demo:echo with storage demo:args nested.value"
            ),
            None
        );
    });
}

#[test]
fn tags_run_every_function_and_sum_their_results() {
    with_server_world("function_tags", |server, world| {
        let root = datapacks(
            "tags",
            &[
                ("demo/function/five.mcfunction", "return 5"),
                ("demo/function/seven.mcfunction", "return 7"),
                (
                    "demo/tags/function/both.json",
                    r##"{"values": ["demo:five", "#demo:seven_only", "demo:five"]}"##,
                ),
                (
                    "demo/tags/function/seven_only.json",
                    r#"{"values": ["demo:seven", {"id": "demo:absent", "required": false}]}"#,
                ),
                (
                    "demo/tags/function/broken.json",
                    r#"{"values": ["demo:absent"]}"#,
                ),
            ],
        );
        load(server, world, &root);

        // Each function once, in tag order; the results add up.
        assert_eq!(run(server, "function #demo:both"), Some(12));
        // A tag missing a required function is left out, so it runs nothing.
        assert_eq!(run(server, "function #demo:broken"), None);
        assert_eq!(run(server, "function #demo:unknown"), None);
    });
}

#[test]
fn bad_lines_keep_the_function_out_of_the_library() {
    with_server_world("function_errors", |server, world| {
        let root = datapacks(
            "errors",
            &[
                ("demo/function/good.mcfunction", "return 1"),
                ("demo/function/slash.mcfunction", "/say hi"),
                ("demo/function/unknown.mcfunction", "notacommand"),
                ("demo/function/dangling.mcfunction", "say hi \\"),
                ("demo/function/bad_macro.mcfunction", "$say no variables"),
            ],
        );
        load(server, world, &root);

        assert!(has_function(server, world, "demo:good"));
        for id in [
            "demo:slash",
            "demo:unknown",
            "demo:dangling",
            "demo:bad_macro",
        ] {
            assert!(!has_function(server, world, id), "{id} should not load");
        }
    });
}

#[test]
fn load_runs_once_and_tick_runs_every_tick() {
    with_server_world("function_tick", |server, world| {
        let root = datapacks(
            "tick",
            &[
                (
                    "demo/function/setup.mcfunction",
                    "scoreboard players add loads c 1",
                ),
                (
                    "demo/function/tick.mcfunction",
                    "scoreboard players add ticks c 1",
                ),
                (
                    "minecraft/tags/function/load.json",
                    r#"{"values": ["demo:setup"]}"#,
                ),
                (
                    "minecraft/tags/function/tick.json",
                    r#"{"values": ["demo:tick"]}"#,
                ),
            ],
        );
        assert_eq!(run(server, "scoreboard objectives add c dummy"), Some(1));
        load(server, world, &root);

        let mut pending = PendingCommandExecutionQueue::new();
        server.tick_functions(&mut pending, true);
        server.tick_functions(&mut pending, true);
        // A frozen game runs no functions.
        server.tick_functions(&mut pending, false);
        assert_eq!(run(server, "scoreboard players get loads c"), Some(1));
        assert_eq!(run(server, "scoreboard players get ticks c"), Some(2));
    });
}

/// One game tick's time advance and the `/schedule` events it fires.
fn tick_time(server: &Arc<Server>, pending: &mut PendingCommandExecutionQueue<CommandSource>) {
    server.advance_game_times(true);
    server.tick_scheduled_functions(pending, true);
}

#[test]
fn schedule_runs_functions_and_tags_when_their_tick_comes() {
    with_server_world("function_schedule", |server, world| {
        let root = datapacks(
            "schedule",
            &[
                (
                    "demo/function/inc.mcfunction",
                    "scoreboard players add n c 1",
                ),
                (
                    "demo/function/tagged.mcfunction",
                    "scoreboard players add t c 10",
                ),
                ("demo/function/echo.mcfunction", "$return $(value)"),
                (
                    "demo/tags/function/all.json",
                    r#"{"values": ["demo:inc", "demo:tagged"]}"#,
                ),
            ],
        );
        load(server, world, &root);
        assert_eq!(run(server, "scoreboard objectives add c dummy"), Some(1));
        let start = world.game_time();
        let result = |delay: i64| i32::try_from(start + delay).expect("small game time");

        // Zero ticks, a macro and an unknown function are refused.
        assert_eq!(run(server, "schedule function demo:inc 0"), None);
        assert_eq!(run(server, "schedule function demo:echo 1"), None);
        assert_eq!(run(server, "schedule function demo:missing 1"), None);

        // `append` keeps earlier events; the same tick is scheduled once.
        assert_eq!(run(server, "schedule function demo:inc 1"), Some(result(1)));
        assert_eq!(
            run(server, "schedule function demo:inc 1 append"),
            Some(result(1))
        );
        assert_eq!(
            run(server, "schedule function demo:inc 2t append"),
            Some(result(2))
        );
        assert_eq!(
            run(server, "schedule function #demo:all 2"),
            Some(result(2))
        );
        // An unknown tag is empty, not an error.
        assert_eq!(
            run(server, "schedule function #demo:none 1"),
            Some(result(1))
        );

        let mut pending = PendingCommandExecutionQueue::new();
        tick_time(server, &mut pending);
        assert_eq!(run(server, "scoreboard players get n c"), Some(1));
        tick_time(server, &mut pending);
        assert_eq!(run(server, "scoreboard players get n c"), Some(3));
        assert_eq!(run(server, "scoreboard players get t c"), Some(10));
        tick_time(server, &mut pending);
        assert_eq!(run(server, "scoreboard players get n c"), Some(3));
    });
}

#[test]
fn schedule_replace_and_clear_drop_earlier_events() {
    with_server_world("function_schedule_clear", |server, world| {
        let root = datapacks(
            "schedule_clear",
            &[(
                "demo/function/inc.mcfunction",
                "scoreboard players add n c 1",
            )],
        );
        load(server, world, &root);
        assert_eq!(run(server, "scoreboard objectives add c dummy"), Some(1));

        assert!(run(server, "schedule function demo:inc 1").is_some());
        assert!(run(server, "schedule function demo:inc 3 append").is_some());
        // `replace`, the default, drops both before scheduling.
        assert!(run(server, "schedule function demo:inc 2").is_some());
        assert!(run(server, "schedule function demo:inc 4 append").is_some());
        assert_eq!(run(server, "schedule clear demo:inc"), Some(2));
        assert_eq!(run(server, "schedule clear demo:inc"), None);

        assert!(run(server, "schedule function demo:inc 1").is_some());
        // A tag's events are kept apart from the function's under `#<id>`.
        assert!(run(server, "schedule function #demo:inc 1").is_some());
        assert_eq!(run(server, "schedule clear demo:inc"), Some(1));
        assert!(run(server, "schedule function demo:inc 1").is_some());

        let mut pending = PendingCommandExecutionQueue::new();
        for _ in 0..5 {
            tick_time(server, &mut pending);
        }
        assert_eq!(run(server, "scoreboard players get n c"), Some(1));
    });
}

#[test]
fn execute_if_function_tests_each_source_by_its_functions_result() {
    with_server_world("function_execute_if", |server, world| {
        let root = datapacks(
            "execute_if",
            &[
                ("demo/function/zero.mcfunction", "return 0"),
                ("demo/function/seven.mcfunction", "return 7"),
                // A function that never returns counts as zero.
                (
                    "demo/function/silent.mcfunction",
                    "scoreboard players add calls c 1",
                ),
                (
                    "demo/tags/function/zero_then_seven.json",
                    r#"{"values": ["demo:zero", "demo:seven"]}"#,
                ),
                ("demo/function/echo.mcfunction", "$return $(value)"),
            ],
        );
        load(server, world, &root);
        assert_eq!(run(server, "scoreboard objectives add c dummy"), Some(1));
        let passes = |condition: &str| {
            run_reported(
                server,
                CommandSender::Console,
                &format!("execute {condition} run return 1"),
            )
            .is_some()
        };

        assert!(passes("if function demo:seven"));
        assert!(!passes("if function demo:zero"));
        assert!(passes("unless function demo:zero"));
        assert!(!passes("unless function demo:seven"));
        assert!(!passes("if function demo:silent"));
        assert!(passes("unless function demo:silent"));
        assert_eq!(run(server, "scoreboard players get calls c"), Some(2));
        // The first function to return decides for the whole tag.
        assert!(!passes("if function #demo:zero_then_seven"));
        // No functions at all runs nothing.
        assert!(!passes("if function #demo:none"));
        assert!(!passes("unless function #demo:none"));
        // Like vanilla, a macro that cannot instantiate reports the error but
        // still tests each source, with no functions left to return a value.
        assert!(!passes("if function demo:echo"));
        assert!(passes("unless function demo:echo"));
        assert_eq!(
            run(server, "execute if function demo:seven run return 5"),
            Some(5)
        );
    });
}

#[test]
fn reload_compiles_the_domain_packs_again_and_reruns_load() {
    with_server_world("function_reload", |server, world| {
        let root = datapacks(
            "reload",
            &[
                (
                    "demo/function/setup.mcfunction",
                    "scoreboard players add loads c 1",
                ),
                (
                    "minecraft/tags/function/load.json",
                    r#"{"values": ["demo:setup"]}"#,
                ),
            ],
        );
        assert_eq!(run(server, "scoreboard objectives add c dummy"), Some(1));
        load(server, world, &root);
        let mut pending = PendingCommandExecutionQueue::new();
        server.tick_functions(&mut pending, true);
        assert_eq!(run(server, "scoreboard players get loads c"), Some(1));

        // The test world has no folder, so its packs are gone after a reload.
        assert!(world.level_path.is_none());
        assert_eq!(run(server, "reload"), Some(0));
        assert!(!has_function(server, world, "demo:setup"));
        server.tick_functions(&mut pending, true);
        assert_eq!(run(server, "scoreboard players get loads c"), Some(1));
    });
}
