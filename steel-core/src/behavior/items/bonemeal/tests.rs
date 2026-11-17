use std::sync::Arc;

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::{RegistryEntry as _, init_vanilla_registry, vanilla_biomes, vanilla_blocks};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos, Direction};

use super::BoneMealItem;
use crate::behavior::init_behaviors;
use crate::test_support::{fresh_test_world, insert_ready_full_chunk};
use crate::world::World;

const WALL: BlockPos = BlockPos::new(4, 64, 4);
const WATER: BlockPos = BlockPos::new(5, 64, 4);

/// A stone wall with one water block east of it, on a stone floor.
fn world(name: &'static str, warm_ocean: bool) -> Arc<World> {
    init_vanilla_registry();
    init_behaviors();
    let fixture = fresh_test_world(name);
    insert_ready_full_chunk(&fixture.world, ChunkPos::new(0, 0));
    let world = Arc::clone(&fixture.world);
    if warm_ocean {
        let warm = vanilla_biomes::WARM_OCEAN.id() as u16;
        world
            .chunk_map
            .with_full_chunk(ChunkPos::new(0, 0), |chunk| {
                chunk.fill_biomes(|_, _, _, _| warm);
            })
            .expect("chunk is loaded");
    }
    let stone = vanilla_blocks::STONE.default_state();
    world.set_block(WALL, stone, UpdateFlags::UPDATE_ALL);
    world.set_block(WATER.below(), stone, UpdateFlags::UPDATE_ALL);
    world.set_block(
        WATER,
        vanilla_blocks::WATER.default_state(),
        UpdateFlags::UPDATE_ALL,
    );
    world
}

#[test]
fn a_side_face_in_a_warm_ocean_grows_a_wall_coral() {
    let world = world("bonemeal_wall_coral", true);
    assert!(BoneMealItem::grow_water_plant(
        &world,
        WATER,
        Some(Direction::East)
    ));
    let grown = world.get_block_state(WATER);
    assert!(
        grown.get_block().has_tag(&BlockTag::WALL_CORALS),
        "grew {}",
        grown.get_block().key
    );
}

#[test]
fn elsewhere_the_same_click_grows_seagrass() {
    let world = world("bonemeal_seagrass", false);
    assert!(BoneMealItem::grow_water_plant(
        &world,
        WATER,
        Some(Direction::East)
    ));
    assert_eq!(
        world.get_block_state(WATER).get_block(),
        &vanilla_blocks::SEAGRASS
    );
}
