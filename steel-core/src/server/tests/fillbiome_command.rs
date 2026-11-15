//! `/fillbiome` run through the server's dispatcher, counting changed
//! quarts the way vanilla's `FillBiomeCommand` does.

use steel_registry::{REGISTRY, RegistryExt as _, init_vanilla_registry, vanilla_game_rules};
use steel_utils::ChunkPos;

use super::insert_ready_full_chunk;
use super::scoreboard_command::{run, with_server_world};
use crate::world::World;

fn biome_key(world: &World, quart: (i32, i32, i32)) -> String {
    let id = world
        .noise_biome_id(quart.0, quart.1, quart.2)
        .expect("the chunk is loaded");
    REGISTRY
        .biomes
        .by_id(usize::from(id))
        .expect("registered biome")
        .key
        .to_string()
}

#[test]
fn quarts_inside_the_quantized_region_change_once() {
    init_vanilla_registry();
    with_server_world("fillbiome_region", |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let before = biome_key(world, (0, 0, 0));
        assert_ne!(before, "minecraft:desert");

        // 0..7 quantizes to 0..4: two quarts on each axis.
        assert_eq!(
            run(server, "fillbiome 0 0 0 7 7 7 minecraft:desert"),
            Some(8)
        );
        assert_eq!(biome_key(world, (0, 0, 0)), "minecraft:desert");
        assert_eq!(biome_key(world, (1, 1, 1)), "minecraft:desert");
        assert_eq!(biome_key(world, (2, 0, 0)), before);
        assert_eq!(biome_key(world, (0, 2, 0)), before);

        // Nothing left to change fails the command.
        assert_eq!(run(server, "fillbiome 0 0 0 7 7 7 minecraft:desert"), None);
    });
}

#[test]
fn a_filter_limits_the_quarts_that_change() {
    init_vanilla_registry();
    with_server_world("fillbiome_filter", |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        assert_eq!(
            run(server, "fillbiome 0 0 0 3 3 3 minecraft:desert"),
            Some(1)
        );

        // Only the desert quart matches.
        assert_eq!(
            run(
                server,
                "fillbiome 0 0 0 7 7 7 minecraft:badlands replace minecraft:desert"
            ),
            Some(1)
        );
        assert_eq!(biome_key(world, (0, 0, 0)), "minecraft:badlands");
        // Tags filter too.
        assert_eq!(
            run(
                server,
                "fillbiome 0 0 0 7 7 7 minecraft:plains replace #minecraft:is_badlands"
            ),
            Some(1)
        );
        assert_eq!(biome_key(world, (0, 0, 0)), "minecraft:plains");
    });
}

#[test]
fn large_or_unloaded_regions_fail() {
    init_vanilla_registry();
    with_server_world("fillbiome_failures", |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        assert!(world.set_game_rule(&vanilla_game_rules::MAX_BLOCK_MODIFICATIONS, 8));
        // 0..4 on each axis is 5×5×5 = 125 blocks.
        assert_eq!(run(server, "fillbiome 0 0 0 7 7 7 minecraft:desert"), None);
        assert!(world.set_game_rule(&vanilla_game_rules::MAX_BLOCK_MODIFICATIONS, 32_768));
        // The far corner's chunk is not loaded.
        assert_eq!(run(server, "fillbiome 0 0 0 20 0 0 minecraft:desert"), None);
    });
}
