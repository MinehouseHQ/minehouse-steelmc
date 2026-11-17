//! The locator bar: waypoint connections and `/waypoint`.

use std::io::Cursor;

use steel_protocol::packets::game::DEFAULT_WAYPOINT_STYLE;
use steel_registry::packets::play::C_WAYPOINT;
use steel_utils::Identifier;
use steel_utils::codec::VarInt;
use steel_utils::locks::SyncMutex;
use steel_utils::serial::ReadFrom as _;

use super::recipe_command::join_with_packets;
use super::scoreboard_command::{run, with_server_world};
use super::{chat_packets, packet_body};
use crate::entity::{Entity as _, LivingEntity as _};
use steel_protocol::packet_traits::EncodedPacket;

/// The operation of every waypoint packet sent so far: 0 track, 1 untrack,
/// 2 update.
fn waypoint_operations(packets: &SyncMutex<Vec<EncodedPacket>>) -> Vec<i32> {
    chat_packets(packets, C_WAYPOINT)
        .iter()
        .map(|packet| {
            let (_, mut body): (i32, Cursor<&[u8]>) = packet_body(packet);
            VarInt::read(&mut body).map_or(-1, |operation| operation.0)
        })
        .collect()
}

#[test]
fn players_see_each_other_until_the_locator_bar_turns_off() {
    with_server_world("waypoint_connections", |server, world| {
        let (alex, packets) = join_with_packets(server, world, "Alex", 1);
        let (steve, _) = join_with_packets(server, world, "Steve", 2);
        alex.set_first_tick(false);
        steve.set_first_tick(false);
        let manager = world.waypoint_manager();
        assert_eq!(manager.transmitters().len(), 2);

        // Nothing is sent while the source is still on its first tick.
        assert_eq!(waypoint_operations(&packets), Vec::<i32>::new());
        manager.update_player(alex.id());
        assert_eq!(waypoint_operations(&packets), vec![0]);

        // A receiver never sees itself.
        manager.update_waypoint(alex.id());
        assert_eq!(waypoint_operations(&packets), vec![0]);

        assert_eq!(run(server, "gamerule locator_bar false"), Some(0));
        assert_eq!(waypoint_operations(&packets), vec![0, 1]);
        manager.update_player(alex.id());
        assert_eq!(waypoint_operations(&packets), vec![0, 1]);

        assert_eq!(run(server, "gamerule locator_bar true"), Some(1));
        assert_eq!(waypoint_operations(&packets), vec![0, 1, 0]);

        // A spectator source is hidden from players who are not spectators.
        assert_eq!(run(server, "gamemode spectator Steve"), Some(1));
        manager.update_waypoint(steve.id());
        assert_eq!(waypoint_operations(&packets), vec![0, 1, 0, 1]);
    });
}

#[test]
fn waypoint_modify_changes_the_icon_and_list_counts_transmitters() {
    with_server_world("waypoint_command", |server, world| {
        let (alex, packets) = join_with_packets(server, world, "Alex", 1);
        let (steve, _) = join_with_packets(server, world, "Steve", 2);
        alex.set_first_tick(false);
        steve.set_first_tick(false);

        assert_eq!(run(server, "waypoint list"), Some(2));

        assert_eq!(run(server, "waypoint modify Steve color red"), Some(0));
        assert_eq!(steve.waypoint_icon().color, Some(0xFF_5555));
        // Re-tracking tells Alex about the new icon.
        assert_eq!(waypoint_operations(&packets), vec![0]);

        assert_eq!(run(server, "waypoint modify Steve color hex 0F0"), Some(0));
        assert_eq!(steve.waypoint_icon().color, Some(0xFF00_FF00_u32 as i32));

        assert_eq!(run(server, "waypoint modify Steve color reset"), Some(0));
        assert_eq!(steve.waypoint_icon().color, None);

        assert_eq!(
            run(server, "waypoint modify Steve style set minecraft:bowtie"),
            Some(0)
        );
        assert_eq!(
            steve.waypoint_icon().style,
            Identifier::vanilla_static("bowtie")
        );
        assert_eq!(run(server, "waypoint modify Steve style reset"), Some(0));
        assert_eq!(steve.waypoint_icon().style, DEFAULT_WAYPOINT_STYLE);
    });
}
