use std::ptr;
use std::sync::Arc;

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction};
use steel_registry::item_stack::ItemStack;
use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_entities, vanilla_items};
use steel_utils::{BlockPos, ChunkPos, WorldAabb};

use crate::behavior::dispense::{BlockSource, DispenseBehavior};
use crate::behavior::init_behaviors;
use crate::block_entity::entities::DispenserBlockEntity;
use crate::entity::entities::mobs::hostile::ZombieEntity;
use crate::entity::{LivingEntity as _, Mob as _, SharedEntity, init_entities, next_entity_id};
use crate::inventory::prelude::EquipmentSlot;
use crate::test_support::{fresh_test_world, insert_ready_full_chunk};
use crate::world::World;

const DISPENSER_POS: BlockPos = BlockPos::new(4, 64, 4);

fn world(name: &'static str) -> Arc<World> {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    let fixture = fresh_test_world(name);
    insert_ready_full_chunk(&fixture.world, ChunkPos::new(0, 0));
    Arc::clone(&fixture.world)
}

/// Fires `stack` from an east-facing dispenser and returns what stays in the slot.
fn dispense(world: &Arc<World>, stack: ItemStack) -> ItemStack {
    let state = vanilla_blocks::DISPENSER
        .default_state()
        .set_value(&BlockStateProperties::FACING, Direction::East);
    let entity = DispenserBlockEntity::new(Arc::downgrade(world), DISPENSER_POS, state);
    let source = BlockSource {
        world,
        pos: DISPENSER_POS,
        state,
        entity: &entity,
    };
    DispenseBehavior::for_item(&stack).dispense(&source, stack)
}

fn entities_in_front(world: &Arc<World>) -> Vec<SharedEntity> {
    let front = DISPENSER_POS.relative(Direction::East);
    world.get_entities_in_aabb(&WorldAabb::new(
        f64::from(front.x()),
        f64::from(front.y()) - 1.0,
        f64::from(front.z()),
        f64::from(front.x() + 1),
        f64::from(front.y() + 2),
        f64::from(front.z() + 1),
    ))
}

#[test]
fn bootstrap_entries_pick_their_behaviors() {
    init_vanilla_registry();
    init_behaviors();
    let behavior = |item| DispenseBehavior::for_item(&ItemStack::new(item));
    assert!(matches!(
        behavior(&vanilla_items::IRON_CHESTPLATE),
        DispenseBehavior::Equipment
    ));
    assert!(matches!(
        behavior(&vanilla_items::SADDLE),
        DispenseBehavior::Equipment
    ));
    assert!(matches!(
        behavior(&vanilla_items::PIG_SPAWN_EGG),
        DispenseBehavior::SpawnEgg
    ));
    assert!(matches!(
        behavior(&vanilla_items::ARMOR_STAND),
        DispenseBehavior::ArmorStand
    ));
    assert!(matches!(
        behavior(&vanilla_items::CHEST),
        DispenseBehavior::Chest
    ));
}

#[test]
fn a_spawn_egg_spawns_its_mob_in_front() {
    let world = world("dispense_spawn_egg");
    let rest = dispense(
        &world,
        ItemStack::with_count(&vanilla_items::PIG_SPAWN_EGG, 2),
    );
    assert_eq!(rest.count(), 1);
    let spawned = entities_in_front(&world);
    assert_eq!(spawned.len(), 1);
    assert!(ptr::eq(
        spawned[0].entity_type(),
        &raw const vanilla_entities::PIG
    ));
}

#[test]
fn an_armor_stand_is_placed_facing_away_from_the_dispenser() {
    let world = world("dispense_armor_stand");
    let rest = dispense(&world, ItemStack::new(&vanilla_items::ARMOR_STAND));
    assert!(rest.is_empty());
    let placed = entities_in_front(&world);
    assert_eq!(placed.len(), 1);
    assert!(ptr::eq(
        placed[0].entity_type(),
        &raw const vanilla_entities::ARMOR_STAND
    ));
    assert!((placed[0].rotation().0 - 270.0).abs() < f32::EPSILON);
}

#[test]
fn armor_is_put_on_the_mob_in_front() {
    let world = world("dispense_armor");
    let front = DISPENSER_POS.relative(Direction::East);
    let zombie = Arc::new(ZombieEntity::new(
        &vanilla_entities::ZOMBIE,
        next_entity_id(),
        DVec3::new(
            f64::from(front.x()) + 0.5,
            f64::from(front.y()),
            f64::from(front.z()) + 0.5,
        ),
        Arc::downgrade(&world),
    ));
    world
        .try_add_entity(Arc::clone(&zombie) as SharedEntity)
        .expect("zombie should attach to the loaded chunk");
    zombie.set_can_pick_up_loot(true);

    let rest = dispense(&world, ItemStack::new(&vanilla_items::IRON_HELMET));
    assert!(rest.is_empty());
    assert!(
        zombie
            .equipment_in_slot(EquipmentSlot::Head)
            .is(&vanilla_items::IRON_HELMET)
    );
}

#[test]
fn armor_drops_when_nothing_can_wear_it() {
    let world = world("dispense_armor_drop");
    let rest = dispense(&world, ItemStack::new(&vanilla_items::IRON_HELMET));
    assert!(rest.is_empty());
    let dropped = entities_in_front(&world);
    assert_eq!(dropped.len(), 1);
    assert!(ptr::eq(
        dropped[0].entity_type(),
        &raw const vanilla_entities::ITEM
    ));
}
