//! Tests for the generic brain behaviors piglins use.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::vanilla_entities;
use steel_utils::ChunkPos;

use super::{
    UniformInt, back_up_if_too_close, copy_memory_with_expiry, dismount_or_skip_mounting, mount,
    start_celebrating_if_target_dead, stop_being_angry_if_target_dead,
};
use crate::behavior::init_behaviors;
use crate::entity::ai::brain::BehaviorControl;
use crate::entity::ai::brain::memory::AnyMemoryModuleType;
use crate::entity::ai::brain::memory_types::{
    ANGRY_AT, ATTACK_TARGET, AVOID_TARGET, CELEBRATE_LOCATION, DANCING, LOOK_TARGET,
    NEAREST_VISIBLE_LIVING_ENTITIES, NEAREST_VISIBLE_ZOMBIFIED, RIDE_TARGET, WALK_TARGET,
};
use crate::entity::ai::brain::nearest_visible::NearestVisibleLivingEntities;
use crate::entity::entities::HoglinEntity;
use crate::entity::{
    Entity, LivingEntity as _, Mob as _, SharedEntity, init_entities, next_entity_id,
};
use crate::test_support::{TestWorld, fresh_test_world, insert_ready_full_chunk};
use crate::world::World;

fn loaded_world(name: &'static str) -> TestWorld {
    steel_registry::init_vanilla_registry();
    init_behaviors();
    init_entities();
    let fixture = fresh_test_world(name);
    insert_ready_full_chunk(&fixture.world, ChunkPos::new(0, 0));
    fixture
}

