use std::sync::Arc;

use glam::DVec3;
use steel_registry::{
    init_vanilla_registry, vanilla_blocks, vanilla_damage_types, vanilla_entities,
};
use steel_utils::text::DisplayResolutor;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos};
use text_components::TextComponent;

use super::{FallLocation, death_message, recheck_status, record_damage};
use crate::behavior::init_behaviors;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::hostile::ZombieEntity;
use crate::entity::{Entity as _, LivingEntity as _, SharedEntity, init_entities, next_entity_id};
use crate::test_support::{fresh_test_world, insert_ready_full_chunk};
use crate::world::World;

fn world(name: &'static str) -> Arc<World> {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    let fixture = fresh_test_world(name);
    insert_ready_full_chunk(&fixture.world, ChunkPos::new(0, 0));
    Arc::clone(&fixture.world)
}

fn zombie(world: &Arc<World>, x: f64) -> Arc<ZombieEntity> {
    let zombie = Arc::new(ZombieEntity::new(
        &vanilla_entities::ZOMBIE,
        next_entity_id(),
        DVec3::new(x, 64.0, 4.5),
        Arc::downgrade(world),
    ));
    world
        .try_add_entity(Arc::clone(&zombie) as SharedEntity)
        .expect("zombie should attach to the loaded chunk");
    zombie
}

fn victim(world: &Arc<World>) -> Arc<ZombieEntity> {
    let victim = zombie(world, 4.5);
    victim.set_custom_name(Some(TextComponent::plain("Bob")));
    victim
}

fn message(victim: &ZombieEntity) -> String {
    death_message(victim).to_plain(&DisplayResolutor)
}

#[test]
fn an_empty_log_reads_as_died() {
    let world = world("combat_tracker_empty");
    let victim = victim(&world);
    assert_eq!(message(&victim), "Bob died");
}

#[test]
fn a_mob_kill_names_the_attacker() {
    let world = world("combat_tracker_mob");
    let victim = victim(&world);
    let attacker = zombie(&world, 6.5);
    let source = DamageSource::direct(
        &vanilla_damage_types::MOB_ATTACK,
        Arc::clone(&attacker) as SharedEntity,
    );
    record_damage(victim.as_ref(), &source, 4.0);
    assert_eq!(message(&victim), "Bob was slain by Zombie");
    assert!(victim.living_base().combat_tracker().lock().in_combat());
}

#[test]
fn environmental_damage_does_not_start_combat() {
    let world = world("combat_tracker_drown");
    let victim = victim(&world);
    record_damage(
        victim.as_ref(),
        &DamageSource::environment(&vanilla_damage_types::DROWN),
        2.0,
    );
    assert_eq!(message(&victim), "Bob drowned");
    assert!(!victim.living_base().combat_tracker().lock().in_combat());
}

#[test]
fn a_long_fall_from_a_ladder_names_the_ladder() {
    let world = world("combat_tracker_ladder");
    let ladder = BlockPos::new(4, 70, 4);
    world.set_block(
        ladder,
        vanilla_blocks::LADDER.default_state(),
        UpdateFlags::UPDATE_ALL,
    );
    let victim = victim(&world);
    victim.set_last_climbable_pos(ladder);
    assert_eq!(
        FallLocation::current(victim.as_ref()),
        Some(FallLocation::Ladder)
    );
    victim.set_fall_distance(12.0);
    record_damage(
        victim.as_ref(),
        &DamageSource::environment(&vanilla_damage_types::FALL),
        9.0,
    );
    assert_eq!(message(&victim), "Bob fell off a ladder");
}

#[test]
fn a_fall_after_a_hit_blames_the_attacker() {
    let world = world("combat_tracker_assist");
    let victim = victim(&world);
    let attacker = zombie(&world, 6.5);
    record_damage(
        victim.as_ref(),
        &DamageSource::direct(
            &vanilla_damage_types::MOB_ATTACK,
            Arc::clone(&attacker) as SharedEntity,
        ),
        2.0,
    );
    victim.set_fall_distance(10.0);
    record_damage(
        victim.as_ref(),
        &DamageSource::environment(&vanilla_damage_types::FALL),
        8.0,
    );
    assert_eq!(message(&victim), "Bob was doomed to fall by Zombie");
}

#[test]
fn death_clears_the_log() {
    let world = world("combat_tracker_recheck");
    let victim = victim(&world);
    record_damage(
        victim.as_ref(),
        &DamageSource::environment(&vanilla_damage_types::DROWN),
        2.0,
    );
    recheck_status(victim.as_ref());
    assert_eq!(
        message(&victim),
        "Bob drowned",
        "a living entity keeps its log"
    );
    victim.set_health(0.0);
    recheck_status(victim.as_ref());
    assert_eq!(message(&victim), "Bob died");
}
