//! Warden tests.

use std::io::Cursor;
use std::sync::Arc;

use glam::DVec3;
use simdnbt::borrow::read_compound;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::{
    init_vanilla_registry, vanilla_blocks, vanilla_damage_types, vanilla_entities,
    vanilla_mob_effects,
};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos, Downcast as _, WorldAabb};

use super::{AngerLevel, AngerManagement, WardenEntity};
use crate::behavior::init_behaviors;
use crate::block_entity::entities::SculkShriekerBlockEntity;
use crate::block_entity::{SharedBlockEntity, init_block_entities};
use crate::entity::ai::brain::memory_types::{ATTACK_TARGET, IS_EMERGING};
use crate::entity::damage::DamageSource;
use crate::entity::entities::ZombieEntity;
use crate::entity::{
    Entity, EntityPose, LivingEntity as _, SharedEntity, init_entities, next_entity_id,
};
use crate::player::Player;
use crate::player::warden_spawn_tracker::WardenSpawnTracker;
use crate::test_support::{
    TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk,
};
use crate::world::World;

const SHRIEKER: BlockPos = BlockPos::new(8, 64, 8);

fn loaded_world(name: &'static str) -> TestWorld {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
    let fixture = fresh_test_world(name);
    for x in -1..=1 {
        for z in -1..=1 {
            insert_ready_full_chunk(&fixture.world, ChunkPos::new(x, z));
        }
    }
    fixture
}

fn spawn_warden(world: &Arc<World>, position: DVec3) -> Arc<WardenEntity> {
    let warden = Arc::new(WardenEntity::new(
        &vanilla_entities::WARDEN,
        next_entity_id(),
        position,
        Arc::downgrade(world),
    ));
    world
        .try_add_entity(Arc::clone(&warden) as SharedEntity)
        .expect("warden should attach to the loaded chunk");
    warden
}

fn spawn_zombie(world: &Arc<World>, position: DVec3) -> SharedEntity {
    let zombie: SharedEntity = Arc::new(ZombieEntity::new(
        &vanilla_entities::ZOMBIE,
        next_entity_id(),
        position,
        Arc::downgrade(world),
    ));
    world
        .try_add_entity(Arc::clone(&zombie))
        .expect("zombie should attach to the loaded chunk");
    zombie
}

fn player_at(world: &Arc<World>, name: &'static str, id: i32, position: DVec3) -> Arc<Player> {
    let player = TestPlayerBuilder::new(Arc::clone(world), name, id).build();
    player.snap_to(position, 0.0, 0.0);
    assert!(world.players.insert(Arc::clone(&player)));
    player
}

fn ticked(anger: &mut AngerManagement, world: &World, times: usize) {
    for _ in 0..times {
        anger.tick(world, |_| true);
    }
}

#[test]
fn angry_suspects_come_first_then_players_then_the_angriest() {
    let fixture = loaded_world("warden_anger_order");
    let world = &fixture.world;
    let zombie = spawn_zombie(world, DVec3::new(4.5, 64.0, 4.5));
    let player: SharedEntity = player_at(world, "Walker", 1, DVec3::new(6.5, 64.0, 6.5));
    let mut anger = AngerManagement::default();

    assert_eq!(anger.increase_anger(&zombie, 60), 60);
    assert_eq!(anger.increase_anger(&player, 50), 50);
    // Neither is angry yet, so the player wins over the angrier zombie.
    let first = anger.active_entity(|_| true).expect("a suspect");
    assert_eq!(first.id(), player.id());
    assert_eq!(anger.active_anger(None), 60);

    // An angry zombie goes before a merely agitated player.
    assert_eq!(anger.increase_anger(&zombie, 30), 90);
    let first = anger.active_entity(|_| true).expect("a suspect");
    assert_eq!(first.id(), zombie.id());
    assert_eq!(anger.active_anger(Some(player.as_ref())), 50);

    // Anger caps at 150.
    assert_eq!(anger.increase_anger(&zombie, 500), 150);
}

#[test]
fn anger_cools_down_and_forgets_suspects() {
    let fixture = loaded_world("warden_anger_decay");
    let world = &fixture.world;
    let zombie = spawn_zombie(world, DVec3::new(4.5, 64.0, 4.5));
    let mut anger = AngerManagement::default();
    anger.increase_anger(&zombie, 3);

    ticked(&mut anger, world, 2);
    assert_eq!(anger.anger_at(zombie.as_ref()), 1);
    ticked(&mut anger, world, 1);
    assert!(!anger.is_suspect(zombie.as_ref()));
    assert_eq!(anger.active_anger(None), 0);

    // A suspect the filter rejects is dropped at once.
    anger.increase_anger(&zombie, 100);
    anger.tick(world, |_| false);
    assert!(!anger.is_suspect(zombie.as_ref()));

    anger.increase_anger(&zombie, 100);
    anger.clear_anger(zombie.as_ref());
    assert!(!anger.is_suspect(zombie.as_ref()));
}