/// A hoglin with `extra` memories registered on top of its own brain's.
fn spawn_hoglin(
    world: &Arc<World>,
    position: DVec3,
    extra: &[&'static dyn AnyMemoryModuleType],
) -> Arc<HoglinEntity> {
    let hoglin = Arc::new(HoglinEntity::new(
        &vanilla_entities::HOGLIN,
        next_entity_id(),
        position,
        Arc::downgrade(world),
    ));
    world
        .try_add_entity(Arc::clone(&hoglin) as SharedEntity)
        .expect("hoglin should attach to the loaded chunk");
    let memories = hoglin.brain_memories().expect("hoglins have a brain");
    let mut memories = memories.lock();
    for memory in extra {
        memory.register_into(&mut memories);
    }
    drop(memories);
    hoglin
}

fn shared(hoglin: &Arc<HoglinEntity>) -> SharedEntity {
    Arc::clone(hoglin) as SharedEntity
}

fn run(
    behavior: &mut impl BehaviorControl<HoglinEntity>,
    world: &Arc<World>,
    body: &HoglinEntity,
) -> bool {
    behavior.try_start(world, body, 0)
}

#[test]
fn copy_memory_with_expiry_copies_only_when_allowed_and_empty() {
    let fixture = loaded_world("brain_copy_memory_with_expiry");
    let world = &fixture.world;
    let body = spawn_hoglin(
        world,
        DVec3::new(4.5, 64.0, 4.5),
        &[&NEAREST_VISIBLE_ZOMBIFIED],
    );
    let zombified = spawn_hoglin(world, DVec3::new(6.5, 64.0, 4.5), &[]);
    let memories = body.brain_memories().expect("brain");
    memories
        .lock()
        .set(&NEAREST_VISIBLE_ZOMBIFIED, Some(shared(&zombified)));

    let mut refuse = copy_memory_with_expiry(
        |_| false,
        &NEAREST_VISIBLE_ZOMBIFIED,
        &AVOID_TARGET,
        UniformInt::of(5, 5),
    );
    assert!(!run(&mut refuse, world, &body));
    assert!(!memories.lock().has_value(&AVOID_TARGET));

    let mut copy = copy_memory_with_expiry(
        |_| true,
        &NEAREST_VISIBLE_ZOMBIFIED,
        &AVOID_TARGET,
        UniformInt::of(5, 5),
    );
    assert!(run(&mut copy, world, &body));
    assert_eq!(
        memories.lock().get(&AVOID_TARGET).map(|target| target.id()),
        Some(zombified.id())
    );
    assert_eq!(memories.lock().time_until_expiry(&AVOID_TARGET), Some(5));
    assert!(
        !run(&mut copy, world, &body),
        "target already holds a value"
    );
}

#[test]
fn celebrating_forgets_dead_mobs_but_not_unforgiven_players() {
    let fixture = loaded_world("brain_start_celebrating");
    let world = &fixture.world;
    let body = spawn_hoglin(
        world,
        DVec3::new(4.5, 64.0, 4.5),
        &[&ANGRY_AT, &CELEBRATE_LOCATION, &DANCING],
    );
    let victim = spawn_hoglin(world, DVec3::new(7.5, 64.0, 4.5), &[]);
    let memories = body.brain_memories().expect("brain");
    {
        let mut memories = memories.lock();
        memories.set(&ATTACK_TARGET, Some(shared(&victim)));
        memories.set(&ANGRY_AT, Some(victim.uuid()));
    }
    let mut celebrate = start_celebrating_if_target_dead(300, |_, _, _| true);

    assert!(
        !run(&mut celebrate, world, &body),
        "the target is still alive"
    );
    victim.set_health(0.0);
    assert!(run(&mut celebrate, world, &body));
    let memories = memories.lock();
    assert_eq!(
        memories.get(&CELEBRATE_LOCATION),
        Some(victim.block_position())
    );
    assert_eq!(memories.time_until_expiry(&CELEBRATE_LOCATION), Some(300));
    assert_eq!(memories.get(&DANCING), Some(true));
    assert!(!memories.has_value(&ATTACK_TARGET));
    assert!(!memories.has_value(&ANGRY_AT));
}

#[test]
fn stop_being_angry_waits_for_the_target_to_die() {
    let fixture = loaded_world("brain_stop_being_angry");
    let world = &fixture.world;
    let body = spawn_hoglin(world, DVec3::new(4.5, 64.0, 4.5), &[&ANGRY_AT]);
    let victim = spawn_hoglin(world, DVec3::new(7.5, 64.0, 4.5), &[]);
    let memories = body.brain_memories().expect("brain");
    memories.lock().set(&ANGRY_AT, Some(victim.uuid()));
    let mut stop = stop_being_angry_if_target_dead();

    assert!(run(&mut stop, world, &body));
    assert!(memories.lock().has_value(&ANGRY_AT));
    victim.set_health(0.0);
    assert!(run(&mut stop, world, &body));
    assert!(!memories.lock().has_value(&ANGRY_AT));
}

#[test]
fn back_up_strafes_away_from_a_close_visible_target() {
    let fixture = loaded_world("brain_back_up_if_too_close");
    let world = &fixture.world;
    let body = spawn_hoglin(world, DVec3::new(4.5, 64.0, 4.5), &[]);
    let near = spawn_hoglin(world, DVec3::new(6.5, 64.0, 4.5), &[]);
    let far = spawn_hoglin(world, DVec3::new(12.5, 64.0, 4.5), &[]);
    let memories = body.brain_memories().expect("brain");
    memories.lock().set(
        &NEAREST_VISIBLE_LIVING_ENTITIES,
        Some(NearestVisibleLivingEntities::new(
            world,
            &shared(&body),
            vec![shared(&near), shared(&far)],
        )),
    );
    let mut back_up = back_up_if_too_close(5, 0.75);

    memories.lock().set(&ATTACK_TARGET, Some(shared(&far)));
    assert!(!run(&mut back_up, world, &body), "the target is far enough");
    memories.lock().set(&ATTACK_TARGET, Some(shared(&near)));
    assert!(run(&mut back_up, world, &body));
    let forward = body
        .mob_base()
        .controls()
        .lock()
        .move_control
        .strafe_forward();
    assert!((forward + 0.75).abs() < f32::EPSILON);
    assert_eq!(
        memories
            .lock()
            .get(&LOOK_TARGET)
            .and_then(|look| look.tracked_entity().map(|entity| entity.id())),
        Some(near.id())
    );
}

#[test]
fn mount_walks_to_a_far_ride_target_and_dismount_forgets_a_dead_one() {
    let fixture = loaded_world("brain_mount_dismount");
    let world = &fixture.world;
    let body = spawn_hoglin(world, DVec3::new(4.5, 64.0, 4.5), &[&RIDE_TARGET]);
    let vehicle = spawn_hoglin(world, DVec3::new(9.5, 64.0, 4.5), &[]);
    let memories = body.brain_memories().expect("brain");
    memories.lock().set(&RIDE_TARGET, Some(shared(&vehicle)));
    let mut mount = mount(0.8);

    assert!(run(&mut mount, world, &body));
    assert!(!body.is_passenger());
    assert!(memories.lock().has_value(&WALK_TARGET));
    assert!(!run(&mut mount, world, &body), "already walking somewhere");

    let mut dismount = dismount_or_skip_mounting(8, |_, _| false);
    assert!(!run(&mut dismount, world, &body), "the vehicle is fine");
    vehicle.set_health(0.0);
    assert!(run(&mut dismount, world, &body));
    assert!(!memories.lock().has_value(&RIDE_TARGET));
}
