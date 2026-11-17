//! `/fetchprofile entity`: only players carry a profile.

use steel_registry::init_vanilla_registry;
use steel_utils::ChunkPos;

use super::insert_ready_full_chunk;
use super::recipe_command::join_with_packets;
use super::scoreboard_command::{run, with_server_world};
use crate::behavior::init_behaviors;
use crate::entity::init_entities;

#[test]
fn fetchprofile_entity_reports_players_and_rejects_other_entities() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    with_server_world("fetchprofile_entity", |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let _alex = join_with_packets(server, world, "Alex", 10_000);
        assert_eq!(run(server, "fetchprofile entity Alex"), Some(1));

        assert_eq!(
            run(server, "summon minecraft:experience_orb 0.5 65 0.5"),
            Some(1)
        );
        assert_eq!(
            run(
                server,
                "fetchprofile entity @e[type=minecraft:experience_orb,limit=1]"
            ),
            None
        );
    });
}
