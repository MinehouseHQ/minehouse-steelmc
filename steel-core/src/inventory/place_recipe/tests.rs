use std::ops::Range;
use std::sync::Arc;

use glam::DVec3;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::recipe::UntypedRecipeRef;
use steel_registry::{REGISTRY, init_vanilla_registry, vanilla_blocks, vanilla_items};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos, Identifier};

use super::{PostPlaceAction, RecipePlacement, place_on_grid};
use crate::behavior::init_behaviors;
use crate::block_entity::entities::FurnaceKind;
use crate::entity::Entity as _;
use crate::inventory::container::{Container as _, SimpleContainer};
use crate::inventory::lock::ContainerRef;
use crate::inventory::menu::Menu;
use crate::inventory::menu::kinds::{crafting, furnace_menu, inventory_menu};
use crate::player::Player;
use crate::test_support::{
    TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk,
};
use steel_utils::locks::IntoShared as _;

fn recipe(key: &'static str) -> UntypedRecipeRef {
    REGISTRY
        .recipes
        .by_key(&Identifier::vanilla_static(key))
        .expect("vanilla recipe")
}

fn placement(key: &'static str, use_max_items: bool) -> RecipePlacement {
    RecipePlacement {
        recipe: recipe(key),
        use_max_items,
        allow_dropping_items_to_clear: false,
    }
}

/// The grid indices each recipe slot lands on.
fn landing(grid: usize, recipe_width: usize, recipe_height: usize) -> Vec<usize> {
    let entries: Vec<usize> = (0..recipe_width * recipe_height).collect();
    let mut landed = vec![usize::MAX; entries.len()];
    place_on_grid(
        grid,
        grid,
        recipe_width,
        recipe_height,
        &entries,
        &mut |entry, index| landed[entry] = index,
    );
    landed
}

#[test]
fn recipes_narrower_or_shorter_than_half_the_grid_are_centered() {
    assert_eq!(landing(3, 2, 2), [0, 1, 3, 4]);
    assert_eq!(landing(3, 1, 2), [1, 4]);
    assert_eq!(landing(3, 1, 1), [4]);
    assert_eq!(landing(3, 3, 1), [3, 4, 5]);
    assert_eq!(landing(3, 3, 2), [0, 1, 2, 3, 4, 5]);
    assert_eq!(landing(3, 3, 3), (0..9).collect::<Vec<_>>());
    assert_eq!(landing(2, 1, 2), [0, 2]);
    assert_eq!(landing(1, 1, 1), [0]);
}

struct Table {
    player: Arc<Player>,
    menu: Menu,
    _world: TestWorld,
}

fn crafting_table(name: &'static str) -> Table {
    init_vanilla_registry();
    init_behaviors();
    let world_fixture = fresh_test_world(name);
    let world = &world_fixture.world;
    let pos = BlockPos::new(0, 64, 0);
    insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
    assert!(world.set_block(
        pos,
        vanilla_blocks::CRAFTING_TABLE.default_state(),
        UpdateFlags::UPDATE_ALL,
    ));
    let player = TestPlayerBuilder::new(Arc::clone(world), "Placer", 1).build();
    player.base().set_position_local(DVec3::new(0.5, 64.0, 0.5));
    let menu = crafting(Arc::clone(&player.inventory), 1, pos);
    Table {
        player,
        menu,
        _world: world_fixture,
    }
}

fn give(player: &Player, slot: usize, item: ItemRef, count: i32) {
    player
        .inventory
        .lock()
        .set_item(slot, ItemStack::with_count(item, count));
}

fn menu_counts(menu: &Menu, slots: Range<usize>) -> Vec<i32> {
    let guard = menu.behavior().lock_all_containers();
    slots
        .map(|slot| menu.behavior().slots()[slot].get_item(&guard).count())
        .collect()
}

fn result(menu: &Menu) -> ItemStack {
    let guard = menu.behavior().lock_all_containers();
    menu.behavior().slots()[0].get_item(&guard).clone()
}

#[test]
fn placing_moves_one_set_and_a_second_press_adds_one_more() {
    let Table {
        player, mut menu, ..
    } = crafting_table("place_recipe_one_more");
    give(&player, 0, &vanilla_items::OAK_PLANKS, 9);

    let action = menu.handle_placement(placement("crafting_table", false), &player);
    assert_eq!(action, Some(PostPlaceAction::Nothing));
    assert_eq!(menu_counts(&menu, 1..10), [1, 1, 0, 1, 1, 0, 0, 0, 0]);
    assert!(result(&menu).is(&vanilla_items::CRAFTING_TABLE));
    assert_eq!(player.inventory.lock().get_item(0).count(), 5);

    menu.handle_placement(placement("crafting_table", false), &player);
    assert_eq!(menu_counts(&menu, 1..10), [2, 2, 0, 2, 2, 0, 0, 0, 0]);
    assert_eq!(player.inventory.lock().get_item(0).count(), 1);

    // One plank left is not a third set: the grid stays as it is.
    menu.handle_placement(placement("crafting_table", false), &player);
    assert_eq!(menu_counts(&menu, 1..10), [2, 2, 0, 2, 2, 0, 0, 0, 0]);
}

