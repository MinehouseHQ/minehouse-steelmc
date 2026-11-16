use std::io::Cursor;
use std::sync::Arc;

use glam::DVec3;
use simdnbt::borrow::read_compound;
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::item_stack::ItemStack;
use steel_registry::{
    init_vanilla_registry, vanilla_blocks, vanilla_damage_types, vanilla_entities, vanilla_items,
    vanilla_mob_effects,
};
use steel_utils::types::{Difficulty, UpdateFlags};
use steel_utils::{BlockPos, ChunkPos, Downcast as _, WorldAabb};

use super::{INVULNERABLE_TICKS, WitherBossEntity};
use crate::behavior::blocks::WitherSkullBlock;
use crate::behavior::init_behaviors;
use crate::block_entity::init_block_entities;
use crate::entity::damage::DamageSource;
use crate::entity::entities::ZombieEntity;
use crate::entity::living_base::MobEffectInstance;
use crate::entity::{Entity, LivingEntity, Mob as _, SharedEntity, init_entities, next_entity_id};
use crate::test_support::{TestWorld, fresh_test_world, insert_ready_full_chunk};

fn world(name: &'static str) -> TestWorld {
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
    fixture.world.set_difficulty(Difficulty::Hard);
    fixture
}

fn add_wither(fixture: &TestWorld) -> (Arc<WitherBossEntity>, SharedEntity) {
    let wither = Arc::new(WitherBossEntity::new(
        &vanilla_entities::WITHER,
        next_entity_id(),
        DVec3::new(0.5, 100.0, 0.5),
        Arc::downgrade(&fixture.world),
    ));
    let shared: SharedEntity = Arc::<WitherBossEntity>::clone(&wither);
    fixture
        .world
        .try_add_entity(Arc::clone(&shared))
        .expect("the wither attaches to the loaded chunk");
    (wither, shared)
}

