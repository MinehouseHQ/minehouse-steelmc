use std::sync::Arc;

use glam::DVec3;
use steel_registry::{init_vanilla_registry, vanilla_damage_types, vanilla_entities};
use steel_utils::{ChunkPos, Downcast as _, WorldAabb};

use super::{DragonPhase, EnderDragonEntity, EnderDragonPartEntity};
use crate::behavior::init_behaviors;
use crate::entity::damage::DamageSource;
use crate::entity::{Entity, LivingEntity, SharedEntity, init_entities, next_entity_id};
use crate::test_support::{TestWorld, fresh_test_world, insert_ready_full_chunk};

fn world(name: &'static str) -> TestWorld {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    let fixture = fresh_test_world(name);
    for x in -1..=1 {
        for z in -1..=1 {
            insert_ready_full_chunk(&fixture.world, ChunkPos::new(x, z));
        }
    }
    fixture
}

fn add_dragon(fixture: &TestWorld) -> (Arc<EnderDragonEntity>, SharedEntity) {
    let dragon = Arc::new(EnderDragonEntity::new(
        &vanilla_entities::ENDER_DRAGON,
        next_entity_id(),
        DVec3::new(0.5, 100.0, 0.5),
        Arc::downgrade(&fixture.world),
    ));
    let shared: SharedEntity = Arc::<EnderDragonEntity>::clone(&dragon);
    fixture
        .world
        .try_add_entity(Arc::clone(&shared))
        .expect("the dragon attaches to the loaded chunk");
    (dragon, shared)
}

fn explosion() -> DamageSource {
    DamageSource::environment(&vanilla_damage_types::EXPLOSION)
}

#[test]
fn phase_ids_match_vanilla() {
    assert_eq!(DragonPhase::HoldingPattern.id(), 0);
    assert_eq!(DragonPhase::Dying.id(), 9);
    assert_eq!(DragonPhase::Hovering.id(), 10);
    assert_eq!(DragonPhase::from_id(7), DragonPhase::SittingAttacking);
    assert_eq!(DragonPhase::from_id(42), DragonPhase::HoldingPattern);
    assert!(DragonPhase::Hovering.is_sitting());
    assert!(!DragonPhase::Takeoff.is_sitting());
}

#[test]
fn parts_take_the_ids_after_the_dragon_and_are_found_by_id() {
    let fixture = world("ender_dragon_part_ids");
    let (dragon, shared) = add_dragon(&fixture);
    let ids: Vec<i32> = dragon.parts().iter().map(|part| part.id()).collect();
    assert_eq!(ids, (1..=8).map(|n| dragon.id() + n).collect::<Vec<_>>());

    dragon.ai_step(&shared);
    let head = fixture
        .world
        .get_accessible_entity_by_id(dragon.id() + 1)
        .expect("the world finds the head by its id");
    let head = head
        .downcast_ref::<EnderDragonPartEntity>()
        .expect("the id belongs to a dragon part");
    assert_eq!(head.name(), "head");
    assert_eq!(head.parent_id(), dragon.id());
    // Hovering, the head hangs one block down and 6.5 ahead.
    assert!((head.position().distance(dragon.position()) - 6.5_f64.hypot(1.0)).abs() < 0.01);

    let near_head = fixture.world.get_entities_in_aabb_with_parts(
        None,
        &WorldAabb::from_min_max(head.position() - 0.1, head.position() + 0.1),
        |_| true,
    );
    assert!(near_head.iter().any(|entity| entity.id() == head.id()));
}

#[test]
fn the_head_takes_full_damage_and_the_wings_a_quarter() {
    let fixture = world("ender_dragon_part_damage");
    let (dragon, _shared) = add_dragon(&fixture);
    let max = dragon.get_max_health();
    assert!((max - 200.0).abs() < f32::EPSILON);

    assert!(dragon.hurt_part(&fixture.world, &dragon.parts()[0], &explosion(), 10.0));
    assert!((dragon.get_health() - (max - 10.0)).abs() < 0.001);

    // Clear the invulnerability window so the next hit lands.
    for _ in 0..20 {
        dragon.living_base().decrement_invulnerable_time();
    }
    assert!(dragon.parts()[6].hurt(&fixture.world, &explosion(), 8.0));
    assert!((dragon.get_health() - (max - 13.0)).abs() < 0.001);
}

#[test]
fn only_players_and_dragon_hurting_sources_hurt_it() {
    let fixture = world("ender_dragon_mob_damage");
    let (dragon, _shared) = add_dragon(&fixture);
    let max = dragon.get_max_health();
    let mob_attack = DamageSource::environment(&vanilla_damage_types::MOB_ATTACK);
    assert!(dragon.hurt(&fixture.world, &mob_attack, 50.0));
    assert!((dragon.get_health() - max).abs() < f32::EPSILON);
}

#[test]
fn a_killing_blow_in_flight_starts_the_death_flight() {
    let fixture = world("ender_dragon_killing_blow");
    let (dragon, _shared) = add_dragon(&fixture);
    dragon.set_phase(DragonPhase::HoldingPattern);
    dragon.hurt(&fixture.world, &explosion(), 10_000.0);
    assert_eq!(dragon.phase(), DragonPhase::Dying);
    assert!((dragon.get_health() - 1.0).abs() < f32::EPSILON);
}

#[test]
fn the_death_animation_rises_and_ends_after_ten_seconds() {
    let fixture = world("ender_dragon_death");
    let (dragon, _shared) = add_dragon(&fixture);
    let start = dragon.position().y;
    dragon.set_health(0.0);
    for _ in 0..199 {
        dragon.tick_death();
    }
    assert!(!dragon.is_removed());
    assert!(dragon.position().y > start + 19.0);
    dragon.tick_death();
    assert!(dragon.is_removed());
}

#[test]
fn the_holding_pattern_flies_along_the_node_graph() {
    let fixture = world("ender_dragon_holding_pattern");
    let (dragon, shared) = add_dragon(&fixture);
    dragon.set_phase(DragonPhase::HoldingPattern);
    let start = dragon.position();
    for _ in 0..100 {
        dragon.ai_step(&shared);
    }
    let target = dragon
        .state
        .lock()
        .phases
        .fly_target()
        .expect("circling, the dragon always has a node to fly to");
    // Nodes sit at least at y 73 and the dragon aims up to 20 above them.
    assert!((73.0..=93.0 + 15.0).contains(&target.y), "target {target}");
    assert!(dragon.position().distance(start) > 5.0);
    assert_ne!(dragon.phase(), DragonPhase::Dying);
}

#[test]
fn a_perched_dragon_takes_off_after_heavy_damage() {
    let fixture = world("ender_dragon_sitting_takeoff");
    let (dragon, _shared) = add_dragon(&fixture);
    dragon.set_phase(DragonPhase::SittingScanning);
    // A quarter of its health in one hit makes it take off.
    dragon.hurt_part(&fixture.world, &dragon.parts()[0], &explosion(), 60.0);
    assert_eq!(dragon.phase(), DragonPhase::Takeoff);
}
