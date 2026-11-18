//! `/compute` and `/execute if|unless predicate` against the built-in
//! `predicate`, `context_int_provider` and `context_float_provider` entries
//! and a data pack's own.

use super::function_command::{datapacks, load};
use super::loot_command::parse_error;
use super::scoreboard_command::{run, with_server_world};

#[test]
fn compute_reads_inline_and_registry_int_providers() {
    with_server_world("compute_int", |server, _world| {
        assert_eq!(
            run(
                server,
                "compute default integer minecraft:brewing/uses_default"
            ),
            Some(20)
        );
        assert_eq!(
            run(
                server,
                "compute default integer {type:\"minecraft:add\",inputs:[1,2,3]}"
            ),
            Some(6)
        );
        assert_eq!(
            run(
                server,
                "compute default integer {type:\"mul\",inputs:[-3,4]}"
            ),
            Some(-12)
        );
        assert_eq!(
            run(
                server,
                "compute default integer {type:\"floor_mod\",left:-7,right:3}"
            ),
            Some(2)
        );
        assert_eq!(
            run(
                server,
                "compute default integer {type:\"floor_div\",left:-7,right:2}"
            ),
            Some(-4)
        );
        assert_eq!(
            run(
                server,
                "compute default integer {type:\"pow\",base:2,exponent:10}"
            ),
            Some(1024)
        );
        assert_eq!(
            run(
                server,
                "compute default integer {type:\"uniform\",min:5,max:5}"
            ),
            Some(5)
        );
        // Without a block state the fast-cooking predicate fails.
        assert_eq!(
            run(
                server,
                "compute default integer minecraft:cooking/time_coal"
            ),
            Some(1600)
        );
    });
}

#[test]
fn compute_rejects_arithmetic_errors_and_unknown_ids() {
    with_server_world("compute_errors", |server, _world| {
        assert_eq!(
            run(
                server,
                "compute default integer {type:\"div\",left:1,right:0}"
            ),
            None
        );
        assert_eq!(
            run(
                server,
                "compute default integer {type:\"add\",inputs:[2147483647,1]}"
            ),
            None
        );
        assert_eq!(
            run(
                server,
                "compute default integer {type:\"pow\",base:0,exponent:0}"
            ),
            None
        );
        assert!(
            parse_error(server, "compute default integer demo:missing")
                .contains("argument.resource_or_id.no_such_element")
        );
        assert!(
            parse_error(server, "compute default integer {type:\"no_such_type\"}")
                .contains("failed_to_parse")
        );
        // An inline value may not refer to an entry the registry lacks.
        assert!(
            parse_error(
                server,
                "compute default integer {type:\"abs\",input:\"demo:missing\"}"
            )
            .contains("failed_to_parse")
        );
    });
}

#[test]
fn compute_floats_floor_the_scaled_value() {
    with_server_world("compute_float", |server, _world| {
        assert_eq!(
            run(
                server,
                "compute default float minecraft:cooking/fast_speed_multiplier"
            ),
            Some(2)
        );
        assert_eq!(
            run(
                server,
                "compute default float {type:\"constant\",value:2.5f} 2"
            ),
            Some(5)
        );
        assert_eq!(
            run(
                server,
                "compute default float {type:\"constant\",value:2.5f}"
            ),
            Some(2)
        );
        assert_eq!(
            run(server, "compute default float {type:\"sqrt\",input:-1}"),
            None
        );
        assert_eq!(
            run(
                server,
                "compute default float {type:\"from_int\",input:7} 0.5"
            ),
            Some(3)
        );
    });
}

