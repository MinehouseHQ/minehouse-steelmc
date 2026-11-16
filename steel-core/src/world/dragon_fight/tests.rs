use std::sync::Arc;

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::{
    init_vanilla_registry, vanilla_blocks, vanilla_dimension_types, vanilla_entities,
};
use steel_utils::{BlockPos, ChunkPos, Downcast as _};

use super::portal::{exit_portal_pattern, gateway_pos};
use super::{DragonRespawnStage, EnderDragonFight, SavedDragonFight};
use crate::behavior::init_behaviors;
use crate::block_entity::init_block_entities;
use crate::entity::entities::{EndCrystalEntity, EnderDragonEntity};
use crate::entity::{
    Entity as _, LivingEntity as _, RemovalReason, SharedEntity, init_entities, next_entity_id,
};
use crate::player::ResetReason;
use crate::test_support::{
    TestPlayerBuilder, TestWorld, fresh_test_world_with_dimension_type, insert_ready_full_chunk,
};

const SEED: i64 = 12345;

fn end_world(name: &'static str) -> TestWorld {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
    let fixture = fresh_test_world_with_dimension_type(
        "dragon_fight",
        name,
        &vanilla_dimension_types::THE_END,
    );
    for x in -1..=1 {
        for z in -1..=1 {
            insert_ready_full_chunk(&fixture.world, ChunkPos::new(x, z));
        }
    }
    fixture
}

fn fight() -> EnderDragonFight {
    let fight = EnderDragonFight::from_saved(&SavedDragonFight::default(), SEED, BlockPos::ZERO);
    fight.skip_arena_loaded_check();
    fight
}

/// A fight that already scanned the world, as after its first ticks.
fn scanned_fight() -> EnderDragonFight {
    let fight = EnderDragonFight::from_saved(
        &SavedDragonFight {
            needs_state_scanning: false,
            ..SavedDragonFight::default()
        },
        SEED,
        BlockPos::ZERO,
    );
    fight.skip_arena_loaded_check();
    fight
}

fn add_player_near_spawn(fixture: &TestWorld) {
    let player =
        TestPlayerBuilder::new(Arc::clone(&fixture.world), "Slayer", next_entity_id()).build();
    player.snap_to(DVec3::new(0.5, 100.0, 0.5), 0.0, 0.0);
    assert!(
        fixture
            .world
            .add_player(Arc::clone(&player), ResetReason::InitialJoin)
    );
}

fn dragon_of(fixture: &TestWorld, fight: &EnderDragonFight) -> SharedEntity {
    let uuid = fight.dragon_uuid().expect("the fight spawned a dragon");
    fixture
        .world
        .get_entity_by_uuid(&uuid)
        .expect("the fight's dragon is in the world")
}

fn as_dragon(shared: &SharedEntity) -> &EnderDragonEntity {
    shared
        .downcast_ref::<EnderDragonEntity>()
        .expect("the fight's dragon is an Ender Dragon")
}

fn tick_until_player_scan(fixture: &TestWorld, fight: &EnderDragonFight) {
    // The first player scan runs on the first tick, as vanilla starts the
    // counter at 21.
    fight.tick(&fixture.world);
}

#[test]
fn respawn_stages_save_under_vanilla_names() {
    let saved = SavedDragonFight {
        respawn_stage: Some(DragonRespawnStage::PreparingToSummonPillars),
        ..SavedDragonFight::default()
    };
    let text = toml::to_string(&saved).expect("the fight serializes");
    assert!(text.contains("\"preparing_to_summon_pillars\""), "{text}");
    let back: SavedDragonFight = toml::from_str(&text).expect("the fight deserializes");
    assert_eq!(back, saved);
    let empty: SavedDragonFight = toml::from_str("").expect("an empty file is a new fight");
    assert_eq!(empty, SavedDragonFight::default());
    assert!(empty.needs_state_scanning);
}

#[test]
fn gateways_are_a_seeded_shuffle_of_the_ring() {
    let saved = fight().to_saved();
    let mut sorted = saved.gateways.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, (0..20).collect::<Vec<_>>());
    assert_eq!(saved.gateways, fight().to_saved().gateways);
    assert_ne!(saved.gateways, (0..20).collect::<Vec<_>>());

    assert_eq!(gateway_pos(0), BlockPos::new(96, 75, 0));
    assert_eq!(gateway_pos(10), BlockPos::new(-96, 75, -1));
}

#[test]
fn exit_portal_pattern_has_vanilla_dimensions() {
    let pattern = exit_portal_pattern();
    assert_eq!(
        (pattern.width(), pattern.height(), pattern.depth()),
        (7, 7, 5)
    );
}