#[test]
fn shift_placing_fills_as_many_sets_as_the_items_make() {
    let Table {
        player, mut menu, ..
    } = crafting_table("place_recipe_max");
    give(&player, 0, &vanilla_items::OAK_PLANKS, 64);
    give(&player, 1, &vanilla_items::OAK_PLANKS, 30);

    menu.handle_placement(placement("crafting_table", true), &player);
    // 94 planks make 23 tables.
    assert_eq!(menu_counts(&menu, 1..10), [23, 23, 0, 23, 23, 0, 0, 0, 0]);
    let inventory = player.inventory.lock();
    assert_eq!(
        inventory.get_item(0).count() + inventory.get_item(1).count(),
        2
    );
}

#[test]
fn missing_ingredients_clear_the_grid_and_ask_for_a_ghost() {
    let Table {
        player, mut menu, ..
    } = crafting_table("place_recipe_ghost");
    give(&player, 0, &vanilla_items::OAK_PLANKS, 3);
    menu.handle_placement(placement("stick", false), &player);
    assert_eq!(menu_counts(&menu, 1..10), [0, 1, 0, 0, 1, 0, 0, 0, 0]);

    let action = menu.handle_placement(placement("crafting_table", false), &player);
    assert_eq!(action, Some(PostPlaceAction::PlaceGhostRecipe));
    assert_eq!(menu_counts(&menu, 1..10), [0; 9]);
    assert!(result(&menu).is_empty());
    assert_eq!(player.inventory.lock().get_item(0).count(), 3);
}

#[test]
fn a_grid_that_cannot_go_back_stays_unless_dropping_is_allowed() {
    let Table {
        player, mut menu, ..
    } = crafting_table("place_recipe_full_inventory");
    give(&player, 0, &vanilla_items::OAK_PLANKS, 2);
    menu.handle_placement(placement("stick", false), &player);
    for slot in 0..36 {
        give(&player, slot, &vanilla_items::DIRT, 64);
    }
    give(&player, 0, &vanilla_items::OAK_PLANKS, 64);

    assert_eq!(
        menu.handle_placement(placement("crafting_table", false), &player),
        Some(PostPlaceAction::Nothing)
    );
    assert_eq!(menu_counts(&menu, 1..10), [0, 1, 0, 0, 1, 0, 0, 0, 0]);

    let creative = RecipePlacement {
        allow_dropping_items_to_clear: true,
        ..placement("crafting_table", false)
    };
    assert_eq!(
        menu.handle_placement(creative, &player),
        Some(PostPlaceAction::Nothing)
    );
    assert_eq!(menu_counts(&menu, 1..10), [1, 1, 0, 1, 1, 0, 0, 0, 0]);
}

#[test]
fn the_inventory_grid_takes_two_by_two_recipes_only() {
    init_vanilla_registry();
    init_behaviors();
    let world_fixture = fresh_test_world("place_recipe_inventory_grid");
    let player = TestPlayerBuilder::new(Arc::clone(&world_fixture.world), "Placer", 1).build();
    let mut menu = inventory_menu(Arc::clone(&player.inventory));
    give(&player, 0, &vanilla_items::OAK_PLANKS, 4);

    menu.handle_placement(placement("crafting_table", false), &player);
    assert_eq!(menu_counts(&menu, 1..5), [1, 1, 1, 1]);
    assert!(result(&menu).is(&vanilla_items::CRAFTING_TABLE));

    // A smelting recipe has no place in a crafting grid.
    assert_eq!(
        menu.handle_placement(
            placement("iron_ingot_from_smelting_raw_iron", false),
            &player
        ),
        None
    );
}

#[test]
fn a_furnace_takes_its_input_from_the_inventory() {
    init_vanilla_registry();
    init_behaviors();
    let world_fixture = fresh_test_world("place_recipe_furnace");
    let player = TestPlayerBuilder::new(Arc::clone(&world_fixture.world), "Placer", 1).build();
    let container = SimpleContainer::new(3).into_shared();
    container
        .lock()
        .set_item(2, ItemStack::new(&vanilla_items::IRON_INGOT));
    let mut menu = furnace_menu(
        Arc::clone(&player.inventory),
        1,
        ContainerRef::from(Arc::clone(&container)),
        FurnaceKind::Furnace,
    );
    give(&player, 0, &vanilla_items::RAW_IRON, 10);

    let action = menu.handle_placement(
        placement("iron_ingot_from_smelting_raw_iron", true),
        &player,
    );
    assert_eq!(action, Some(PostPlaceAction::Nothing));
    let furnace = container.lock();
    assert_eq!(furnace.get_item(0).count(), 10);
    assert!(furnace.get_item(0).is(&vanilla_items::RAW_IRON));
    // The result went back to the inventory when the grid was cleared.
    assert!(furnace.get_item(2).is_empty());
    assert!(
        player
            .inventory
            .lock()
            .get_item(1)
            .is(&vanilla_items::IRON_INGOT)
    );

    drop(furnace);
    assert_eq!(
        menu.handle_placement(placement("crafting_table", false), &player),
        None
    );
}
