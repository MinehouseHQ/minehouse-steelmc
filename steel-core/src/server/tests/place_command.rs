//! `/place` run through the server's dispatcher: features and templates go
//! straight into the loaded world, as vanilla's `PlaceCommand` places them
//! into the `ServerLevel`.

use std::collections::BTreeSet;
use std::sync::Arc;

use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::{init_vanilla_registry, vanilla_blocks};
use steel_utils::{BlockPos, ChunkPos};

use super::insert_ready_full_chunk;
use super::scoreboard_command::{run, with_server_world};
use crate::behavior::init_behaviors;
use crate::block_entity::init_block_entities;
use crate::entity::init_entities;
use crate::world::World;

fn init() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
}

/// Loads the chunks around chunk `(0, 0)`.
fn load_neighbors(world: &Arc<World>) {
    for chunk_x in -1..=1 {
        for chunk_z in -1..=1 {
            if (chunk_x, chunk_z) != (0, 0) {
                insert_ready_full_chunk(world, ChunkPos::new(chunk_x, chunk_z));
            }
        }
    }
}

/// Loads the chunks from `(-1, -1)` to `(1, 1)`.
fn load_around_origin(world: &Arc<World>) {
    insert_ready_full_chunk(world, ChunkPos::new(0, 0));
    load_neighbors(world);
}

fn is(world: &World, pos: BlockPos, block: BlockRef) -> bool {
    world.get_block_state(pos).get_block() == block
}

/// The positions of `block` in the box from `min` to `max`, relative to `min`.
fn blocks_in(
    world: &World,
    min: BlockPos,
    max: BlockPos,
    block: BlockRef,
) -> BTreeSet<(i32, i32, i32)> {
    let mut found = BTreeSet::new();
    for x in min.x()..=max.x() {
        for y in min.y()..=max.y() {
            for z in min.z()..=max.z() {
                if is(world, BlockPos::new(x, y, z), block) {
                    found.insert((x - min.x(), y - min.y(), z - min.z()));
                }
            }
        }
    }
    found
}

#[test]
fn a_feature_lands_in_the_world() {
    init();
    with_server_world("place_feature", |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        // Vanilla needs the chunks around the origin loaded too.
        assert_eq!(
            run(server, "place feature minecraft:end_platform 8 64 8"),
            None
        );

        load_neighbors(world);
        assert_eq!(
            run(server, "place feature minecraft:end_platform 8 64 8"),
            Some(1)
        );
        // The 5×5 obsidian floor under the origin, air above it.
        for x in 6..=10 {
            for z in 6..=10 {
                assert!(is(
                    world,
                    BlockPos::new(x, 63, z),
                    &vanilla_blocks::OBSIDIAN
                ));
            }
        }
        assert!(is(world, BlockPos::new(5, 63, 8), &vanilla_blocks::AIR));
        assert!(is(world, BlockPos::new(8, 64, 8), &vanilla_blocks::AIR));
    });
}

#[test]
fn ores_replace_stone_through_section_access() {
    init();
    with_server_world("place_ore", |server, world| {
        load_around_origin(world);
        assert_eq!(
            run(server, "fill 0 0 0 15 31 15 minecraft:stone"),
            Some(16 * 32 * 16)
        );

        assert_eq!(
            run(server, "place feature minecraft:ore_diamond_large 8 16 8"),
            Some(1)
        );
        let ores = blocks_in(
            world,
            BlockPos::new(0, 0, 0),
            BlockPos::new(15, 31, 15),
            &vanilla_blocks::DIAMOND_ORE,
        );
        assert!(!ores.is_empty());
        // Ore only replaces stone: the air around the filled box stays air.
        assert!(
            blocks_in(
                world,
                BlockPos::new(-8, 32, -8),
                BlockPos::new(23, 40, 23),
                &vanilla_blocks::DIAMOND_ORE,
            )
            .is_empty()
        );
    });
}

#[test]
fn a_template_places_rotated_and_rotted() {
    init();
    with_server_world("place_template", |server, world| {
        load_around_origin(world);
        let bone = &vanilla_blocks::BONE_BLOCK;

        assert_eq!(
            run(server, "place template minecraft:fossil/skull_4 0 64 0"),
            Some(1)
        );
        let whole = blocks_in(
            world,
            BlockPos::new(0, 64, 0),
            BlockPos::new(3, 67, 3),
            bone,
        );
        assert!(whole.len() > 8);

        // A half turn around the origin corner puts the template at -3..0.
        assert_eq!(
            run(
                server,
                "place template minecraft:fossil/skull_4 -4 64 -4 180"
            ),
            Some(1)
        );
        let turned = blocks_in(
            world,
            BlockPos::new(-7, 64, -7),
            BlockPos::new(-4, 67, -4),
            bone,
        );
        let expected = whole
            .iter()
            .map(|&(x, y, z)| (3 - x, y, 3 - z))
            .collect::<BTreeSet<_>>();
        assert_eq!(turned, expected);

        // Half integrity with one seed rots the same blocks every time.
        for corner in [(0, 8), (8, 8)] {
            let command = format!(
                "place template minecraft:fossil/skull_4 {} 64 {} none none 0.5 7",
                corner.0, corner.1
            );
            assert_eq!(run(server, &command), Some(1));
        }
        let rotted = blocks_in(
            world,
            BlockPos::new(0, 64, 8),
            BlockPos::new(3, 67, 11),
            bone,
        );
        let again = blocks_in(
            world,
            BlockPos::new(8, 64, 8),
            BlockPos::new(11, 67, 11),
            bone,
        );
        assert_eq!(rotted, again);
        assert!(rotted.is_subset(&whole));
        assert!(rotted.len() < whole.len());

        // No integrity places nothing but still succeeds.
        assert_eq!(
            run(
                server,
                "place template minecraft:fossil/skull_4 -8 64 8 none none 0"
            ),
            Some(1)
        );
        assert!(
            blocks_in(
                world,
                BlockPos::new(-8, 64, 8),
                BlockPos::new(-5, 67, 11),
                bone,
            )
            .is_empty()
        );
    });
}

#[test]
fn bad_templates_and_unloaded_areas_fail() {
    init();
    with_server_world("place_template_failures", |server, world| {
        load_around_origin(world);
        assert_eq!(
            run(server, "place template minecraft:no_such_template 0 64 0"),
            None
        );
        // The template's far corner reaches the unloaded chunk at x 32.
        assert_eq!(
            run(server, "place template minecraft:fossil/skull_4 30 64 0"),
            None
        );
        assert!(is(world, BlockPos::new(30, 64, 0), &vanilla_blocks::AIR));
    });
}
