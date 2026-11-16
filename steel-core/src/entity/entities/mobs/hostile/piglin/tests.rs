//! Piglin and piglin brute tests.

use std::io::Cursor;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::read_compound;
use simdnbt::owned::NbtCompound;
use steel_registry::entity_type::EntityType;
use steel_registry::item_stack::ItemStack;
use steel_registry::{
    init_vanilla_registry, vanilla_attributes, vanilla_blocks, vanilla_dimension_types,
    vanilla_entities, vanilla_items,
};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos, WorldAabb};

use super::{AbstractPiglin as _, PiglinBruteEntity, PiglinEntity, anger_nearby_piglins};
use crate::behavior::init_behaviors;
use crate::entity::ai::brain::memory_types::{ADMIRING_ITEM, ANGRY_AT};
use crate::entity::entities::HoglinEntity;
use crate::entity::{
    Entity, LivingEntity as _, Mob as _, SharedEntity, init_entities, next_entity_id,
};
use crate::inventory::container::Container as _;
use crate::inventory::equipment::EquipmentSlot;
use crate::player::ResetReason;
use crate::test_support::{
    TestPlayerBuilder, TestWorld, fresh_test_world, fresh_test_world_with_dimension_type,
    insert_ready_full_chunk,
};
use crate::world::World;

const AREA: WorldAabb = WorldAabb::new(0.0, 60.0, 0.0, 16.0, 80.0, 16.0);

fn detached_piglin() -> PiglinEntity {
    PiglinEntity::new(&vanilla_entities::PIGLIN, 1, DVec3::ZERO, Weak::new())
}

fn with_floor(world_fixture: TestWorld) -> TestWorld {
    let world = &world_fixture.world;
    insert_ready_full_chunk(world, ChunkPos::new(0, 0));
    let stone = vanilla_blocks::STONE.default_state();
    for x in 0..16 {
        for z in 0..16 {
            world.set_block(BlockPos::new(x, 64, z), stone, UpdateFlags::UPDATE_ALL);
        }
    }
    world_fixture
}

fn nether_world(name: &'static str) -> TestWorld {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    with_floor(fresh_test_world_with_dimension_type(
        "piglin",
        name,
        &vanilla_dimension_types::THE_NETHER,
    ))
}

fn add_piglin(world: &Arc<World>, x: f64) -> Arc<PiglinEntity> {
    let piglin = Arc::new(PiglinEntity::new(
        &vanilla_entities::PIGLIN,
        next_entity_id(),
        DVec3::new(x, 65.0, 8.5),
        Arc::downgrade(world),
    ));
    let entity: SharedEntity = Arc::<PiglinEntity>::clone(&piglin);
    world
        .try_add_entity(entity)
        .expect("piglin should attach to the loaded chunk");
    piglin
}

fn tick(piglin: &Arc<PiglinEntity>) {
    let entity: SharedEntity = Arc::<PiglinEntity>::clone(piglin);
    entity.tick();
}

fn entities_of(world: &World, key: &EntityType) -> usize {
    world
        .get_entities_in_aabb(&AREA)
        .iter()
        .filter(|entity| entity.entity_type().key == key.key)
        .count()
}

fn round_trip(save: &dyn Entity, load: &dyn Entity) {
    let mut nbt = NbtCompound::new();
    save.save_additional(&mut nbt);
    let mut bytes = Vec::new();
    nbt.write(&mut bytes);
    let borrowed = read_compound(&mut Cursor::new(&bytes))
        .unwrap_or_else(|error| panic!("reborrow failed: {error}"));
    load.load_additional((&borrowed).into());
}

#[test]
fn a_baby_is_smaller_and_faster() {
    init_vanilla_registry();
    let piglin = detached_piglin();
    let adult_height = piglin.dimensions_for_pose(piglin.pose()).height;
    let adult_speed = piglin
        .attributes()
        .lock()
        .required_value(vanilla_attributes::MOVEMENT_SPEED);
    piglin.set_baby(true);
    assert!(piglin.is_baby());
    assert!(piglin.dimensions_for_pose(piglin.pose()).height < adult_height);
    let baby_speed = piglin
        .attributes()
        .lock()
        .required_value(vanilla_attributes::MOVEMENT_SPEED);
    assert!((baby_speed - adult_speed * 1.2).abs() < 1.0e-9);
}

#[test]
fn saved_state_round_trips() {
    init_vanilla_registry();
    let piglin = detached_piglin();
    piglin.set_baby(true);
    piglin.set_immune_to_zombification(true);
    let _ = piglin
        .inventory()
        .lock()
        .add_item(&ItemStack::with_count(&vanilla_items::GOLD_NUGGET, 7));
    piglin.set_can_pick_up_loot(false);
    let loaded = detached_piglin();
    loaded.set_can_pick_up_loot(false);
    round_trip(&piglin, &loaded);
    assert!(loaded.is_baby());
    assert!(loaded.is_immune_to_zombification());
    assert!(!loaded.can_pick_up_loot());
    let nuggets = loaded.inventory().lock().get_item(0).clone();
    assert!(nuggets.is(&vanilla_items::GOLD_NUGGET));
    assert_eq!(nuggets.count(), 7);
}

