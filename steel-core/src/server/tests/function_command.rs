//! Data pack functions loaded from a pack folder and run through `/function`
//! and the game loop's `#minecraft:load` and `#minecraft:tick` tags.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{env, fs, process};

use steel_utils::Identifier;

use super::scoreboard_command::{run, with_server_world};
use crate::command::PendingCommandExecutionQueue;
use crate::datapack::find_packs;
use crate::server::Server;
use crate::world::World;

/// A `datapacks` folder with one pack holding `files`, by path under `data/`.
fn datapacks(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = env::temp_dir().join(format!("steel-function-command-{name}-{}", process::id()));
    let _ = fs::remove_dir_all(&root);
    let pack = root.join("pack");
    write(&pack.join("pack.mcmeta"), "{}");
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
