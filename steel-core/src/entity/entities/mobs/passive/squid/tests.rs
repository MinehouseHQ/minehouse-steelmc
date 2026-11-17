use steel_registry::{init_vanilla_registry, vanilla_entities};

use super::*;

fn test_squid() -> SquidEntity {
    init_vanilla_registry();
    SquidEntity::new(&vanilla_entities::SQUID, 1, DVec3::ZERO, Weak::new())
}

/// A nose-down squid inks behind itself, so Z comes out positive. Reaching for
/// `DVec3::rotate_x` mirrors it and the ink squirts forwards instead.
#[test]
fn rotate_vector_trails_ink_behind_a_pitched_squid() {
    let squid = test_squid();
    squid.state.lock().x_body_rot_old = 30.0;

    let rotated = squid.squid_rotate_vector(DVec3::new(0.0, -1.0, 0.0));

    assert!(
        (rotated - DVec3::new(0.0, -0.866_025, 0.5)).length() < 1e-4,
        "ink must trail behind the squid, got {rotated:?}"
    );
}

#[test]
fn a_squid_may_spawn_in_water() {
    use std::sync::Arc;

    use steel_registry::vanilla_blocks;
    use steel_utils::types::UpdateFlags;
    use steel_utils::{BlockPos, ChunkPos};

    use crate::behavior::init_behaviors;
    use crate::test_support::{fresh_test_world, insert_ready_full_chunk};

    init_vanilla_registry();
    init_behaviors();
    let fixture = fresh_test_world("squid_spawn_obstruction");
    let world = &fixture.world;
    insert_ready_full_chunk(world, ChunkPos::new(0, 0));
    for y in 60..=64 {
        world.set_block(
            BlockPos::new(8, y, 8),
            vanilla_blocks::WATER.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
    }
    let position = DVec3::new(8.5, 62.0, 8.5);
    let squid = SquidEntity::new(&vanilla_entities::SQUID, 1, position, Arc::downgrade(world));
    let glow_squid = GlowSquidEntity::new(
        &vanilla_entities::GLOW_SQUID,
        2,
        position,
        Arc::downgrade(world),
    );

    assert!(squid.check_spawn_obstruction(world.as_ref()));
    assert!(glow_squid.check_spawn_obstruction(world.as_ref()));
}
