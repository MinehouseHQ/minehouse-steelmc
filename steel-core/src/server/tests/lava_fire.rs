//! Lava setting nearby flammable blocks on fire, vanilla
//! `LavaFluid.randomTick`.

use std::sync::Arc;

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_game_rules::FIRE_SPREAD_RADIUS_AROUND_PLAYER;
use steel_registry::{init_vanilla_registry, vanilla_blocks};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos};

use super::insert_ready_full_chunk;
use super::scoreboard_command::with_server_world;
use crate::behavior::{FLUID_BEHAVIORS, init_behaviors};
use crate::block_entity::init_block_entities;
use crate::entity::init_entities;
use crate::fluid::lava_id;
use crate::world::World;

const LAVA: BlockPos = BlockPos::new(8, 64, 8);

fn init() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
}

/// Lava ringed by planks at its own level, with air above.
fn build(world: &Arc<World>) {
    for dx in -2..=2 {
        for dz in -2..=2 {
            let pos = LAVA.offset(dx, -1, dz);
            world.set_block(
                pos,
                vanilla_blocks::STONE.default_state(),
                UpdateFlags::UPDATE_ALL,
            );
            if dx != 0 || dz != 0 {
                world.set_block(
                    pos.above(),
                    vanilla_blocks::OAK_PLANKS.default_state(),
                    UpdateFlags::UPDATE_ALL,
                );
            }
        }
    }
    world.set_block(
        LAVA,
        vanilla_blocks::LAVA.default_state(),
        UpdateFlags::UPDATE_ALL,
    );
}

fn fire_count(world: &World) -> usize {
    let mut count = 0;
    for dx in -2..=2 {
        for dy in 1..=3 {
            for dz in -2..=2 {
                if world.get_block_state(LAVA.offset(dx, dy, dz)).get_block()
                    == &vanilla_blocks::FIRE
                {
                    count += 1;
                }
            }
        }
    }
    count
}

fn random_ticks(world: &Arc<World>, times: usize) {
    let lava = FLUID_BEHAVIORS.get_behavior(lava_id());
    for _ in 0..times {
        lava.random_tick(world, LAVA);
    }
}

#[test]
fn lava_lights_flammable_blocks_around_it() {
    init();
    with_server_world("lava_fire_spreads", |_server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        world.set_game_rule(&FIRE_SPREAD_RADIUS_AROUND_PLAYER, -1);
        build(world);
        random_ticks(world, 300);
        assert!(fire_count(world) > 0, "lava sets the planks on fire");
    });
}

#[test]
fn lava_lights_nothing_without_a_player_nearby() {
    init();
    with_server_world("lava_fire_no_player", |_server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        build(world);
        random_ticks(world, 300);
        assert_eq!(fire_count(world), 0, "fire only spreads near players");
    });
}
