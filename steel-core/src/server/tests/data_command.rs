//! `/data` on live entities, run through the server's dispatcher.

use steel_registry::init_vanilla_registry;
use steel_utils::ChunkPos;

use super::insert_ready_full_chunk;
use super::scoreboard_command::{run, with_server_world};
use crate::behavior::init_behaviors;
use crate::entity::init_entities;

#[test]
fn merging_into_a_live_entity_moves_it_through_the_world() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    with_server_world("data_merge_live_entity", |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let orb = "@e[type=minecraft:experience_orb,limit=1]";
        assert_eq!(
            run(server, "summon minecraft:experience_orb 0.5 65 0.5"),
            Some(1)
        );

        // The merged data carries the orb's own position, which used to
        // bypass the world's entity manager and panic the tick.
        assert_eq!(
            run(server, &format!("data merge entity {orb} {{Value:5s}}")),
            Some(1)
        );
        assert_eq!(
            run(server, &format!("data get entity {orb} Value")),
            Some(5)
        );

        assert_eq!(
            run(
                server,
                &format!("data merge entity {orb} {{Pos:[3.5d,65.0d,0.5d]}}")
            ),
            Some(1)
        );
        assert_eq!(
            run(server, &format!("data get entity {orb} Pos[0]")),
            Some(3)
        );
    });
}
