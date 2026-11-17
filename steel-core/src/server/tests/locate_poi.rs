//! `/locate poi`: vanilla `LocateCommand.locatePoi`.

use steel_registry::{init_vanilla_registry, vanilla_blocks};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos};

use super::insert_ready_full_chunk;
use super::scoreboard_command::{run, with_server_world};
use crate::behavior::init_behaviors;
use crate::block_entity::init_block_entities;

#[test]
fn locate_poi_finds_the_closest_point_of_interest_by_type_or_tag() {
    init_vanilla_registry();
    init_behaviors();
    init_block_entities();
    with_server_world("locate_poi", |server, world| {
        for x in 0..4 {
            for z in 0..4 {
                insert_ready_full_chunk(world, ChunkPos::new(x, z));
            }
        }
        assert!(world.set_block(
            BlockPos::new(30, 64, 40),
            vanilla_blocks::BELL.default_state(),
            UpdateFlags::UPDATE_ALL
        ));
        assert!(world.set_block(
            BlockPos::new(60, 70, 0),
            vanilla_blocks::LECTERN.default_state(),
            UpdateFlags::UPDATE_ALL
        ));

        assert_eq!(
            run(
                server,
                "execute positioned 0 64 0 run locate poi minecraft:meeting"
            ),
            Some(50),
            "the result is the horizontal distance to the bell"
        );
        assert_eq!(
            run(
                server,
                "execute positioned 0 64 0 run locate poi #minecraft:acquirable_job_site"
            ),
            Some(60),
            "a tag finds the lectern, a librarian's job site"
        );
        assert_eq!(
            run(
                server,
                "execute positioned 0 64 0 run locate poi minecraft:nether_portal"
            ),
            None,
            "no portal is in range"
        );
    });
}
