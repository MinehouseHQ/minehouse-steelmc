use glam::DVec3;
use steel_registry::blocks::properties::Direction;
use steel_registry::data_components::vanilla_components::{CUSTOM_NAME, DyedItemColor};
use steel_registry::init_vanilla_registry;
use steel_registry::items::item::BlockHitResult;
use steel_utils::ChunkPos;
use steel_utils::types::GameType;
use text_components::TextComponent;

use super::*;
use crate::behavior::{BLOCK_BEHAVIORS, init_behaviors};
use crate::test_support::{
    TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk,
};

const POS: BlockPos = BlockPos::new(8, 64, 8);

fn cauldron_world(name: &'static str, state: BlockStateId) -> TestWorld {
    init_vanilla_registry();
    init_behaviors();
    let fixture = fresh_test_world(name);
    insert_ready_full_chunk(&fixture.world, ChunkPos::from_block_pos(POS));
    assert!(fixture.world.set_block(POS, state, UpdateFlags::UPDATE_ALL));
    fixture
}

fn water(level: u8) -> BlockStateId {
    vanilla_blocks::WATER_CAULDRON
        .default_state()
        .set_value(LEVEL_CAULDRON, level)
}

/// Uses `held` on the cauldron through its registered behavior, survival mode.
fn use_on(fixture: &TestWorld, held: ItemStack) -> (InteractionResult, ItemStack, Arc<Player>) {
    let world = &fixture.world;
    let player = TestPlayerBuilder::new(Arc::clone(world), "Cauldron", 41).build();
    player.set_game_mode(GameType::Survival);
    player.inventory.lock().set_selected_item(held);
    let mut inv = InventoryAccess::new(Arc::clone(&player.inventory), InteractionHand::MainHand);
    let state = world.get_block_state(POS);
    let hit = BlockHitResult {
        location: DVec3::new(8.5, 65.0, 8.5),
        direction: Direction::Up,
        block_pos: POS,
        miss: false,
        inside: false,
        world_border_hit: false,
    };
    let result = BLOCK_BEHAVIORS.get_behavior(state.get_block()).use_item_on(
        state,
        world,
        POS,
        &player,
        InteractionHand::MainHand,
        &hit,
        &mut inv,
    );
    let in_hand = player.inventory.lock().get_selected_item().clone();
    (result, in_hand, player)
}

#[test]
fn water_bucket_fills_an_empty_cauldron() {
    init_vanilla_registry();
    let fixture = cauldron_world(
        "cauldron_water_bucket",
        vanilla_blocks::CAULDRON.default_state(),
    );
    let (result, in_hand, _) = use_on(&fixture, ItemStack::new(&vanilla_items::WATER_BUCKET));
    assert_eq!(result, InteractionResult::Success);
    assert!(in_hand.is(&vanilla_items::BUCKET));
    assert_eq!(fixture.world.get_block_state(POS), water(3));
}

#[test]
fn bucket_takes_lava_and_only_full_water() {
    init_vanilla_registry();
    let fixture = cauldron_world(
        "cauldron_lava",
        vanilla_blocks::LAVA_CAULDRON.default_state(),
    );
    let (result, in_hand, _) = use_on(&fixture, ItemStack::new(&vanilla_items::BUCKET));
    assert_eq!(result, InteractionResult::Success);
    assert!(in_hand.is(&vanilla_items::LAVA_BUCKET));
    assert_eq!(
        fixture.world.get_block_state(POS),
        vanilla_blocks::CAULDRON.default_state()
    );

    let fixture = cauldron_world("cauldron_partial_water", water(2));
    let (result, in_hand, _) = use_on(&fixture, ItemStack::new(&vanilla_items::BUCKET));
    assert_eq!(result, InteractionResult::TryEmptyHandInteraction);
    assert!(in_hand.is(&vanilla_items::BUCKET));
    assert_eq!(fixture.world.get_block_state(POS), water(2));
}

#[test]
fn bottles_move_water_one_level_at_a_time() {
    init_vanilla_registry();
    let fixture = cauldron_world("cauldron_bottle", water(1));
    let (result, in_hand, _) = use_on(&fixture, ItemStack::new(&vanilla_items::GLASS_BOTTLE));
    assert_eq!(result, InteractionResult::Success);
    assert!(
        in_hand
            .get(POTION_CONTENTS)
            .is_some_and(|potion| potion.is(&vanilla_potions::WATER))
    );
    assert_eq!(
        fixture.world.get_block_state(POS),
        vanilla_blocks::CAULDRON.default_state()
    );

    let (result, in_hand, _) = use_on(&fixture, in_hand);
    assert_eq!(result, InteractionResult::Success);
    assert!(in_hand.is(&vanilla_items::GLASS_BOTTLE));
    assert_eq!(fixture.world.get_block_state(POS), water(1));

    let potion = PotionContents::create_item_stack(&vanilla_items::POTION, &vanilla_potions::WATER);
    let (result, _, _) = use_on(&fixture, potion);
    assert_eq!(result, InteractionResult::Success);
    assert_eq!(fixture.world.get_block_state(POS), water(2));
}

#[test]
fn water_removes_dye_and_drains_a_level() {
    init_vanilla_registry();
    let fixture = cauldron_world("cauldron_dye", water(3));
    let mut boots = ItemStack::new(&vanilla_items::LEATHER_BOOTS);
    boots.set(DYED_COLOR, DyedItemColor::new(0x00_FF_00));
    let (result, in_hand, _) = use_on(&fixture, boots);
    assert_eq!(result, InteractionResult::Success);
    assert!(in_hand.is(&vanilla_items::LEATHER_BOOTS));
    assert!(!in_hand.has(DYED_COLOR));
    assert_eq!(fixture.world.get_block_state(POS), water(2));

    let (result, _, _) = use_on(&fixture, ItemStack::new(&vanilla_items::LEATHER_BOOTS));
    assert_eq!(result, InteractionResult::TryEmptyHandInteraction);
}

#[test]
fn water_washes_a_dyed_shulker_box_keeping_its_name() {
    init_vanilla_registry();
    let fixture = cauldron_world("cauldron_shulker", water(1));
    let mut boxed = ItemStack::new(&vanilla_items::RED_SHULKER_BOX);
    boxed.set(CUSTOM_NAME, TextComponent::plain("Loot"));
    let (result, in_hand, _) = use_on(&fixture, boxed);
    assert_eq!(result, InteractionResult::Success);
    assert!(in_hand.is(&vanilla_items::SHULKER_BOX));
    assert!(in_hand.has(CUSTOM_NAME));
    assert_eq!(
        fixture.world.get_block_state(POS),
        vanilla_blocks::CAULDRON.default_state()
    );

    let fixture = cauldron_world("cauldron_plain_shulker", water(1));
    let (result, _, _) = use_on(&fixture, ItemStack::new(&vanilla_items::SHULKER_BOX));
    assert_eq!(result, InteractionResult::TryEmptyHandInteraction);
}

#[test]
fn lava_cannot_be_poured_under_water() {
    init_vanilla_registry();
    let fixture = cauldron_world(
        "cauldron_under_water",
        vanilla_blocks::CAULDRON.default_state(),
    );
    assert!(fixture.world.set_block(
        POS.above(),
        vanilla_blocks::WATER.default_state(),
        UpdateFlags::UPDATE_ALL
    ));
    let (result, in_hand, _) = use_on(&fixture, ItemStack::new(&vanilla_items::LAVA_BUCKET));
    assert_eq!(result, InteractionResult::Consume);
    assert!(in_hand.is(&vanilla_items::LAVA_BUCKET));
}
