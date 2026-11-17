//! `/posteffect` and the post effect list it changes, run through the
//! server's dispatcher the way vanilla's `PostEffectCommand` reports results.

use steel_registry::packets::play::C_POST_EFFECTS;
use steel_utils::Identifier;

use super::chat_packets;
use super::recipe_command::join_with_packets;
use super::scoreboard_command::{run, with_server_world};

#[test]
fn post_effects_are_added_listed_removed_and_cleared_like_vanilla() {
    with_server_world("posteffect", |server, world| {
        let (alex, packets) = join_with_packets(server, world, "Alex", 1);
        let (steve, _) = join_with_packets(server, world, "Steve", 2);
        let creeper = Identifier::vanilla_static("creeper");
        let spider = Identifier::vanilla_static("spider");

        assert_eq!(run(server, "posteffect list Alex"), Some(0));
        assert_eq!(run(server, "posteffect add @a minecraft:creeper"), Some(2));
        // Applied effects are not added twice, so vanilla fails the command.
        assert_eq!(run(server, "posteffect add @a minecraft:creeper"), None);
        assert_eq!(run(server, "posteffect add Alex spider"), Some(1));
        assert_eq!(alex.post_effects(), [creeper.clone(), spider]);
        assert_eq!(run(server, "posteffect list Alex"), Some(2));

        // Vanilla `ServerPlayer.tick` resends the list once it changed.
        let sent = chat_packets(&packets, C_POST_EFFECTS).len();
        alex.send_dirty_post_effects();
        assert_eq!(chat_packets(&packets, C_POST_EFFECTS).len(), sent + 1);
        alex.send_dirty_post_effects();
        assert_eq!(chat_packets(&packets, C_POST_EFFECTS).len(), sent + 1);

        assert_eq!(
            run(server, "posteffect remove @a minecraft:spider"),
            Some(1)
        );
        assert_eq!(run(server, "posteffect remove Alex minecraft:spider"), None);
        assert_eq!(alex.post_effects(), [creeper]);

        assert_eq!(run(server, "posteffect clear @a"), Some(2));
        assert_eq!(run(server, "posteffect clear @a"), None);
        assert_eq!(alex.post_effects(), []);
        assert_eq!(steve.post_effects(), []);
    });
}
