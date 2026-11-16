use std::io::Cursor;
use std::sync::Arc;

use glam::DVec3;
use simdnbt::borrow::read_compound;
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_entities, vanilla_items};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos, Downcast as _, WorldAabb};

use super::goals::TurtleLayEggGoal;
use super::{BABY_SCALE, HATCHLING_AGE, TurtleEntity, spawn_hatchling};
use crate::behavior::init_behaviors;
use crate::block_entity::init_block_entities;
use crate::entity::ai::goal::Goal as _;
use crate::entity::entities::ItemEntity;
use crate::entity::{
    AgeableMob, Animal as _, Entity, EntitySpawnReason, LivingEntity as _, PathfinderMob as _,
    SharedEntity, init_entities, next_entity_id,
};
use crate::test_support::{TestWorld, fresh_test_world, insert_ready_full_chunk};
use crate::world::LevelReader as _;

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
    fixture
}

fn set(fixture: &TestWorld, pos: BlockPos, block: BlockRef) {
    fixture
        .world
        .set_block(pos, block.default_state(), UpdateFlags::UPDATE_NONE);
}

fn add_turtle(fixture: &TestWorld, position: DVec3) -> (Arc<TurtleEntity>, SharedEntity) {
    let turtle = Arc::new(TurtleEntity::new(
        &vanilla_entities::TURTLE,
        next_entity_id(),
        position,
        Arc::downgrade(&fixture.world),
    ));
    let shared: SharedEntity = Arc::<TurtleEntity>::clone(&turtle);
    fixture
        .world
        .try_add_entity(Arc::clone(&shared))
        .expect("the turtle attaches to the loaded chunk");
    (turtle, shared)
}

fn turtles_near(fixture: &TestWorld, center: DVec3) -> Vec<SharedEntity> {
    fixture.world.get_entities_in_aabb_matching(
        &WorldAabb::from_min_max(center - 2.0, center + 2.0),
        |entity| entity.downcast_ref::<TurtleEntity>().is_some(),
    )
}

#[test]
fn hatchlings_are_tiny_babies_homed_on_the_nest() {
    let fixture = world("turtle_hatch");
    let nest = BlockPos::new(3, 64, 3);
    spawn_hatchling(&fixture.world, nest, 1);
    let turtles = turtles_near(&fixture, DVec3::new(3.5, 64.0, 3.5));
    assert_eq!(turtles.len(), 1);
    let turtle = turtles[0].downcast_ref::<TurtleEntity>().expect("a turtle");
    assert!(AgeableMob::is_baby(turtle));
    assert_eq!(turtle.get_age(), HATCHLING_AGE);
    assert_eq!(turtle.home_pos(), nest);
    assert!(turtle.position().distance(DVec3::new(3.5, 64.0, 3.3)) < 1e-6);
    let width = turtle.bounding_box().max_x() - turtle.bounding_box().min_x();
    assert!((width - f64::from(1.2 * BABY_SCALE)).abs() < 1e-3);
    assert!((turtle.get_age_scale() - BABY_SCALE).abs() < f32::EPSILON);
}

#[test]
fn baby_rider_anchor_uses_the_full_height() {
    init_vanilla_registry();
    assert!((vanilla_entities::TURTLE.dimensions.height - super::TURTLE_HEIGHT).abs() < 1e-6);
}

#[test]
fn home_and_egg_survive_save_and_load() {
    let fixture = world("turtle_save");
    let (turtle, _shared) = add_turtle(&fixture, DVec3::new(0.5, 70.0, 0.5));
    turtle.set_home_pos(BlockPos::new(10, 62, -4));
    turtle.set_has_egg(true);
    let mut saved = NbtCompound::new();
    turtle.save_additional(&mut saved);
    let mut bytes = Vec::new();
    saved.write(&mut bytes);
    let borrowed = read_compound(&mut Cursor::new(&bytes))
        .unwrap_or_else(|error| panic!("test nbt should reborrow: {error}"));
    let (loaded, _loaded_shared) = add_turtle(&fixture, DVec3::new(2.5, 70.0, 2.5));
    loaded.load_additional((&borrowed).into());
    assert_eq!(loaded.home_pos(), BlockPos::new(10, 62, -4));
    assert!(loaded.has_egg());

    let (fresh, _fresh_shared) = add_turtle(&fixture, DVec3::new(5.5, 70.0, 5.5));
    fresh.load_additional(
        (&read_compound(&mut Cursor::new(&{
            let mut empty = Vec::new();
            NbtCompound::new().write(&mut empty);
            empty
        }))
        .expect("empty compound"))
            .into(),
    );
    assert_eq!(
        fresh.home_pos(),
        BlockPos::new(5, 70, 5),
        "home defaults to here"
    );
}