/// The T of soul sand along X with its bottom middle at (1, 64, 0).
fn build_base(fixture: &TestWorld) {
    let world = &fixture.world;
    for pos in [
        BlockPos::new(1, 64, 0),
        BlockPos::new(0, 65, 0),
        BlockPos::new(1, 65, 0),
        BlockPos::new(2, 65, 0),
    ] {
        world.set_block(
            pos,
            vanilla_blocks::SOUL_SAND.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
    }
    for pos in [BlockPos::new(0, 64, 0), BlockPos::new(2, 64, 0)] {
        world.set_block(
            pos,
            vanilla_blocks::AIR.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
    }
}

fn withers_near(fixture: &TestWorld, center: DVec3) -> Vec<SharedEntity> {
    fixture.world.get_entities_in_aabb_matching(
        &WorldAabb::from_min_max(center - 4.0, center + 4.0),
        |entity| entity.downcast_ref::<WitherBossEntity>().is_some(),
    )
}

#[test]
fn three_skulls_on_soul_sand_build_a_charging_wither() {
    let fixture = world("wither_build");
    build_base(&fixture);
    let skull = vanilla_blocks::WITHER_SKELETON_SKULL.default_state();
    for x in 0..=1 {
        fixture
            .world
            .set_block(BlockPos::new(x, 66, 0), skull, UpdateFlags::UPDATE_ALL);
    }
    let spawn = DVec3::new(1.5, 64.55, 0.5);
    WitherSkullBlock::check_spawn(&fixture.world, BlockPos::new(1, 66, 0));
    assert!(
        withers_near(&fixture, spawn).is_empty(),
        "two skulls build nothing"
    );

    assert!(WitherSkullBlock::can_spawn_mob(
        &fixture.world,
        BlockPos::new(2, 66, 0),
        &ItemStack::new(&vanilla_items::WITHER_SKELETON_SKULL),
    ));
    fixture
        .world
        .set_block(BlockPos::new(2, 66, 0), skull, UpdateFlags::UPDATE_ALL);
    WitherSkullBlock::check_spawn(&fixture.world, BlockPos::new(2, 66, 0));

    let withers = withers_near(&fixture, spawn);
    assert_eq!(withers.len(), 1);
    let wither = withers[0]
        .downcast_ref::<WitherBossEntity>()
        .expect("a wither");
    assert!(wither.position().distance(spawn) < 1e-6);
    assert_eq!(
        wither.rotation().0,
        90.0,
        "a T across X looks along Z: yaw 90"
    );
    assert_eq!(wither.invulnerable_ticks(), INVULNERABLE_TICKS);
    assert!((wither.get_health() - wither.get_max_health() / 3.0).abs() < 1e-3);
    for pos in [
        BlockPos::new(1, 64, 0),
        BlockPos::new(1, 65, 0),
        BlockPos::new(2, 66, 0),
    ] {
        assert!(
            fixture.world.get_block_state(pos).is_air(),
            "{pos:?} is cleared"
        );
    }
}

#[test]
fn peaceful_builds_nothing() {
    let fixture = world("wither_build_peaceful");
    build_base(&fixture);
    fixture.world.set_difficulty(Difficulty::Peaceful);
    assert!(!WitherSkullBlock::can_spawn_mob(
        &fixture.world,
        BlockPos::new(2, 66, 0),
        &ItemStack::new(&vanilla_items::WITHER_SKELETON_SKULL),
    ));
}

#[test]
fn charging_wither_ignores_damage_then_explodes_awake() {
    let fixture = world("wither_charge");
    let (wither, _shared) = add_wither(&fixture);
    wither.make_invulnerable();
    let attack = DamageSource::environment(&vanilla_damage_types::GENERIC);
    assert!(!wither.hurt_server(&fixture.world, &attack, 5.0));
    let kill = DamageSource::environment(&vanilla_damage_types::GENERIC_KILL);
    assert!(
        wither.hurt_server(&fixture.world, &kill, 1.0),
        "/kill still works"
    );

    let health = wither.get_health();
    wither.set_invulnerable_ticks(1);
    wither.custom_server_ai_step();
    assert_eq!(wither.invulnerable_ticks(), 0);
    assert!(wither.get_health() >= health);
    assert!(wither.hurt_server(&fixture.world, &attack, 5.0));
}

#[test]
fn undead_cannot_hurt_the_wither() {
    let fixture = world("wither_friends");
    let (wither, _shared) = add_wither(&fixture);
    let zombie: SharedEntity = Arc::new(ZombieEntity::new(
        &vanilla_entities::ZOMBIE,
        next_entity_id(),
        DVec3::new(2.5, 100.0, 0.5),
        Arc::downgrade(&fixture.world),
    ));
    let source = DamageSource::direct(&vanilla_damage_types::MOB_ATTACK, zombie);
    assert!(!wither.hurt_server(&fixture.world, &source, 5.0));
    assert!(!wither.hurt_server(
        &fixture.world,
        &DamageSource::environment(&vanilla_damage_types::DROWN),
        5.0,
    ));
}

#[test]
fn effects_do_not_stick() {
    let fixture = world("wither_effects");
    let (wither, _shared) = add_wither(&fixture);
    assert!(!wither.add_mob_effect_with_source(
        MobEffectInstance::new(vanilla_mob_effects::SLOWNESS, 0),
        None,
    ));
    assert!(!wither.can_be_affected(&MobEffectInstance::new(vanilla_mob_effects::WITHER, 0)));
}

#[test]
fn peaceful_despawns_the_wither() {
    let fixture = world("wither_peaceful");
    let (wither, _shared) = add_wither(&fixture);
    wither.check_despawn();
    assert!(!wither.is_removed(), "hard keeps the wither");
    fixture.world.set_difficulty(Difficulty::Peaceful);
    wither.check_despawn();
    assert!(wither.is_removed());
}

#[test]
fn invul_survives_save_and_load() {
    let fixture = world("wither_save");
    let (wither, _shared) = add_wither(&fixture);
    wither.set_invulnerable_ticks(123);
    let mut saved = NbtCompound::new();
    wither.save_additional(&mut saved);
    let mut bytes = Vec::new();
    saved.write(&mut bytes);
    let borrowed = read_compound(&mut Cursor::new(&bytes))
        .unwrap_or_else(|error| panic!("test nbt should reborrow: {error}"));
    let (loaded, _loaded_shared) = add_wither(&fixture);
    loaded.load_additional((&borrowed).into());
    assert_eq!(loaded.invulnerable_ticks(), 123);
}