#[test]
fn providers_read_scores_and_storage() {
    with_server_world("compute_score", |server, _world| {
        assert_eq!(run(server, "scoreboard objectives add c dummy"), Some(1));
        assert_eq!(run(server, "scoreboard players set n c 4"), Some(4));
        assert_eq!(
            run(
                server,
                "compute default integer {type:\"score\",target:{type:\"fixed\",name:\"n\"},score:\"c\"}"
            ),
            Some(4)
        );
        // A missing score falls back.
        assert_eq!(
            run(
                server,
                "compute default integer {type:\"score\",target:{type:\"fixed\",name:\"m\"},score:\"c\",fallback:9}"
            ),
            Some(9)
        );
        assert!(run(server, "data modify storage demo:s v set value 3.7d").is_some());
        assert_eq!(
            run(
                server,
                "compute default integer {type:\"storage\",storage:\"demo:s\",path:\"v\"}"
            ),
            Some(3)
        );
        assert_eq!(
            run(
                server,
                "compute default float {type:\"storage\",storage:\"demo:s\",path:\"v\"} 10"
            ),
            Some(37)
        );
    });
}

#[test]
fn execute_tests_inline_and_registry_predicates() {
    with_server_world("execute_predicate", |server, _world| {
        let inline = "{type:\"int_value_check\",value:5,test:{min:1,max:10}}";
        assert_eq!(
            run(server, &format!("execute if predicate {inline}")),
            Some(1)
        );
        assert_eq!(
            run(server, &format!("execute unless predicate {inline}")),
            None
        );
        // The console has no tool.
        assert_eq!(
            run(server, "execute if predicate minecraft:tool/can_shear"),
            None
        );
        assert_eq!(
            run(
                server,
                "execute if predicate {type:\"inverted\",term:\"minecraft:tool/can_shear\"}"
            ),
            Some(1)
        );
        assert!(
            parse_error(server, "execute if predicate demo:missing")
                .contains("argument.resource_or_id.no_such_element")
        );
    });
}

#[test]
fn data_pack_entries_load_and_bad_ones_are_dropped() {
    with_server_world("predicate_pack", |server, world| {
        let root = datapacks(
            "predicates",
            &[
                ("demo/context_int_provider/seven.json", "7"),
                (
                    "demo/context_int_provider/doubled.json",
                    r#"{"type":"minecraft:mul","inputs":["demo:seven",2]}"#,
                ),
                (
                    "demo/predicate/sometimes.json",
                    r#"{"type":"minecraft:int_value_check","value":"demo:doubled","test":14}"#,
                ),
                (
                    "demo/predicate/broken.json",
                    r#"{"type":"minecraft:inverted","term":"demo:nothing"}"#,
                ),
                (
                    "demo/predicate/loop_a.json",
                    r#"{"type":"minecraft:inverted","term":"demo:loop_b"}"#,
                ),
                (
                    "demo/predicate/loop_b.json",
                    r#"{"type":"minecraft:inverted","term":"demo:loop_a"}"#,
                ),
                (
                    "demo/predicate/tagged.json",
                    r##"{"type":"minecraft:all_of","terms":"#demo:checks"}"##,
                ),
                (
                    "demo/tags/predicate/checks.json",
                    r#"{"values":["demo:sometimes"]}"#,
                ),
                (
                    "demo/function/check.mcfunction",
                    "execute if predicate demo:sometimes run return 3\nreturn 0",
                ),
            ],
        );
        load(server, world, &root);
        assert_eq!(
            run(server, "compute default integer demo:doubled"),
            Some(14)
        );
        assert_eq!(run(server, "execute if predicate demo:sometimes"), Some(1));
        assert_eq!(run(server, "execute if predicate demo:tagged"), Some(1));
        assert_eq!(run(server, "function demo:check"), Some(3));
        // Entries with missing or cyclic references are dropped on load.
        for command in [
            "execute if predicate demo:broken",
            "execute unless predicate demo:broken",
            "execute if predicate demo:loop_a",
        ] {
            assert!(parse_error(server, command).contains("no_such_element"));
        }
        // The built-in entries stay beside the pack's.
        assert_eq!(
            run(
                server,
                "compute default integer minecraft:compostable/always_add_one"
            ),
            Some(1)
        );
    });
}