#[test]
fn mating_gives_an_egg_instead_of_a_baby() {
    let fixture = world("turtle_mate");
    let (mother, _a) = add_turtle(&fixture, DVec3::new(0.5, 70.0, 0.5));
    let (father, _b) = add_turtle(&fixture, DVec3::new(1.5, 70.0, 0.5));
    mother.set_in_love(None);
    father.set_in_love(None);
    mother.spawn_child_from_breeding(&fixture.world, father.as_ref());
    assert!(mother.has_egg());
    assert!(!father.has_egg());
    assert_eq!(mother.get_age(), 6000);
    assert_eq!(father.get_age(), 6000);
    assert!(!mother.is_in_love() && !father.is_in_love());
    assert_eq!(turtles_near(&fixture, DVec3::new(1.0, 70.0, 0.5)).len(), 2);
    assert!(
        !mother.can_fall_in_love(),
        "no new love while carrying an egg"
    );
}

#[test]
fn growing_up_sheds_a_scute() {
    let fixture = world("turtle_grow");
    let (turtle, _shared) = add_turtle(&fixture, DVec3::new(0.5, 70.0, 0.5));
    turtle.set_age(-1);
    turtle.set_age(0);
    let scutes = fixture.world.get_entities_in_aabb_matching(
        &WorldAabb::from_min_max(DVec3::new(-2.0, 68.0, -2.0), DVec3::new(3.0, 73.0, 3.0)),
        |entity| {
            entity
                .downcast_ref::<ItemEntity>()
                .is_some_and(|item| item.get_item().is(&vanilla_items::TURTLE_SCUTE))
        },
    );
    assert_eq!(scutes.len(), 1);
}

#[test]
fn walk_targets_favor_water_and_sand() {
    let fixture = world("turtle_walk_target");
    let (turtle, _shared) = add_turtle(&fixture, DVec3::new(0.5, 70.0, 0.5));
    set(&fixture, BlockPos::new(4, 63, 4), &vanilla_blocks::WATER);
    set(&fixture, BlockPos::new(6, 63, 6), &vanilla_blocks::SAND);
    assert!((turtle.get_walk_target_value(BlockPos::new(4, 63, 4)) - 10.0).abs() < 1e-6);
    assert!((turtle.get_walk_target_value(BlockPos::new(6, 64, 6)) - 10.0).abs() < 1e-6);
    turtle.set_going_home(true);
    assert!(turtle.get_walk_target_value(BlockPos::new(4, 63, 4)) < 10.0);
}

#[test]
fn stable_destinations_follow_the_travel_target() {
    let fixture = world("turtle_stable");
    let (turtle, _shared) = add_turtle(&fixture, DVec3::new(0.5, 70.0, 0.5));
    set(&fixture, BlockPos::new(2, 63, 2), &vanilla_blocks::SAND);
    set(&fixture, BlockPos::new(3, 64, 3), &vanilla_blocks::WATER);
    assert!(turtle.is_stable_destination(BlockPos::new(2, 64, 2)));
    assert!(!turtle.is_stable_destination(BlockPos::new(2, 66, 2)));
    turtle.set_travel_pos(Some(BlockPos::new(500, 60, 0)));
    assert!(!turtle.is_stable_destination(BlockPos::new(2, 64, 2)));
    assert!(turtle.is_stable_destination(BlockPos::new(3, 64, 3)));
}

#[test]
fn spawns_only_on_low_sand() {
    let fixture = world("turtle_spawn_rule");
    let world = &fixture.world;
    let sea = world.sea_level();
    let rule = TurtleEntity::SPAWN_RULE;
    let low = BlockPos::new(2, sea, 2);
    set(&fixture, low.below(), &vanilla_blocks::GRASS_BLOCK);
    assert!(!rule(
        &vanilla_entities::TURTLE,
        world.as_ref(),
        EntitySpawnReason::Natural,
        low,
        &mut rand::rng(),
    ));
    let high = BlockPos::new(4, sea + 4, 4);
    set(&fixture, high.below(), &vanilla_blocks::SAND);
    assert!(!rule(
        &vanilla_entities::TURTLE,
        world.as_ref(),
        EntitySpawnReason::Natural,
        high,
        &mut rand::rng(),
    ));
}

#[test]
fn a_turtle_home_with_an_egg_digs_and_lays() {
    let fixture = world("turtle_lay");
    let sand = BlockPos::new(2, 63, 2);
    set(&fixture, sand, &vanilla_blocks::SAND);
    let (turtle, shared) = add_turtle(&fixture, DVec3::new(2.5, 64.0, 2.5));
    turtle.set_home_pos(sand.above());
    turtle.set_has_egg(true);
    let mut goal = TurtleLayEggGoal::new(1.0);
    assert!(goal.can_use(turtle.as_ref()));
    goal.start(turtle.as_ref());
    let mut ticks = 0;
    while turtle.has_egg() && ticks < 400 {
        goal.tick(turtle.as_ref(), &shared);
        ticks += 1;
    }
    assert!(!turtle.has_egg(), "the egg was laid after {ticks} ticks");
    assert!(ticks > 200, "digging takes 200 ticks, took {ticks}");
    assert!(!turtle.is_laying_egg());
    assert!(turtle.is_in_love());
    let egg = fixture.world.get_block_state(sand.above());
    assert_eq!(egg.get_block(), &vanilla_blocks::TURTLE_EGG);
    assert!((1..=4).contains(&egg.get_value(&BlockStateProperties::EGGS)));
}
