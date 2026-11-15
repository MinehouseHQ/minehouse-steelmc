//! `/place` run through the server's dispatcher: features and templates go
//! straight into the loaded world, as vanilla's `PlaceCommand` places them
//! into the `ServerLevel`.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::thread;

use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::{init_vanilla_registry, vanilla_blocks};
use steel_utils::threading::DEBUG_STACK_SIZE;
use steel_utils::{BlockPos, ChunkPos, Identifier};

use super::insert_ready_full_chunk;
use super::scoreboard_command::{run, with_server_world};
use crate::behavior::init_behaviors;
use crate::block_entity::init_block_entities;
use crate::entity::init_entities;
use crate::server::Server;
use crate::world::World;
use crate::worldgen::generator::{ChunkGenerator as _, StructureStartLevel};

fn init() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
}

/// Runs `test` on a thread with the stack Steel's own threads get: structure
/// generation overflows the default test stack in debug builds.
fn with_structure_world(
    name: &'static str,
    test: impl FnOnce(&Arc<Server>, &Arc<World>) + Send + 'static,
) {
    thread::Builder::new()
        .stack_size(DEBUG_STACK_SIZE)
        .spawn(move || with_server_world(name, test))
        .expect("test thread should spawn")
        .join()
        .expect("test thread panicked");
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

#[test]
fn a_structure_generates_outside_its_biomes() {
    init();
    with_structure_world("place_structure", |server, world| {
        load_around_origin(world);
        assert_eq!(
            run(server, "fill -16 63 -16 31 63 31 minecraft:stone"),
            Some(48 * 48)
        );

        // The void world's biome is plains; vanilla's `/place structure` skips
        // the biome check, so a swamp hut still starts here.
        assert_eq!(
            run(server, "place structure minecraft:swamp_hut 8 64 8"),
            Some(1)
        );
        let planks = blocks_in(
            world,
            BlockPos::new(-16, 64, -16),
            BlockPos::new(31, 80, 31),
            &vanilla_blocks::SPRUCE_PLANKS,
        );
        assert!(!planks.is_empty());
        let cauldrons = blocks_in(
            world,
            BlockPos::new(-16, 64, -16),
            BlockPos::new(31, 80, 31),
            &vanilla_blocks::CAULDRON,
        );
        assert_eq!(cauldrons.len(), 1);
    });
}

#[test]
fn bad_structures_and_unloaded_starts_fail() {
    init();
    with_structure_world("place_structure_failures", |server, world| {
        load_around_origin(world);
        assert_eq!(
            run(server, "place structure minecraft:no_such_structure 8 64 8"),
            None
        );
        // A mineshaft spreads far past the loaded chunks around its start, so
        // vanilla refuses it before placing anything.
        let start = world
            .chunk_map
            .world_gen_context
            .generator
            .generate_structure_start(
                &Identifier::vanilla_static("mineshaft"),
                ChunkPos::new(0, 0),
                StructureStartLevel {
                    seed: world.seed(),
                    min_y: world.get_min_y(),
                    height: world.get_height(),
                    sea_level: world.sea_level,
                },
            )
            .expect("a mineshaft starts in any chunk");
        let bounds = start.bounding_box.expect("a valid start has a box");
        assert!(bounds.min_x() < -16 || bounds.max_x() > 31);
        assert_eq!(
            run(server, "place structure minecraft:mineshaft 8 64 8"),
            None
        );
        let min = BlockPos::new(-16, world.get_min_y(), -16);
        let max = BlockPos::new(31, world.get_min_y() + world.get_height() - 1, 31);
        assert!(blocks_in(world, min, max, &vanilla_blocks::OAK_PLANKS).is_empty());
        assert!(blocks_in(world, min, max, &vanilla_blocks::RAIL).is_empty());
    });
}

#[test]
fn a_jigsaw_pool_places_from_its_named_jigsaw() {
    init();
    with_structure_world("place_jigsaw", |server, world| {
        load_around_origin(world);
        assert_eq!(
            run(server, "fill -8 63 -8 23 63 23 minecraft:grass_block"),
            Some(32 * 32)
        );

        // A feature element's only jigsaw is vanilla's synthetic `bottom` one, and
        // its ground level delta of 1 sinks the start a block below the position.
        assert_eq!(
            run(
                server,
                "place jigsaw minecraft:abandoned_camp/trees/oak minecraft:bottom 1 8 65 8"
            ),
            Some(1)
        );
        assert!(is(world, BlockPos::new(8, 64, 8), &vanilla_blocks::OAK_LOG));

        assert_eq!(
            run(
                server,
                "place jigsaw minecraft:abandoned_camp/trees/oak minecraft:no_such_jigsaw 1 0 64 0"
            ),
            None
        );
        assert_eq!(
            run(
                server,
                "place jigsaw minecraft:no_such_pool minecraft:bottom 1 0 64 0"
            ),
            None
        );
    });
}
