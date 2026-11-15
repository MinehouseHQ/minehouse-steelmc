//! `/dialog`, which shows registry and inline dialogs and clears them.

use std::sync::Arc;

use steel_registry::packets::play::{C_CLEAR_DIALOG, C_SHOW_DIALOG};
use steel_registry::{REGISTRY, RegistryExt as _};
use steel_utils::Identifier;
use steel_utils::codec::VarInt;
use steel_utils::serial::ReadFrom as _;

use super::recipe_command::join_with_packets;
use super::scoreboard_command::{run, with_server_world};
use super::{chat_packets, packet_body};
use crate::command::execution::CommandSource;
use crate::command::sender::CommandSender;

#[test]
fn dialogs_are_shown_by_registry_id_and_cleared() {
    with_server_world("dialog_show_clear", |server, world| {
        let (_alex, alex_packets) = join_with_packets(server, world, "Alex", 1);
        let (_steve, steve_packets) = join_with_packets(server, world, "Steve", 2);

        assert_eq!(
            run(server, "dialog show Alex minecraft:server_links"),
            Some(1)
        );
        let shown = chat_packets(&alex_packets, C_SHOW_DIALOG);
        assert_eq!(shown.len(), 1);
        let (_, mut body) = packet_body(&shown[0]);
        let holder = VarInt::read(&mut body).expect("holder id decodes").0;
        let id = REGISTRY
            .dialogs
            .id_from_key(&Identifier::vanilla_static("server_links"))
            .expect("server_links is registered");
        // A registry holder is its id plus one.
        assert_eq!(usize::try_from(holder - 1).ok(), Some(id));
        assert!(chat_packets(&steve_packets, C_SHOW_DIALOG).is_empty());

        // Unknown and inline dialogs fail to parse.
        for (command, error) in [
            (
                "dialog show Alex minecraft:missing",
                "argument.resource_or_id.no_such_element",
            ),
            (
                "dialog show Alex {type:\"notice\"}",
                "argument.resource_or_id.failed_to_parse",
            ),
            (
                "dialog show Alex \"minecraft:server_links\"",
                "argument.resource_or_id.failed_to_parse",
            ),
        ] {
            let source = CommandSource::new(CommandSender::Console, Arc::clone(server));
            let dispatcher = server.command_dispatcher.read();
            let parse = dispatcher.parse(command, source);
            let Err(failure) = dispatcher.context_chain(parse) else {
                panic!("{command} should fail to parse");
            };
            assert!(failure.to_string().contains(error), "{failure}");
        }

        // An inline dialog is sent as a direct holder: zero, then its NBT.
        assert_eq!(
            run(server, "dialog show @a {type:\"notice\",title:\"Hi\"}"),
            Some(2)
        );
        let shown = chat_packets(&steve_packets, C_SHOW_DIALOG);
        assert_eq!(shown.len(), 1);
        let (_, mut body) = packet_body(&shown[0]);
        assert_eq!(VarInt::read(&mut body).expect("holder id decodes").0, 0);

        assert_eq!(run(server, "dialog clear @a"), Some(2));
        assert_eq!(chat_packets(&alex_packets, C_CLEAR_DIALOG).len(), 1);
        assert_eq!(chat_packets(&steve_packets, C_CLEAR_DIALOG).len(), 1);
    });
}
