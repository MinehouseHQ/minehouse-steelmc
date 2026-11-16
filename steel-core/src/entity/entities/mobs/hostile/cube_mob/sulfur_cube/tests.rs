use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::owned::NbtCompound;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::BlockRef;
use steel_registry::data_components::vanilla_components::SULFUR_CUBE_CONTENT;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::Item;
use steel_registry::{
    init_vanilla_registry, vanilla_attributes, vanilla_blocks, vanilla_damage_types,
    vanilla_entities, vanilla_items,
};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos};

use super::{PICKUP_TIMER_DURATION, SulfurCubeEntity};
use crate::behavior::dispense::DispenseBehavior;
use crate::behavior::init_behaviors;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::hostile::cube_mob::AbstractCubeMob as _;
use crate::entity::{
    AgeableMob, Bucketable as _, Entity, KnockbackCause, LivingEntity, Mob, SharedEntity,
    Shearable as _, init_entities, next_entity_id,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoverType;
use crate::test_support::{TestWorld, fresh_test_world, insert_ready_full_chunk};

fn detached_cube() -> SulfurCubeEntity {
    init_vanilla_registry();
    let cube = SulfurCubeEntity::new(&vanilla_entities::SULFUR_CUBE, 1, DVec3::ZERO, Weak::new());
    cube.set_size(2, true);
    cube
}

/// Swallows `item` and applies its archetypes at once, as the next equipment
/// check would.
fn swallow(cube: &SulfurCubeEntity, item: &'static Item) {
    let stack = ItemStack::new(item);
    cube.set_item_slot(EquipmentSlot::Body, stack.copy_with_count(1));
    cube.apply_body_item_change(&ItemStack::empty(), &stack);
}

fn floor_world(name: &'static str, floor: BlockRef) -> TestWorld {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    let fixture = fresh_test_world(name);
    insert_ready_full_chunk(&fixture.world, ChunkPos::new(0, 0));
    for x in 0..16 {
        for z in 0..16 {
            fixture.world.set_block(
                BlockPos::new(x, 63, z),
                floor.default_state(),
                UpdateFlags::UPDATE_NONE,
            );
        }
    }
    fixture
}

fn cube_in(fixture: &TestWorld, position: DVec3) -> Arc<SulfurCubeEntity> {
    let cube = Arc::new(SulfurCubeEntity::new(
        &vanilla_entities::SULFUR_CUBE,
        next_entity_id(),
        position,
        Arc::downgrade(&fixture.world),
    ));
    cube.set_size(2, true);
    cube
}

#[test]
fn health_is_four_per_size_and_a_size_one_adult_is_a_baby() {
    let cube = detached_cube();
    assert!((cube.get_max_health() - 8.0).abs() < f32::EPSILON);
    assert!(!AgeableMob::is_baby(&cube));
    cube.set_size(1, true);
    assert!((cube.get_max_health() - 4.0).abs() < f32::EPSILON);
    assert!(AgeableMob::is_baby(&cube), "a size 1 sulfur cube is a baby");
}

#[test]
fn swallowing_tnt_makes_it_explosive_and_shearing_spits_it_out() {
    let cube = detached_cube();
    assert!(cube.equip_item(&ItemStack::new(&vanilla_items::TNT)));
    cube.apply_body_item_change(
        &ItemStack::empty(),
        &cube.equipment_in_slot(EquipmentSlot::Body),
    );
    assert!(cube.has_body_item());
    assert!(cube.state.lock().explosion.is_some());
    assert!(cube.state.lock().floats_in_liquids);
    assert!(
        !cube.equip_item(&ItemStack::new(&vanilla_items::TNT)),
        "same block twice"
    );
    assert_eq!(cube.get_split_count(), 2);

    let world = fresh_test_world("sulfur_cube_shear");
    cube.shear(&world.world, SoundSource::Players, &ItemStack::empty());
    assert!(!cube.has_body_item());
    assert_eq!(cube.state.lock().pickup_timer, PICKUP_TIMER_DURATION);
}

#[test]
fn babies_cannot_swallow_blocks() {
    let cube = detached_cube();
    cube.set_size(1, true);
    assert!(!cube.equip_item(&ItemStack::new(&vanilla_items::OAK_PLANKS)));
    assert!(!Mob::can_hold_item(
        &cube,
        &ItemStack::new(&vanilla_items::OAK_PLANKS)
    ));
}

#[test]
fn archetype_modifiers_come_and_go_with_the_block() {
    let cube = detached_cube();
    let resistance = || {
        cube.attributes()
            .lock()
            .required_value(vanilla_attributes::KNOCKBACK_RESISTANCE)
    };
    assert!(resistance().abs() < 1e-9);
    swallow(&cube, &vanilla_items::SOUL_SAND);
    assert!((resistance() - 0.7).abs() < 1e-6, "got {}", resistance());
    let held = cube.equipment_in_slot(EquipmentSlot::Body);
    cube.set_item_slot(EquipmentSlot::Body, ItemStack::empty());
    cube.apply_body_item_change(&held, &ItemStack::empty());
    assert!(resistance().abs() < 1e-9);
    assert!(cube.state.lock().explosion.is_none());
}

#[test]
fn priming_lights_the_fuse_and_stops_the_split() {
    let fixture = floor_world("sulfur_cube_prime", &vanilla_blocks::STONE);
    let cube = cube_in(&fixture, DVec3::new(8.5, 64.0, 8.5));
    swallow(&cube, &vanilla_items::TNT);
    assert!(cube.can_explode());
    assert!(cube.prime_time(false));
    assert_eq!(cube.fuse(), 120);
    assert!(cube.is_primed());
    assert!(!cube.can_explode());
    assert_eq!(cube.get_split_count(), 0);
    assert!(!cube.prime_time(false), "already lit");
}

#[test]
fn fuse_and_bucket_state_round_trip_through_nbt() {
    let cube = detached_cube();
    cube.state.lock().fuse = 33;
    cube.state.lock().pickup_timer = 12;
    cube.set_from_bucket(true);
    let mut nbt = NbtCompound::new();
    cube.save_additional(&mut nbt);
    assert_eq!(nbt.int("fuse"), Some(33));
    assert_eq!(nbt.int("pickup_timer"), Some(12));
    assert_eq!(nbt.byte("from_bucket"), Some(1));
}

#[test]
fn the_bucket_remembers_the_swallowed_block() {
    let cube = detached_cube();
    cube.set_item_slot(
        EquipmentSlot::Body,
        ItemStack::new(&vanilla_items::MAGMA_BLOCK),
    );
    let mut bucket = cube.bucket_item_stack();
    cube.save_to_bucket_tag(&mut bucket);
    let content = bucket.get(SULFUR_CUBE_CONTENT).expect("content saved");
    assert!(
        content
            .absorbed_block_item_stack()
            .create()
            .is(&vanilla_items::MAGMA_BLOCK)
    );

    let restored = detached_cube();
    restored.apply_implicit_item_components(&bucket);
    assert!(
        restored
            .equipment_in_slot(EquipmentSlot::Body)
            .is(&vanilla_items::MAGMA_BLOCK)
    );
}

#[test]
fn a_kicked_cube_flies_away_from_the_attacker() {
    let fixture = floor_world("sulfur_cube_kick", &vanilla_blocks::STONE);
    let cube = cube_in(&fixture, DVec3::new(8.5, 64.0, 8.5));
    swallow(&cube, &vanilla_items::OAK_PLANKS);
    let attacker: SharedEntity = Arc::new(SulfurCubeEntity::new(
        &vanilla_entities::SULFUR_CUBE,
        next_entity_id(),
        DVec3::new(6.5, 64.0, 8.5),
        Arc::downgrade(&fixture.world),
    ));
    attacker.set_rotation((-90.0, 0.0));
    let source = DamageSource::direct(&vanilla_damage_types::PLAYER_ATTACK, attacker);
    cube.set_velocity(DVec3::ZERO);
    // Vanilla knockback direction points back at the attacker.
    cube.knockback_with_source(0.4, -1.0, 0.0, &KnockbackCause::new(&source, 4.0, false));
    let velocity = cube.velocity();
    assert!(velocity.x > 0.0, "pushed away, got {velocity}");
    assert!(velocity.y > 0.0, "lifted, got {velocity}");
    assert!(velocity.z.abs() < 1e-6, "straight hit, got {velocity}");
}

#[test]
fn an_empty_cube_takes_normal_knockback() {
    let fixture = floor_world("sulfur_cube_plain_kick", &vanilla_blocks::STONE);
    let cube = cube_in(&fixture, DVec3::new(8.5, 64.0, 8.5));
    let source = DamageSource::environment(&vanilla_damage_types::GENERIC);
    cube.knockback_with_source(0.4, -1.0, 0.0, &KnockbackCause::new(&source, 4.0, false));
    assert!((cube.velocity().x - 0.4).abs() < 1e-6);
}

#[test]
fn a_bouncy_cube_rebounds_off_a_wall() {
    let fixture = floor_world("sulfur_cube_wall_bounce", &vanilla_blocks::STONE);
    for y in 64..67 {
        fixture.world.set_block(
            BlockPos::new(10, y, 8),
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_NONE,
        );
    }
    let cube = cube_in(&fixture, DVec3::new(8.5, 64.0, 8.5));
    swallow(&cube, &vanilla_items::OAK_PLANKS);
    let bounciness = cube
        .attributes()
        .lock()
        .required_value(vanilla_attributes::BOUNCINESS);
    assert!(bounciness > 0.5, "planks are bouncy: {bounciness}");
    let movement = DVec3::new(2.0, 0.0, 0.0);
    cube.set_velocity(movement);
    let _ = Arc::clone(&cube).move_entity(MoverType::SelfMovement, movement);
    let velocity = cube.velocity();
    assert!(
        (velocity.x + 2.0 * bounciness).abs() < 1e-6,
        "rebounds by its bounciness: {velocity}"
    );
}

#[test]
fn a_plain_cube_stops_on_stone_but_bounces_on_a_bed() {
    let stone = floor_world("sulfur_cube_floor_stone", &vanilla_blocks::STONE);
    let cube = cube_in(&stone, DVec3::new(8.5, 64.5, 8.5));
    let movement = DVec3::new(0.0, -1.0, 0.0);
    cube.set_velocity(movement);
    let _ = Arc::clone(&cube).move_entity(MoverType::SelfMovement, movement);
    assert!(cube.velocity().y.abs() < 1e-9, "stone has no restitution");

    let slime = floor_world("sulfur_cube_floor_bed", &vanilla_blocks::WHITE_BED);
    let cube = cube_in(&slime, DVec3::new(8.5, 64.5, 8.5));
    cube.set_velocity(movement);
    let _ = Arc::clone(&cube).move_entity(MoverType::SelfMovement, movement);
    assert!(cube.velocity().y > 0.0, "beds bounce: {}", cube.velocity());
}

#[test]
fn dispensers_feed_blocks_and_tnt_to_cubes() {
    init_vanilla_registry();
    init_behaviors();
    assert!(matches!(
        DispenseBehavior::for_item(&ItemStack::new(&vanilla_items::TNT)),
        DispenseBehavior::Tnt
    ));
    assert!(matches!(
        DispenseBehavior::for_item(&ItemStack::new(&vanilla_items::OAK_PLANKS)),
        DispenseBehavior::SulfurCubeBlock
    ));
    assert!(matches!(
        DispenseBehavior::for_item(&ItemStack::new(&vanilla_items::STICK)),
        DispenseBehavior::Default
    ));
}
