use std::sync::Arc;

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction};
use steel_registry::data_components::vanilla_components::{POTION_CONTENTS, PotionContents};
use steel_registry::item_stack::ItemStack;
use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_items, vanilla_potions};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, ChunkPos};

use crate::behavior::dispense::{BlockSource, DispenseBehavior};
use crate::behavior::init_behaviors;
use crate::block_entity::entities::DispenserBlockEntity;
use crate::entity::init_entities;
use crate::test_support::{fresh_test_world, insert_ready_full_chunk};
use crate::world::World;

const DISPENSER_POS: BlockPos = BlockPos::new(4, 64, 4);
const FRONT: BlockPos = BlockPos::new(5, 64, 4);

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

fn put_in_front(world: &Arc<World>, state: BlockStateId) {
    world.set_block(FRONT, state, UpdateFlags::UPDATE_ALL);
}

#[test]
fn glowstone_charges_the_respawn_anchor_in_front() {
    let world = world("dispense_glowstone");
    put_in_front(&world, vanilla_blocks::RESPAWN_ANCHOR.default_state());
    let rest = dispense(&world, ItemStack::with_count(&vanilla_items::GLOWSTONE, 2));
    assert_eq!(rest.count(), 1);
    assert_eq!(
        world
            .get_block_state(FRONT)
            .get_value(&BlockStateProperties::RESPAWN_ANCHOR_CHARGES),
        1
    );
}

#[test]
fn honeycomb_waxes_the_copper_in_front() {
    let world = world("dispense_honeycomb");
    put_in_front(&world, vanilla_blocks::COPPER_BLOCK.default_state());
    let rest = dispense(&world, ItemStack::new(&vanilla_items::HONEYCOMB));
    assert!(rest.is_empty());
    assert_eq!(
        world.get_block_state(FRONT).get_block(),
        &vanilla_blocks::WAXED_COPPER_BLOCK
    );
}

#[test]
fn a_water_bottle_turns_dirt_into_mud() {
    let world = world("dispense_water_potion");
    put_in_front(&world, vanilla_blocks::DIRT.default_state());
    let water = PotionContents::create_item_stack(&vanilla_items::POTION, &vanilla_potions::WATER);
    let rest = dispense(&world, water);
    assert!(rest.is(&vanilla_items::GLASS_BOTTLE));
    assert_eq!(
        world.get_block_state(FRONT).get_block(),
        &vanilla_blocks::MUD
    );
}

#[test]
fn a_glass_bottle_fills_with_water_in_front() {
    let world = world("dispense_glass_bottle");
    put_in_front(&world, vanilla_blocks::WATER.default_state());
    let rest = dispense(&world, ItemStack::new(&vanilla_items::GLASS_BOTTLE));
    assert!(rest.is(&vanilla_items::POTION));
    assert!(
        rest.get_or_default(POTION_CONTENTS, PotionContents::empty())
            .is(&vanilla_potions::WATER)
    );
}

#[test]
fn a_powder_snow_bucket_places_powder_snow() {
    let world = world("dispense_powder_snow");
    let rest = dispense(&world, ItemStack::new(&vanilla_items::POWDER_SNOW_BUCKET));
    assert!(rest.is(&vanilla_items::BUCKET));
    assert_eq!(
        world.get_block_state(FRONT).get_block(),
        &vanilla_blocks::POWDER_SNOW
    );
}

#[test]
fn flint_and_steel_wears_down_even_against_solid_stone() {
    let world = world("dispense_flint_blocked");
    put_in_front(&world, vanilla_blocks::STONE.default_state());
    let rest = dispense(&world, ItemStack::new(&vanilla_items::FLINT_AND_STEEL));
    assert_eq!(
        world.get_block_state(FRONT).get_block(),
        &vanilla_blocks::STONE
    );
    assert_eq!(rest.get_damage_value(), 1);
}

#[test]
fn flint_and_steel_sets_fire_to_air_in_front() {
    let world = world("dispense_flint_fire");
    world.set_block(
        FRONT.below(),
        vanilla_blocks::STONE.default_state(),
        UpdateFlags::UPDATE_ALL,
    );
    let rest = dispense(&world, ItemStack::new(&vanilla_items::FLINT_AND_STEEL));
    assert_eq!(
        world.get_block_state(FRONT).get_block(),
        &vanilla_blocks::FIRE
    );
    assert_eq!(rest.get_damage_value(), 1);
}
