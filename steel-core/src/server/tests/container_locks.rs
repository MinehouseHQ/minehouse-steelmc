//! Container custom names and locks, vanilla `BaseContainerBlockEntity` and
//! `BeaconBlockEntity`.

use std::sync::Arc;

use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, ChestType, Direction};
use steel_registry::data_components::vanilla_components::{CUSTOM_NAME, LOCK};
use steel_registry::item_stack::ItemStack;
use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_items};
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, ChunkPos};
use text_components::TextComponent;

use super::scoreboard_command::with_server_world;
use super::{insert_ready_full_chunk, test_player_with_packets};
use crate::behavior::{BLOCK_BEHAVIORS, init_behaviors};
use crate::block_entity::init_block_entities;
use crate::entity::{init_entities, next_entity_id};
use crate::player::Player;
use crate::world::World;

fn init() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
}

/// Saved block entity data locked to diamonds, optionally named.
fn locked_data(name: Option<&str>) -> NbtCompound {
    let mut lock = NbtCompound::new();
    lock.insert("items", "minecraft:diamond");
    let mut nbt = NbtCompound::new();
    nbt.insert("lock", lock);
    if let Some(name) = name {
        nbt.insert(
            "CustomName",
            TextComponent::from(name.to_owned()).to_codec_nbt(),
        );
    }
    nbt
}

fn load(world: &World, pos: BlockPos, nbt: &NbtCompound) {
    let Some(block_entity) = world.get_block_entity(pos) else {
        panic!("a block entity at {pos:?}");
    };
    if let Err(error) = block_entity.load_with_owned_components(nbt) {
        panic!("block entity data loads: {error:?}");
    }
}

fn open(world: &Arc<World>, pos: BlockPos, player: &Player) -> bool {
    let state = world.get_block_state(pos);
    BLOCK_BEHAVIORS
        .get_behavior(state.get_block())
        .open_menu_provider(state, world, pos, player)
}

fn hold(player: &Player, item: ItemStack) {
    player
        .inventory
        .lock()
        .set_item_in_hand(InteractionHand::MainHand, item);
}

#[test]
fn a_locked_barrel_opens_only_for_the_key() {
    init();
    with_server_world("container_locks_barrel", |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let pos = BlockPos::new(4, 64, 4);
        world.set_block(
            pos,
            vanilla_blocks::BARREL.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        load(world, pos, &locked_data(Some("Vault")));
        let player =
            test_player_with_packets(server, Arc::clone(world), "Player", next_entity_id()).0;

        assert!(!open(world, pos, &player), "an empty hand cannot unlock");
        assert!(!player.has_container_open());

        hold(&player, ItemStack::new(&vanilla_items::DIAMOND));
        assert!(open(world, pos, &player), "the key unlocks the barrel");
        assert!(player.has_container_open());

        let Some(barrel) = world.get_block_entity(pos) else {
            panic!("the barrel is still there");
        };
        let saved = barrel.save_custom_only();
        assert!(saved.get("lock").is_some(), "the lock is saved");
        assert!(saved.get("CustomName").is_some(), "the name is saved");
        let components = barrel.collect_components();
        assert_eq!(
            components.get(CUSTOM_NAME),
            Some(TextComponent::from("Vault".to_owned()))
        );
        assert!(
            components.get(LOCK).is_some(),
            "a broken barrel keeps its lock"
        );
    });
}

#[test]
fn a_double_chest_with_one_locked_half_stays_shut() {
    init();
    with_server_world("container_locks_double_chest", |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let left = BlockPos::new(4, 64, 4);
        let right = BlockPos::new(5, 64, 4);
        for (pos, chest_type) in [(left, ChestType::Left), (right, ChestType::Right)] {
            let state = vanilla_blocks::CHEST
                .default_state()
                .set_value(&BlockStateProperties::HORIZONTAL_FACING, Direction::North)
                .set_value(&BlockStateProperties::CHEST_TYPE, chest_type);
            world.set_block(pos, state, UpdateFlags::UPDATE_ALL);
        }
        load(world, right, &locked_data(None));
        let player =
            test_player_with_packets(server, Arc::clone(world), "Player", next_entity_id()).0;

        assert!(
            !open(world, left, &player),
            "the locked half keeps both shut"
        );
        assert!(!player.has_container_open());

        hold(&player, ItemStack::new(&vanilla_items::DIAMOND));
        assert!(open(world, left, &player));
        assert!(player.has_container_open());
    });
}

#[test]
fn a_placed_named_shulker_box_keeps_its_name() {
    init();
    with_server_world("container_locks_named_shulker", |_server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let pos = BlockPos::new(4, 64, 4);
        world.set_block(
            pos,
            vanilla_blocks::SHULKER_BOX.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        let Some(shulker_box) = world.get_block_entity(pos) else {
            panic!("a shulker box");
        };
        let mut stack = ItemStack::new(&vanilla_items::SHULKER_BOX);
        let name = TextComponent::from("Ores".to_owned());
        stack.set(CUSTOM_NAME, name.clone());
        shulker_box.apply_components_from_item_stack(&stack);

        let Some(container_name) = shulker_box.container_name() else {
            panic!("a shulker box has a container name");
        };
        assert_eq!(container_name.display_name(), name);
        let saved = shulker_box.save_without_metadata();
        assert!(
            saved
                .compound("components")
                .is_none_or(|components| components.get("minecraft:custom_name").is_none()),
            "the name moves out of the stored components"
        );
        assert!(shulker_box.save_custom_only().get("CustomName").is_some());
    });
}

#[test]
fn a_beacon_keeps_its_name_and_lock() {
    init();
    with_server_world("container_locks_beacon", |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let pos = BlockPos::new(4, 64, 4);
        world.set_block(
            pos,
            vanilla_blocks::BEACON.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        load(world, pos, &locked_data(Some("Lighthouse")));
        let player =
            test_player_with_packets(server, Arc::clone(world), "Player", next_entity_id()).0;
        assert!(!open(world, pos, &player));

        let Some(beacon) = world.get_block_entity(pos) else {
            panic!("a beacon");
        };
        let Some(update) = beacon.get_update_tag() else {
            panic!("a beacon sends its data");
        };
        assert!(update.get("CustomName").is_some(), "clients see the name");
        assert!(beacon.save_custom_only().get("lock").is_some());

        hold(&player, ItemStack::new(&vanilla_items::DIAMOND));
        assert!(open(world, pos, &player));
    });
}