#[test]
fn fight_spawns_its_dragon_for_nearby_players() {
    let fixture = end_world("spawns_dragon");
    let fight = fight();
    fight.tick(&fixture.world);
    assert!(
        fight.dragon_uuid().is_none(),
        "nobody is near, so nothing happens"
    );

    add_player_near_spawn(&fixture);
    tick_until_player_scan(&fixture, &fight);
    for _ in 0..20 {
        fight.tick(&fixture.world);
    }
    assert_eq!(fight.dragon_event().players().len(), 1);
    let shared = dragon_of(&fixture, &fight);
    let dragon = as_dragon(&shared);
    assert_eq!(dragon.fight_origin(), BlockPos::ZERO);
    assert!((dragon.position().y - 128.0).abs() < f64::EPSILON);
    assert!(!fight.is_dragon_killed());
    // A world without a portal gets an inactive podium on its first scan.
    assert!(fight.exit_portal_location().is_some());
}

#[test]
fn killing_the_fight_dragon_opens_the_portal_and_places_the_egg() {
    let fixture = end_world("kill_dragon");
    let fight = scanned_fight();
    add_player_near_spawn(&fixture);
    tick_until_player_scan(&fixture, &fight);
    let shared = dragon_of(&fixture, &fight);
    let dragon = as_dragon(&shared);
    dragon.ai_step(&shared);

    fight.update_dragon(dragon);
    assert!((fight.dragon_event().progress() - 1.0).abs() < f32::EPSILON);
    fight.set_dragon_killed(&fixture.world, dragon);

    assert!(fight.is_dragon_killed());
    assert!(fight.has_previously_killed_dragon());
    assert!(!fight.dragon_event().is_visible());
    let portal = fight.exit_portal_location().expect("the portal was placed");
    let egg = (portal.y()..portal.y() + 10)
        .map(|y| BlockPos::new(0, y, 0))
        .find(|pos| fixture.world.get_block_state(*pos).get_block() == &vanilla_blocks::DRAGON_EGG);
    assert!(egg.is_some(), "the first kill leaves the egg on the podium");
    let open = (-3..=3).any(|x| {
        fixture
            .world
            .get_block_state(portal.offset(x, 0, 0))
            .get_block()
            == &vanilla_blocks::END_PORTAL
    });
    assert!(open, "the exit portal is open");
    assert_eq!(fight.to_saved().gateways.len(), 19, "one gateway opened");
}

#[test]
fn four_crystals_on_the_portal_start_the_ritual() {
    let fixture = end_world("respawn");
    let fight = scanned_fight();
    add_player_near_spawn(&fixture);
    tick_until_player_scan(&fixture, &fight);
    let shared = dragon_of(&fixture, &fight);
    fight.set_dragon_killed(&fixture.world, as_dragon(&shared));
    let portal = fight.exit_portal_location().expect("the portal was placed");

    let mut crystals = Vec::new();
    for (index, direction) in steel_utils::Direction::HORIZONTAL.into_iter().enumerate() {
        let pos = portal.above().relative_n(direction, 3);
        let crystal: SharedEntity = Arc::new(EndCrystalEntity::new(
            &vanilla_entities::END_CRYSTAL,
            next_entity_id(),
            DVec3::new(
                f64::from(pos.x()) + 0.5,
                f64::from(pos.y()),
                f64::from(pos.z()) + 0.5,
            ),
            Arc::downgrade(&fixture.world),
        ));
        fixture
            .world
            .try_add_entity(Arc::clone(&crystal))
            .expect("the crystal attaches to the loaded chunk");
        crystals.push(crystal);
        if index < 3 {
            fight.try_respawn(&fixture.world);
            assert_eq!(
                fight.respawn_stage(),
                None,
                "the ritual needs all four crystals"
            );
        }
    }
    fight.try_respawn(&fixture.world);
    assert_eq!(fight.respawn_stage(), Some(DragonRespawnStage::Start));

    fight.tick(&fixture.world);
    assert_eq!(
        fight.respawn_stage(),
        Some(DragonRespawnStage::PreparingToSummonPillars)
    );
    let beam = crystals[0]
        .downcast_ref::<EndCrystalEntity>()
        .and_then(EndCrystalEntity::beam_target);
    assert_eq!(beam, Some(BlockPos::new(0, 128, 0)));

    for crystal in &crystals {
        crystal.set_removed(RemovalReason::Discarded);
    }
    fight.tick(&fixture.world);
    assert_eq!(
        fight.respawn_stage(),
        None,
        "the ritual stops without its crystals"
    );
}
