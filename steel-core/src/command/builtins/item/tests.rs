use steel_registry::data_components::components::Rarity;
use steel_registry::data_components::vanilla_components::{
    CUSTOM_NAME, ENCHANTMENTS, ItemEnchantments, RARITY,
};
use steel_registry::item_stack_template::ItemStackTemplate;
use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_items};
use steel_utils::{ChunkPos, types::UpdateFlags};
use text_components::Modifier as _;
use text_components::format::Color;

use super::*;
use crate::behavior::init_behaviors;
use crate::block_entity::init_block_entities;
use crate::inventory::slot_range::slot_range;
use crate::test_support::{fresh_test_world, insert_ready_full_chunk};

fn stone(count: i32) -> ItemStack {
    ItemStack::with_count(&vanilla_items::STONE, count)
}

fn drain(provider: &mut ItemProvider, limit: usize) -> Vec<i32> {
    let mut counts = Vec::new();
    while provider.has_next() && counts.len() < limit {
        counts.push(provider.next().count);
    }
    counts
}

#[test]
fn distributors_build_vanilla_item_providers() {
    init_vanilla_registry();
    let items = || vec![stone(1), stone(2)];

    // `replace`: each item once.
    let mut replace = Distributor::Replace.provider(items());
    assert_eq!(drain(&mut replace, 5), [1, 2]);
    replace.restart();
    assert_eq!(drain(&mut replace, 5), [1, 2]);

    // `fill`: the items over and over.
    let mut fill = Distributor::Fill.provider(items());
    assert_eq!(drain(&mut fill, 5), [1, 2, 1, 2, 1]);
    assert!(!Distributor::Fill.provider(Vec::new()).has_next());

    // `override`: each item once, then empty stacks.
    let mut overriding = Distributor::Override.provider(items());
    assert_eq!(drain(&mut overriding, 4), [1, 2, 0, 0]);
    overriding.restart();
    assert_eq!(drain(&mut overriding, 3), [1, 2, 0]);
}

#[test]
fn replacing_slots_counts_offered_slots_and_stops_when_items_run_out() {
    init_vanilla_registry();
    init_behaviors();
    init_block_entities();
    let fixture = fresh_test_world("item_replace_slots");
    let world = &fixture.world;
    insert_ready_full_chunk(world, ChunkPos::new(0, 0));
    let pos = BlockPos::new(1, 64, 1);
    assert!(world.set_block(
        pos,
        vanilla_blocks::CHEST.default_state(),
        UpdateFlags::UPDATE_NONE
    ));
    let container = world
        .get_block_entity(pos)
        .and_then(ContainerRef::from_block_entity)
        .expect("a chest is a container");
    let all = slot_range("container.*").expect("vanilla slot range");
    let slots = container_slots(&container, all);
    assert_eq!(slots.len(), 27, "a chest has 27 of the 54 container slots");

    // `replace` with one item fills one slot and offers only that one.
    let mut selected = 0;
    let mut items = Distributor::Replace.provider(vec![stone(3)]);
    assert_eq!(replace_slot_items(&slots, &mut items, &mut selected), 1);
    assert_eq!(selected, 1);

    // `fill` reaches every slot.
    let mut selected = 0;
    let mut items = Distributor::Fill.provider(vec![stone(1)]);
    assert_eq!(replace_slot_items(&slots, &mut items, &mut selected), 27);
    assert_eq!(selected, 27);
    let copies: Vec<_> = container_slots(&container, all)
        .iter()
        .map(SlotAccess::get)
        .collect();
    assert!(copies.iter().all(|stack| stack.is(&vanilla_items::STONE)));

    // `override` clears what the source does not cover.
    let mut selected = 0;
    let mut items = Distributor::Override.provider(vec![stone(5)]);
    assert_eq!(replace_slot_items(&slots, &mut items, &mut selected), 27);
    assert_eq!(slots[0].get().count, 5);
    assert!(slots[1].get().is_empty());
    assert!(slots[26].get().is_empty());
}

#[test]
fn with_counts_follow_item_input_limits() {
    init_vanilla_registry();
    assert_eq!(
        create_item_stack(&stone(1), 64).map(|stack| stack.count),
        Ok(64)
    );
    let Err(error) = create_item_stack(&stone(1), 65) else {
        panic!("65 stone is over the stack size");
    };
    assert_eq!(
        error.message_component(),
        translations::ARGUMENTS_ITEM_OVERSTACKED
            .message(["minecraft:stone", "64"])
            .component()
    );
    assert!(create_item_stack(&ItemStack::new(&vanilla_items::DIAMOND_SWORD), 2).is_err());
}

#[test]
fn response_tracker_reports_the_only_changed_target() {
    let mut tracker = ResponseTracker::default();
    tracker.track("a", 0);
    tracker.track("b", 3);
    assert_eq!(tracker.only_non_zero, Some("b"));
    assert_eq!(tracker.non_zero_count, 1);
    tracker.track("c", 2);
    assert_eq!(tracker.only_non_zero, None);
    assert_eq!(tracker.non_zero_count, 2);
    assert_eq!(tracker.total, 5);
}

/// Vanilla `ItemStack.getDisplayName` built by hand: `[name]` in `color`
/// with the item as hover.
fn bracketed(stack: &ItemStack, name: TextComponent, color: Color) -> TextComponent {
    let hover = ItemStackTemplate::from_stack(stack)
        .and_then(|template| template.to_hover_event())
        .expect("a non-empty stack has a hover event");
    translations::CHAT_SQUARE_BRACKETS
        .message([TextComponent::new().add_child(name)])
        .component()
        .color(color)
        .hover_event(hover)
}

#[test]
fn item_display_names_match_vanilla() {
    init_vanilla_registry();
    init_behaviors();
    let stone_name = || TextComponent::from(&translations::BLOCK_MINECRAFT_STONE);

    let plain = stone(1);
    assert_eq!(
        ITEM_BEHAVIORS.display_name(&plain),
        bracketed(&plain, stone_name(), Color::White)
    );

    let mut uncommon = stone(1);
    uncommon.set(RARITY, Rarity::Uncommon);
    assert_eq!(
        ITEM_BEHAVIORS.display_name(&uncommon),
        bracketed(&uncommon, stone_name(), Color::Yellow)
    );

    // A custom name is italic inside the brackets.
    let mut named = stone(1);
    named.set(CUSTOM_NAME, TextComponent::plain("Rock"));
    let hover = ItemStackTemplate::from_stack(&named)
        .and_then(|template| template.to_hover_event())
        .expect("a named stone has a hover event");
    assert_eq!(
        ITEM_BEHAVIORS.display_name(&named),
        translations::CHAT_SQUARE_BRACKETS
            .message([TextComponent::new()
                .add_child(TextComponent::plain("Rock"))
                .italic(true)])
            .component()
            .color(Color::White)
            .hover_event(hover)
    );
}

#[test]
fn enchanting_raises_rarity_like_vanilla() {
    init_vanilla_registry();
    let mut sword = ItemStack::new(&vanilla_items::DIAMOND_SWORD);
    assert_eq!(sword.rarity(), Rarity::Common);
    let mut enchantments = ItemEnchantments::empty();
    enchantments.set(Identifier::vanilla_static("sharpness"), 1);
    sword.set(ENCHANTMENTS, enchantments);
    assert_eq!(sword.rarity(), Rarity::Rare);
    let mut rare = sword.clone();
    rare.set(RARITY, Rarity::Rare);
    assert_eq!(rare.rarity(), Rarity::Epic);
}
