use std::sync::Arc;

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, PotentSulfurState};
use steel_registry::{
    init_vanilla_registry, vanilla_blocks, vanilla_entities, vanilla_mob_effects,
};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos};

use super::PotentSulfurBlockEntity;
use crate::behavior::init_behaviors;
use crate::block_entity::init_block_entities;
use crate::entity::entities::mobs::hostile::ZombieEntity;
use crate::entity::{LivingEntity as _, SharedEntity, init_entities, next_entity_id};
use crate::test_support::{fresh_test_world, insert_ready_full_chunk};
use crate::world::World;

const SULFUR: BlockPos = BlockPos::new(4, 64, 4);

/// Wet potent sulfur under one block of still water, with air above.
fn world(name: &'static str) -> Arc<World> {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
    let fixture = fresh_test_world(name);
    insert_ready_full_chunk(&fixture.world, ChunkPos::new(0, 0));
    let world = Arc::clone(&fixture.world);
    let sulfur = vanilla_blocks::POTENT_SULFUR.default_state().set_value(
        &BlockStateProperties::POTENT_SULFUR_STATE,
        PotentSulfurState::Wet,
    );
    world.set_block(SULFUR, sulfur, UpdateFlags::UPDATE_CLIENTS);
    world.set_block(
        SULFUR.above(),
        vanilla_blocks::WATER.default_state(),
        UpdateFlags::UPDATE_CLIENTS,
    );
    world
}

fn zombie_at(world: &Arc<World>, position: DVec3) -> Arc<ZombieEntity> {
    let zombie = Arc::new(ZombieEntity::new(
        &vanilla_entities::ZOMBIE,
        next_entity_id(),
        position,
        Arc::downgrade(world),
    ));
    world
        .try_add_entity(Arc::clone(&zombie) as SharedEntity)
        .expect("zombie should attach to the loaded chunk");
    zombie
}

#[test]
fn breathing_the_gas_over_the_water_gives_nausea() {
    let world = world("potent_sulfur_nausea");
    let zombie = zombie_at(&world, DVec3::new(4.5, 65.0, 4.5));
    PotentSulfurBlockEntity::tick_nausea(&world, SULFUR);
    assert!(zombie.has_mob_effect(vanilla_mob_effects::NAUSEA));
}

#[test]
fn the_gas_does_not_reach_a_mob_standing_away_from_the_water() {
    let world = world("potent_sulfur_no_nausea");
    world.set_block(
        BlockPos::new(6, 64, 4),
        vanilla_blocks::STONE.default_state(),
        UpdateFlags::UPDATE_CLIENTS,
    );
    let zombie = zombie_at(&world, DVec3::new(6.5, 65.0, 4.5));
    PotentSulfurBlockEntity::tick_nausea(&world, SULFUR);
    assert!(!zombie.has_mob_effect(vanilla_mob_effects::NAUSEA));
}
