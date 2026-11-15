//! `/help` run through the server's dispatcher, printing vanilla's
//! `getSmartUsage` lines to the source that asked.

use std::sync::Arc;

use steel_registry::packets::play::C_SYSTEM_CHAT;
use steel_utils::text::DisplayResolutor;

use super::recipe_command::join_with_packets;
use super::scoreboard_command::{run, run_as, with_server_world};
use super::{chat_packets, decode_system_chat};
use crate::command::sender::CommandSender;

#[test]
fn help_lists_the_commands_the_player_can_use() {
    with_server_world("help_player", |server, world| {
        let (player, packets) = join_with_packets(server, world, "Alex", 1);
        let result = run_as(server, CommandSender::Player(Arc::clone(&player)), "help");
        let lines: Vec<String> = chat_packets(&packets, C_SYSTEM_CHAT)
            .iter()
            .map(|packet| decode_system_chat(packet).to_plain(&DisplayResolutor))
            .collect();

        assert_eq!(result, Some(i32::try_from(lines.len()).expect("few lines")));
        assert!(lines.contains(&"/help [<command>]".to_owned()), "{lines:?}");
        assert!(lines.contains(&"/list [uuids]".to_owned()), "{lines:?}");
        // Operator commands are not usable, so they get no line.
        assert!(
            !lines.iter().any(|line| line.starts_with("/stop")),
            "{lines:?}"
        );
        assert!(
            !lines.iter().any(|line| line.starts_with("/op ")),
            "{lines:?}"
        );
    });
}

#[test]
fn help_for_a_command_lists_its_branches() {
    with_server_world("help_command", |server, world| {
        let (player, packets) = join_with_packets(server, world, "Alex", 1);
        let sender = || CommandSender::Player(Arc::clone(&player));

        assert_eq!(run_as(server, sender(), "help list"), Some(1));
        let lines: Vec<String> = chat_packets(&packets, C_SYSTEM_CHAT)
            .iter()
            .map(|packet| decode_system_chat(packet).to_plain(&DisplayResolutor))
            .collect();
        assert_eq!(lines, ["/list [uuids]"]);

        // Text that matches no command fails the command.
        assert_eq!(run_as(server, sender(), "help nosuchcommand"), None);
        // The console can use everything, operator commands included.
        assert!(run(server, "help stop").is_some());
    });
}
