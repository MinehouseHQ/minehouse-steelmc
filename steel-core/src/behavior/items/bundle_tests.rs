use steel_registry::{init_vanilla_registry, vanilla_items};

use super::*;
use crate::behavior::init_behaviors;
use crate::inventory::click::Click;
use crate::inventory::container::Container as _;
use crate::test_support::{TestPlayerBuilder, fresh_test_world};

/// Inventory menu slot of hotbar slot 0.
const HOTBAR_0: usize = 36;

fn setup() {
    init_vanilla_registry();
    init_behaviors();
}

fn bundle_with(stacks: &[ItemStack]) -> ItemStack {
    let mut contents = BundleMutable::from_contents(&BundleContents::empty());
    for stack in stacks {
        let mut stack = stack.clone();
        contents.try_insert(&mut stack);
    }
    let mut bundle = ItemStack::new(&vanilla_items::BUNDLE);
    bundle.set(BUNDLE_CONTENTS, contents.to_immutable());
    bundle
}

fn contents(bundle: &ItemStack) -> Vec<(String, i32)> {
    bundle
        .get(BUNDLE_CONTENTS)
        .map_or_else(Vec::new, |contents| {
            contents
                .items()
                .iter()
                .map(|item| (item.create().item().key.path.to_string(), item.count()))
                .collect()
        })
}

#[test]
fn bundle_weight_follows_max_stack_size() {
    setup();
    let mut contents = BundleMutable::from_contents(&BundleContents::empty());
    let mut pearls = ItemStack::with_count(&vanilla_items::ENDER_PEARL, 16);
    assert_eq!(contents.try_insert(&mut pearls), 16);
    assert!(pearls.is_empty());
    assert_eq!(contents.weight(), CheckedFraction::ONE);

    let mut stone = ItemStack::new(&vanilla_items::STONE);
    assert_eq!(contents.try_insert(&mut stone), 0);
    assert_eq!(stone.count(), 1);
}

#[test]
fn partial_insert_leaves_the_rest_and_merges_equal_stacks() {
    setup();
    let mut contents = BundleMutable::from_contents(&BundleContents::empty());
    let mut stone = ItemStack::with_count(&vanilla_items::STONE, 40);
    assert_eq!(contents.try_insert(&mut stone), 40);
    let mut more = ItemStack::with_count(&vanilla_items::STONE, 40);
    assert_eq!(contents.try_insert(&mut more), 24);
    assert_eq!(more.count(), 16);
    let bundle = contents.to_immutable();
    assert_eq!(bundle.items().len(), 1);
    assert_eq!(bundle.items()[0].count(), 64);
}

#[test]
fn remove_one_takes_the_selected_stack_or_the_newest() {
    setup();
    let mut contents = BundleMutable::from_contents(
        bundle_with(&[
            ItemStack::with_count(&vanilla_items::STONE, 2),
            ItemStack::with_count(&vanilla_items::DIRT, 3),
        ])
        .get(BUNDLE_CONTENTS)
        .expect("bundle contents"),
    );
    contents.toggle_selected_item(1);
    let removed = contents.remove_one().expect("selected stack");
    assert!(removed.is(&vanilla_items::STONE));
    let removed = contents.remove_one().expect("newest stack");
    assert!(removed.is(&vanilla_items::DIRT));
    assert!(contents.remove_one().is_none());
    assert_eq!(contents.weight(), CheckedFraction::ZERO);
}

#[test]
fn shulker_boxes_and_bundles_with_items_follow_container_rules() {
    setup();
    let mut contents = BundleMutable::from_contents(&BundleContents::empty());
    let mut shulker = ItemStack::new(&vanilla_items::SHULKER_BOX);
    assert_eq!(contents.try_insert(&mut shulker), 0);

    let mut inner = bundle_with(&[ItemStack::with_count(&vanilla_items::STONE, 32)]);
    assert_eq!(contents.try_insert(&mut inner), 1);
    // 1/2 for the stone plus 1/16 for the nested bundle itself.
    let mut stone = ItemStack::with_count(&vanilla_items::STONE, 64);
    assert_eq!(contents.try_insert(&mut stone), 28);
}

#[test]
fn carried_bundle_scoops_up_a_slot_and_puts_one_back() {
    setup();
    let fixture = fresh_test_world("bundle_carried");
    let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "Bundler", 51).build();
    player
        .inventory
        .lock()
        .set_item(0, ItemStack::with_count(&vanilla_items::DIRT, 10));
    let mut menu = player.inventory_menu.lock();
    *menu.behavior_mut().carried_mut() = ItemStack::new(&vanilla_items::BUNDLE);

    menu.clicked(
        Click::Pickup {
            slot: HOTBAR_0,
            button: MouseButton::Left,
        },
        &player,
    );
    assert_eq!(
        contents(menu.behavior().carried()),
        [("dirt".to_owned(), 10)]
    );
    assert!(player.inventory.lock().get_item(0).is_empty());

    menu.clicked(
        Click::Pickup {
            slot: HOTBAR_0,
            button: MouseButton::Right,
        },
        &player,
    );
    assert_eq!(contents(menu.behavior().carried()), Vec::new());
    assert_eq!(player.inventory.lock().get_item(0).count(), 10);
}

#[test]
fn items_clicked_onto_a_bundle_go_inside_and_come_back_out() {
    setup();
    let fixture = fresh_test_world("bundle_in_slot");
    let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "Bundler", 52).build();
    player
        .inventory
        .lock()
        .set_item(0, ItemStack::new(&vanilla_items::BUNDLE));
    let mut menu = player.inventory_menu.lock();
    *menu.behavior_mut().carried_mut() = ItemStack::with_count(&vanilla_items::COBBLESTONE, 5);

    menu.clicked(
        Click::Pickup {
            slot: HOTBAR_0,
            button: MouseButton::Left,
        },
        &player,
    );
    assert!(menu.behavior().carried().is_empty());
    assert_eq!(
        contents(player.inventory.lock().get_item(0)),
        [("cobblestone".to_owned(), 5)]
    );

    menu.clicked(
        Click::Pickup {
            slot: HOTBAR_0,
            button: MouseButton::Right,
        },
        &player,
    );
    assert!(menu.behavior().carried().is(&vanilla_items::COBBLESTONE));
    assert_eq!(menu.behavior().carried().count(), 5);
    assert_eq!(contents(player.inventory.lock().get_item(0)), Vec::new());
}

#[test]
fn selecting_a_stack_picks_what_comes_out_first() {
    setup();
    let fixture = fresh_test_world("bundle_select");
    let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "Bundler", 53).build();
    player.inventory.lock().set_item(
        0,
        bundle_with(&[
            ItemStack::with_count(&vanilla_items::STONE, 2),
            ItemStack::with_count(&vanilla_items::DIRT, 3),
        ]),
    );
    let mut menu = player.inventory_menu.lock();
    menu.set_selected_bundle_item_index(HOTBAR_0, 1);
    menu.clicked(
        Click::Pickup {
            slot: HOTBAR_0,
            button: MouseButton::Right,
        },
        &player,
    );
    assert!(menu.behavior().carried().is(&vanilla_items::STONE));
}
