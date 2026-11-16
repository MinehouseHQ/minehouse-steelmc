//! `/spectate` and the spectator camera it sets.

use steel_registry::packets::play::C_SET_CAMERA;

use super::chat_packets;
use super::recipe_command::join_with_packets;
use super::scoreboard_command::{run, with_server_world};
use crate::entity::Entity as _;

#[test]
fn a_spectator_watches_through_the_target_until_stopped() {
    with_server_world("spectate_command", |server, world| {
        let (alex, packets) = join_with_packets(server, world, "Alex", 1);
        let (steve, _) = join_with_packets(server, world, "Steve", 2);

        // Only spectators can spectate, and never themselves.
        assert_eq!(run(server, "spectate Steve Alex"), None);
        assert_eq!(run(server, "gamemode spectator Alex"), Some(1));
        assert_eq!(run(server, "spectate Alex Alex"), None);

        assert_eq!(run(server, "spectate Steve Alex"), Some(1));
        assert_eq!(alex.camera_id(), steve.id());
        assert!(!alex.is_own_camera());
        assert!(!chat_packets(&packets, C_SET_CAMERA).is_empty());
        assert!(
            (alex.position() - steve.position()).length() < 1e-9,
            "the spectator moves to the camera"
        );

        assert_eq!(run(server, "execute as Alex run spectate"), Some(1));
        assert!(alex.is_own_camera());
        assert_eq!(alex.camera_id(), alex.id());
    });
}

#[test]
fn leaving_spectator_mode_returns_the_camera() {
    with_server_world("spectate_gamemode", |server, world| {
        let (alex, _) = join_with_packets(server, world, "Alex", 1);
        let (steve, _) = join_with_packets(server, world, "Steve", 2);
        assert_eq!(run(server, "gamemode spectator Alex"), Some(1));
        assert_eq!(run(server, "spectate Steve Alex"), Some(1));
        assert_eq!(alex.camera_id(), steve.id());

        // A spectator watching someone is hidden from other spectators.
        assert_eq!(run(server, "gamemode spectator Steve"), Some(1));
        assert!(!alex.broadcast_to_player(&steve));

        assert_eq!(run(server, "gamemode creative Alex"), Some(1));
        assert!(alex.is_own_camera());
    });
}
