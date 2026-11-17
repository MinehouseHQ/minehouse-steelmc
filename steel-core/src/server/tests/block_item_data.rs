//! Placing a block from a stack that carries block state and block entity
//! data, vanilla `BlockItem.updateBlockStateFromTag` and
//! `updateCustomBlockEntityTag`.

use std::collections::BTreeMap;
use std::sync::Arc;

use glam::DVec3;
use simdnbt::owned::NbtCompound;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::data_components::components::{BlockEntityData, CustomData};
use steel_registry::data_components::vanilla_components::{
    BLOCK_ENTITY_DATA, BLOCK_STATE, BlockItemStateProperties,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::{
    init_vanilla_registry, vanilla_block_entity_types, vanilla_blocks, vanilla_items,
};
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, ChunkPos, Direction};

use super::insert_ready_full_chunk;
use super::scoreboard_command::with_server_world;
use crate::behavior::items::BlockItem;
use crate::behavior::{
    BlockHitResult, BlockPlaceContext, InteractionResult, PlacementOrientation, PlacementSource,
    init_behaviors,
};
use crate::block_entity::init_block_entities;
use crate::entity::init_entities;
use crate::world::World;

fn init() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
}

/// Places `stack` as `block` on top of the stone at `support`.
fn place_on(
    world: &Arc<World>,
    support: BlockPos,
    block: BlockRef,
    stack: &mut ItemStack,
) -> InteractionResult {
    let source = PlacementSource::direct(
        None,
        InteractionHand::MainHand,
        stack,
        PlacementOrientation::Directional {
            direction: Direction::North,
        },
        false,
    );
    let context = BlockPlaceContext::new(
        world,
        source,
        &BlockHitResult {
            location: DVec3::new(
                f64::from(support.x()) + 0.5,
                f64::from(support.y()) + 1.0,
                f64::from(support.z()) + 0.5,
            ),
            direction: Direction::Up,
            block_pos: support,
            miss: false,
            inside: false,
            world_border_hit: false,
        },
    );
    BlockItem::new(block).place(context)
}

fn block_entity_data(
    block_entity_type: BlockEntityTypeRef,
    key: &str,
    value: &str,
) -> BlockEntityData {
    let mut payload = NbtCompound::new();
    payload.insert(key, value);
    let data = CustomData::try_from_compound(payload).expect("valid custom data");
    BlockEntityData::new(block_entity_type, data)
}

#[test]
fn placed_blocks_take_the_stacks_state_and_data() {
    init();
    with_server_world("block_item_data", |_server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let support = BlockPos::new(4, 63, 4);
        world.set_block(
            support,
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_ALL,
        );

        let mut light = ItemStack::new(&vanilla_items::LIGHT);
        let mut properties = BTreeMap::new();
        properties.insert("level".to_owned(), "3".to_owned());
        properties.insert("no_such_property".to_owned(), "x".to_owned());
        light.set(BLOCK_STATE, BlockItemStateProperties::new(properties));
        assert_eq!(
            place_on(world, support, &vanilla_blocks::LIGHT, &mut light),
            InteractionResult::Success
        );
        assert_eq!(
            world
                .get_block_state(support.above())
                .get_value(&BlockStateProperties::LEVEL),
            3,
            "the stack's block state sets the light level"
        );

        let chest_support = support.offset(2, 0, 0);
        world.set_block(
            chest_support,
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        let mut chest = ItemStack::new(&vanilla_items::CHEST);
        chest.set(
            BLOCK_ENTITY_DATA,
            block_entity_data(
                &vanilla_block_entity_types::CHEST,
                "LootTable",
                "minecraft:chests/simple_dungeon",
            ),
        );
        assert_eq!(
            place_on(world, chest_support, &vanilla_blocks::CHEST, &mut chest),
            InteractionResult::Success
        );
        let saved = world
            .get_block_entity(chest_support.above())
            .expect("a chest")
            .save_custom_only();
        assert_eq!(
            saved
                .string("LootTable")
                .map(ToString::to_string)
                .as_deref(),
            Some("minecraft:chests/simple_dungeon")
        );

        let sign_support = support.offset(0, 0, 2);
        world.set_block(
            sign_support,
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        let mut barrel = ItemStack::new(&vanilla_items::BARREL);
        barrel.set(
            BLOCK_ENTITY_DATA,
            block_entity_data(
                &vanilla_block_entity_types::CHEST,
                "LootTable",
                "minecraft:chests/simple_dungeon",
            ),
        );
        assert_eq!(
            place_on(world, sign_support, &vanilla_blocks::BARREL, &mut barrel),
            InteractionResult::Success
        );
        let saved = world
            .get_block_entity(sign_support.above())
            .expect("a barrel")
            .save_custom_only();
        assert!(
            saved.get("LootTable").is_none(),
            "data for another block entity type is ignored"
        );
    });
}
