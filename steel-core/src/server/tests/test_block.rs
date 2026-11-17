//! Test blocks: start blocks powering their neighbors and the other modes
//! noting a redstone pulse, vanilla `TestBlockEntity`.

use std::io::Cursor;
use std::sync::Arc;

use simdnbt::borrow::read_compound;
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction, TestBlockMode};
use steel_registry::data_components::vanilla_components::BLOCK_STATE;
use steel_registry::{init_vanilla_registry, vanilla_blocks};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos, Downcast as _};

use super::insert_ready_full_chunk;
use super::scoreboard_command::with_server_world;
use crate::behavior::{BLOCK_BEHAVIORS, init_behaviors};
use crate::block_entity::entities::TestBlockEntity;
use crate::block_entity::{SharedBlockEntity, init_block_entities};
use crate::entity::init_entities;
use crate::world::{SignalQueryContext, World};

fn init() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
}

fn with_world(name: &'static str, test: impl FnOnce(&Arc<World>)) {
    init();
    with_server_world(name, |_server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        test(world);
    });
}

fn test_block(block_entity: &SharedBlockEntity) -> &TestBlockEntity {
    block_entity
        .downcast_ref::<TestBlockEntity>()
        .expect("test block entity")
}

fn place(world: &Arc<World>, pos: BlockPos, mode: TestBlockMode) -> SharedBlockEntity {
    let state = vanilla_blocks::TEST_BLOCK
        .default_state()
        .set_value(&BlockStateProperties::TEST_BLOCK_MODE, mode);
    assert!(world.set_block(pos, state, UpdateFlags::UPDATE_ALL));
    world.get_block_entity(pos).expect("a test block entity")
}

fn signal(world: &Arc<World>, pos: BlockPos) -> i32 {
    let state = world.get_block_state(pos);
    BLOCK_BEHAVIORS.get_behavior(state.get_block()).get_signal(
        state,
        world.as_ref(),
        pos,
        Direction::Up,
        SignalQueryContext::DEFAULT,
    )
}

#[test]
fn a_redstone_pulse_triggers_an_accept_block() {
    with_world("test_block_accept", |world| {
        let pos = BlockPos::new(4, 64, 4);
        let block_entity = place(world, pos, TestBlockMode::Accept);
        assert!(!test_block(&block_entity).has_triggered());
        world.set_block(
            pos.below(),
            vanilla_blocks::REDSTONE_BLOCK.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        assert!(test_block(&block_entity).is_powered());
        assert!(test_block(&block_entity).has_triggered());
    });
}

#[test]
fn a_triggered_start_block_powers_until_reset() {
    with_world("test_block_start", |world| {
        let pos = BlockPos::new(4, 64, 4);
        let block_entity = place(world, pos, TestBlockMode::Start);
        world.set_block(
            pos.below(),
            vanilla_blocks::REDSTONE_BLOCK.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        assert!(
            !test_block(&block_entity).is_powered(),
            "redstone does not start a start block"
        );
        assert_eq!(signal(world, pos), 0);

        test_block(&block_entity).trigger(world);
        assert_eq!(signal(world, pos), 15);
        test_block(&block_entity).reset(world);
        assert_eq!(signal(world, pos), 0);
    });
}

#[test]
fn test_block_data_saves_under_vanilla_keys() {
    with_world("test_block_nbt", |world| {
        let pos = BlockPos::new(4, 64, 4);
        let block_entity = place(world, pos, TestBlockMode::Log);
        test_block(&block_entity).set_message("hello".to_owned());
        let mut nbt = NbtCompound::new();
        block_entity.save_additional(&mut nbt);
        assert_eq!(
            nbt.string("mode").map(ToString::to_string).as_deref(),
            Some("log")
        );
        assert_eq!(
            nbt.string("message").map(ToString::to_string).as_deref(),
            Some("hello")
        );
        assert_eq!(nbt.byte("powered"), Some(0));

        let mut bytes = Vec::new();
        NbtCompound::new().write(&mut bytes);
        let empty = read_compound(&mut Cursor::new(bytes.as_slice())).expect("nbt");
        block_entity.load_additional(&empty);
        assert_eq!(
            test_block(&block_entity).mode(),
            TestBlockMode::Fail,
            "a missing mode loads as fail"
        );
    });
}

#[test]
fn a_picked_test_block_keeps_its_mode() {
    init();
    let state = vanilla_blocks::TEST_BLOCK.default_state().set_value(
        &BlockStateProperties::TEST_BLOCK_MODE,
        TestBlockMode::Accept,
    );
    let stack = BLOCK_BEHAVIORS
        .get_behavior(&vanilla_blocks::TEST_BLOCK)
        .get_clone_item_stack(&vanilla_blocks::TEST_BLOCK, state, None, false)
        .expect("a test block stack");
    let mode = stack
        .get(BLOCK_STATE)
        .and_then(|properties| properties.properties().get("mode").cloned());
    assert_eq!(mode.as_deref(), Some("accept"));
}
