//! `/stopwatch` and `execute if|unless stopwatch`.

use super::scoreboard_command::{run, run_reported, with_server_world};
use crate::command::sender::CommandSender;

#[test]
fn stopwatches_are_created_queried_restarted_and_removed() {
    with_server_world("stopwatch_command", |server, _| {
        assert_eq!(run(server, "stopwatch create demo:race"), Some(1));
        assert_eq!(run(server, "stopwatch create demo:race"), None);
        // Under a second has passed, so whole seconds scale to zero.
        assert_eq!(run(server, "stopwatch query demo:race"), Some(0));
        assert_eq!(run(server, "stopwatch query demo:race 0"), Some(0));
        assert_eq!(run(server, "stopwatch query demo:missing"), None);
        assert_eq!(run(server, "stopwatch restart demo:race"), Some(1));
        assert_eq!(run(server, "stopwatch restart demo:missing"), None);
        assert_eq!(run(server, "stopwatch remove demo:race"), Some(1));
        assert_eq!(run(server, "stopwatch remove demo:race"), None);
        assert_eq!(run(server, "stopwatch query demo:race"), None);
    });
}

#[test]
fn execute_tests_a_stopwatch_against_a_range_of_seconds() {
    with_server_world("stopwatch_execute", |server, _| {
        assert_eq!(run(server, "stopwatch create race"), Some(1));
        let passes = |condition: &str| {
            run_reported(
                server,
                CommandSender::Console,
                &format!("execute {condition} run return 1"),
            )
            .is_some()
        };

        assert!(passes("if stopwatch race 0.."));
        assert!(passes("if stopwatch minecraft:race ..60"));
        assert!(!passes("if stopwatch race 60.."));
        assert!(passes("unless stopwatch race 60.."));
        assert_eq!(run(server, "execute if stopwatch race ..1000"), Some(1));
        assert_eq!(run(server, "execute unless stopwatch race ..1000"), None);
        // An unknown stopwatch is an error.
        assert_eq!(run(server, "execute if stopwatch missing 0.."), None);
    });
}