fn reload(nbt: &NbtCompound) -> AngerManagement {
    let mut bytes = Vec::new();
    nbt.write(&mut bytes);
    let borrowed = read_compound(&mut Cursor::new(&bytes)).expect("valid nbt");
    AngerManagement::load((&borrowed).into())
}

#[test]
fn saved_anger_finds_its_suspect_again_by_uuid() {
    let fixture = loaded_world("warden_anger_saved");
    let world = &fixture.world;
    let zombie = spawn_zombie(world, DVec3::new(4.5, 64.0, 4.5));
    let mut anger = AngerManagement::default();
    anger.increase_anger(&zombie, 70);

    let mut loaded = reload(&anger.save());
    assert_eq!(loaded.anger_by_uuid(), &[(zombie.uuid(), 70)]);
    assert!(!loaded.is_suspect(zombie.as_ref()));

    // The lookup runs within three ticks; the anger keeps cooling meanwhile.
    ticked(&mut loaded, world, 3);
    assert!(loaded.is_suspect(zombie.as_ref()));
    assert_eq!(loaded.anger_by_uuid(), &[]);
    let left = loaded.anger_at(zombie.as_ref());
    assert!((67..70).contains(&left), "anger {left}");

    // A suspect saved again before it is found keeps its new anger.
    let mut fresh = reload(&anger.save());
    assert_eq!(fresh.increase_anger(&zombie, 10), 80);
}

#[test]
fn a_bad_suspect_list_loads_as_no_suspects() {
    let mut entry = NbtCompound::new();
    entry.insert("uuid", NbtTag::IntArray(vec![1, 2, 3, 4]));
    entry.insert("anger", -5);
    let mut nbt = NbtCompound::new();
    nbt.insert("suspects", NbtTag::List(NbtList::Compound(vec![entry])));

    assert_eq!(reload(&nbt).anger_by_uuid(), &[]);
    assert_eq!(reload(&NbtCompound::new()).anger_by_uuid(), &[]);
}

#[test]
fn hitting_the_warden_makes_the_attacker_its_target() {
    let fixture = loaded_world("warden_hurt");
    let world = &fixture.world;
    let warden = spawn_warden(world, DVec3::new(8.5, 64.0, 8.5));
    let zombie = spawn_zombie(world, DVec3::new(10.5, 64.0, 8.5));
    assert_eq!(warden.anger_level(), AngerLevel::Calm);

    let source = DamageSource::direct(
        &vanilla_damage_types::MOB_ATTACK,
        SharedEntity::clone(&zombie),
    );
    assert!(warden.hurt_server(world, &source, 1.0));

    // Angry's minimum plus the on-hurt boost.
    assert_eq!(
        warden.anger_management().lock().anger_at(zombie.as_ref()),
        100
    );
    assert_eq!(warden.anger_level(), AngerLevel::Angry);
    let target = warden
        .memories()
        .lock()
        .get(&ATTACK_TARGET)
        .expect("the attacker becomes the target");
    assert_eq!(target.id(), zombie.id());
    assert_eq!(
        warden.entity_angry_at().map(|entity| entity.id()),
        Some(zombie.id())
    );
}

#[test]
fn the_warden_ignores_other_wardens_and_armor_stands() {
    let fixture = loaded_world("warden_targets");
    let world = &fixture.world;
    let warden = spawn_warden(world, DVec3::new(8.5, 64.0, 8.5));
    let other = spawn_warden(world, DVec3::new(12.5, 64.0, 8.5));
    let zombie = spawn_zombie(world, DVec3::new(4.5, 64.0, 8.5));

    assert!(!warden.can_target_entity(other.as_ref()));
    assert!(warden.can_target_entity(zombie.as_ref()));

    let other: SharedEntity = other;
    warden.increase_anger_at(Some(&other));
    assert!(!warden.anger_management().lock().is_suspect(other.as_ref()));
    warden.increase_anger_at(Some(&zombie));
    assert_eq!(
        warden.anger_management().lock().anger_at(zombie.as_ref()),
        35
    );
    assert_eq!(warden.anger_level(), AngerLevel::Calm);
}