#[test]
fn wants_gold_but_not_soul_fire_or_a_second_loved_item() {
    init_vanilla_registry();
    let piglin = detached_piglin();
    assert!(piglin.wants_to_pickup(&ItemStack::new(&vanilla_items::GOLD_INGOT)));
    assert!(piglin.wants_to_pickup(&ItemStack::new(&vanilla_items::GOLDEN_APPLE)));
    assert!(!piglin.wants_to_pickup(&ItemStack::new(&vanilla_items::SOUL_TORCH)));
    piglin.set_item_slot(
        EquipmentSlot::OffHand,
        ItemStack::new(&vanilla_items::GOLD_BLOCK),
    );
    assert!(!piglin.wants_to_pickup(&ItemStack::new(&vanilla_items::GOLD_INGOT)));
}

#[test]
fn piglins_prefer_crossbows_and_gold_over_stronger_items() {
    init_vanilla_registry();
    let piglin = detached_piglin();
    let crossbow = ItemStack::new(&vanilla_items::CROSSBOW);
    let netherite = ItemStack::new(&vanilla_items::NETHERITE_SWORD);
    assert!(!piglin.can_replace_current_item(&netherite, &crossbow, EquipmentSlot::MainHand));
    assert!(piglin.can_replace_current_item(&crossbow, &netherite, EquipmentSlot::MainHand));
}

#[test]
fn bartering_a_gold_ingot_throws_loot() {
    let world_fixture = nether_world("barter");
    let world = &world_fixture.world;
    let piglin = add_piglin(world, 8.5);
    let ingot = world
        .spawn_item(
            DVec3::new(8.5, 65.0, 8.5),
            ItemStack::new(&vanilla_items::GOLD_INGOT),
        )
        .expect("the ingot spawns");
    piglin.pick_up_item_piglin(world, &ingot);
    assert!(ingot.is_removed());
    assert!(
        piglin
            .equipment_in_slot(EquipmentSlot::OffHand)
            .is(&vanilla_items::GOLD_INGOT)
    );
    assert!(piglin.memories().lock().has_value(&ADMIRING_ITEM));
    assert!(
        !piglin.is_persistence_required(),
        "a gold ingot keeps despawning on"
    );

    for _ in 0..130 {
        tick(&piglin);
    }
    assert!(piglin.equipment_in_slot(EquipmentSlot::OffHand).is_empty());
    assert!(
        entities_of(world, &vanilla_entities::ITEM) > 0,
        "the piglin throws its barter loot"
    );
}

#[test]
fn opening_a_container_angers_idle_piglins() {
    let world_fixture = nether_world("anger");
    let world = &world_fixture.world;
    let piglin = add_piglin(world, 8.5);
    tick(&piglin);
    let player = TestPlayerBuilder::new(Arc::clone(world), "Looter", next_entity_id()).build();
    player.snap_to(DVec3::new(4.5, 65.0, 8.5), 0.0, 0.0);
    assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));

    anger_nearby_piglins(world, &player, false);
    assert_eq!(piglin.memories().lock().get(&ANGRY_AT), Some(player.uuid()));
}

#[test]
fn a_crossbow_piglin_shoots_the_hoglin_it_is_angry_at() {
    let world_fixture = nether_world("crossbow");
    let world = &world_fixture.world;
    let piglin = add_piglin(world, 3.5);
    piglin.set_item_slot(
        EquipmentSlot::MainHand,
        ItemStack::new(&vanilla_items::CROSSBOW),
    );
    let hoglin: SharedEntity = Arc::new(HoglinEntity::new(
        &vanilla_entities::HOGLIN,
        next_entity_id(),
        DVec3::new(10.5, 65.0, 8.5),
        Arc::downgrade(world),
    ));
    world
        .try_add_entity(Arc::clone(&hoglin))
        .expect("hoglin should attach to the loaded chunk");
    piglin
        .memories()
        .lock()
        .set_with_expiry(&ANGRY_AT, hoglin.uuid(), 600);

    let mut saw_charging = false;
    let mut fired = false;
    for _ in 0..200 {
        tick(&piglin);
        saw_charging |= piglin.is_charging_crossbow();
        fired = entities_of(world, &vanilla_entities::ARROW) > 0;
        if fired {
            break;
        }
    }
    assert!(saw_charging, "the piglin charges its crossbow first");
    assert!(fired, "the charged crossbow fires at the hoglin");
}

#[test]
fn zombifies_outside_the_nether() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    let world_fixture = with_floor(fresh_test_world("piglin_zombifies"));
    let world = &world_fixture.world;
    let piglin = add_piglin(world, 8.5);
    let _ = piglin
        .inventory()
        .lock()
        .add_item(&ItemStack::new(&vanilla_items::GOLD_NUGGET));
    assert!(piglin.is_converting());
    for _ in 0..302 {
        if piglin.is_removed() {
            break;
        }
        tick(&piglin);
    }
    assert!(piglin.is_removed());
    assert_eq!(entities_of(world, &vanilla_entities::ZOMBIFIED_PIGLIN), 1);
    assert!(
        entities_of(world, &vanilla_entities::ITEM) > 0,
        "the inventory drops on conversion"
    );
}

#[test]
fn brutes_only_pick_up_golden_axes_and_never_hunt() {
    init_vanilla_registry();
    let brute =
        PiglinBruteEntity::new(&vanilla_entities::PIGLIN_BRUTE, 1, DVec3::ZERO, Weak::new());
    assert!(brute.wants_to_pick_up(&ItemStack::new(&vanilla_items::GOLDEN_AXE)));
    assert!(!brute.wants_to_pick_up(&ItemStack::new(&vanilla_items::GOLD_INGOT)));
    assert!(!brute.can_hunt());
}
