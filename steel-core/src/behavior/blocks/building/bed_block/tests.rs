use std::sync::Arc;

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BedPart, BlockStateProperties};
use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_entities};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos, Direction};

use super::kick_villager_out_of_bed;
use crate::behavior::init_behaviors;
use crate::entity::entities::mobs::villagers::VillagerEntity;
use crate::entity::{LivingEntity as _, SharedEntity, init_entities, next_entity_id};
use crate::test_support::{fresh_test_world, insert_ready_full_chunk};
use crate::world::World;

const HEAD: BlockPos = BlockPos::new(4, 64, 4);

fn world_with_bed(name: &'static str) -> Arc<World> {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    let fixture = fresh_test_world(name);
    insert_ready_full_chunk(&fixture.world, ChunkPos::new(0, 0));
    let world = Arc::clone(&fixture.world);
    let head = vanilla_blocks::RED_BED
        .default_state()
        .set_value(&BlockStateProperties::HORIZONTAL_FACING, Direction::East)
        .set_value(&BlockStateProperties::BED_PART, BedPart::Head);
    world.set_block(HEAD, head, UpdateFlags::UPDATE_ALL);
    world
}

#[test]
fn using_an_occupied_bed_wakes_the_sleeping_villager() {
    let world = world_with_bed("bed_kicks_villager");
    let villager = Arc::new(VillagerEntity::new(
        &vanilla_entities::VILLAGER,
        next_entity_id(),
        DVec3::new(4.5, 64.6, 4.5),
        Arc::downgrade(&world),
    ));
    world
        .try_add_entity(Arc::clone(&villager) as SharedEntity)
        .expect("villager should attach to the loaded chunk");
    villager
        .start_sleeping(HEAD)
        .expect("the villager can sleep in the bed");
    assert!(villager.is_sleeping());

    assert!(kick_villager_out_of_bed(&world, HEAD));
    assert!(!villager.is_sleeping());
    assert!(
        !kick_villager_out_of_bed(&world, HEAD),
        "nobody is left to wake"
    );
}