/// A stone floor under the shrieker, wide enough for every spawn attempt.
fn shrieker_on_a_floor(world: &Arc<World>) -> SharedBlockEntity {
    let stone = vanilla_blocks::STONE.default_state();
    for x in -2..=18 {
        for z in -2..=18 {
            world.set_block(BlockPos::new(x, 63, z), stone, UpdateFlags::UPDATE_ALL);
        }
    }
    let shrieker = vanilla_blocks::SCULK_SHRIEKER
        .default_state()
        .set_value(&BlockStateProperties::CAN_SUMMON, true);
    assert!(world.set_block(SHRIEKER, shrieker, UpdateFlags::UPDATE_ALL));
    world.get_block_entity(SHRIEKER).expect("a sculk shrieker")
}

fn shrieker(block_entity: &SharedBlockEntity) -> &SculkShriekerBlockEntity {
    block_entity
        .downcast_ref::<SculkShriekerBlockEntity>()
        .expect("a sculk shrieker block entity")
}

fn wardens_near(world: &World, center: BlockPos) -> Vec<SharedEntity> {
    let (x, y, z) = center.get_center();
    world
        .get_entities_in_aabb(&WorldAabb::new(
            x - 8.0,
            y - 8.0,
            z - 8.0,
            x + 8.0,
            y + 8.0,
            z + 8.0,
        ))
        .into_iter()
        .filter(|entity| entity.entity_type() == &vanilla_entities::WARDEN)
        .collect()
}

#[test]
fn the_last_warning_summons_an_emerging_warden() {
    let fixture = loaded_world("warden_summon");
    let world = &fixture.world;
    let block_entity = shrieker_on_a_floor(world);
    let shrieker = shrieker(&block_entity);
    let player = player_at(world, "Walker", 1, DVec3::new(8.5, 64.0, 10.5));

    // Three warnings short of the warden: the reply is only a sound.
    player.set_warden_spawn_tracker(WardenSpawnTracker::new(0, 2, 0));
    shrieker.try_shriek(world, Some(&player));
    shrieker.try_respond(world);
    assert!(wardens_near(world, SHRIEKER).is_empty());

    // The shriek ends; the fourth warning brings the warden.
    let quiet = world
        .get_block_state(SHRIEKER)
        .set_value(&BlockStateProperties::SHRIEKING, false);
    assert!(world.set_block(SHRIEKER, quiet, UpdateFlags::UPDATE_ALL));
    player.set_warden_spawn_tracker(WardenSpawnTracker::new(0, 3, 0));
    shrieker.try_shriek(world, Some(&player));
    assert_eq!(shrieker.warning_level(), 4);
    shrieker.try_respond(world);

    let wardens = wardens_near(world, SHRIEKER);
    assert_eq!(wardens.len(), 1);
    let warden = wardens[0]
        .downcast_ref::<WardenEntity>()
        .expect("a warden entity");
    assert_eq!(warden.pose(), EntityPose::Emerging);
    assert!(warden.memories().lock().has_value(&IS_EMERGING));
    assert!(warden.on_ground() || warden.position().y >= 64.0);
    assert!(player.mob_effect(vanilla_mob_effects::DARKNESS).is_some());
}

#[test]
fn an_angry_warden_roars_then_attacks() {
    let fixture = loaded_world("warden_roar");
    let world = &fixture.world;
    let stone = vanilla_blocks::STONE.default_state();
    for x in 0..=16 {
        for z in 0..=16 {
            world.set_block(BlockPos::new(x, 63, z), stone, UpdateFlags::UPDATE_ALL);
        }
    }
    let warden = spawn_warden(world, DVec3::new(8.5, 64.0, 8.5));
    let zombie = spawn_zombie(world, DVec3::new(12.5, 64.0, 8.5));
    warden.increase_anger_at_by(Some(&zombie), 100, false);
    assert!(warden.memories().lock().get(&ATTACK_TARGET).is_none());

    let mut roared = false;
    for _ in 0..200 {
        world.level_data.write().advance_game_time();
        Arc::clone(&warden).tick();
        roared |= warden.pose() == EntityPose::Roaring;
        if warden.memories().lock().has_value(&ATTACK_TARGET) {
            break;
        }
    }
    assert!(roared, "the warden roars before it attacks");
    let target = warden
        .memories()
        .lock()
        .get(&ATTACK_TARGET)
        .expect("the roar ends in an attack");
    assert_eq!(target.id(), zombie.id());
}
