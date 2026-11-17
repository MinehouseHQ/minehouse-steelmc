//! Vanilla placement, survival and chaining checks for signs.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::{init_vanilla_registry, vanilla_items};
use steel_utils::ChunkPos;
use steel_utils::types::UpdateFlags;

use super::*;
use crate::behavior::{PlacementOrientation, init_behaviors};
use crate::test_support::{
    TestLevel, TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk,
};

fn world(name: &'static str) -> Arc<World> {
    init_vanilla_registry();
    init_behaviors();
    let fixture = fresh_test_world(name);
    insert_ready_full_chunk(&fixture.world, ChunkPos::new(0, 0));
    Arc::clone(&fixture.world)
}

fn hit(pos: BlockPos, face: Direction) -> BlockHitResult {
    let center = DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + 0.5,
        f64::from(pos.z()) + 0.5,
    );
    BlockHitResult {
        location: center + face.offset_vec().as_dvec3() * 0.5,
        direction: face,
        block_pos: pos,
        miss: false,
        inside: false,
        world_border_hit: false,
    }
}

fn place_state<B: BlockBehavior>(
    world: &Arc<World>,
    behavior: &B,
    item: &ItemStack,
    hit_result: &BlockHitResult,
    yaw: f32,
) -> Option<BlockStateId> {
    let mut stack = item.clone();
    let source = PlacementSource::direct(
        None,
        InteractionHand::MainHand,
        &mut stack,
        PlacementOrientation::Player {
            rotation: yaw,
            pitch: 0.0,
        },
        false,
    );
    let context = BlockPlaceContext::new(world, source, hit_result);
    behavior.get_state_for_placement(&context)
}

#[test]
fn signs_placed_into_a_water_source_are_waterlogged() {
    let world = world("sign_waterlogged_placement");
    let wall = BlockPos::new(4, 64, 4);
    let pos = wall.south();
    world.set_block(
        wall,
        vanilla_blocks::STONE.default_state(),
        UpdateFlags::UPDATE_ALL,
    );
    world.set_block(
        pos.below(),
        vanilla_blocks::STONE.default_state(),
        UpdateFlags::UPDATE_ALL,
    );
    world.set_block(
        pos,
        vanilla_blocks::WATER.default_state(),
        UpdateFlags::UPDATE_ALL,
    );
    let item = ItemStack::new(&vanilla_items::OAK_SIGN);

    let standing = StandingSignBlock::new(&vanilla_blocks::OAK_SIGN);
    let state = place_state(
        &world,
        &standing,
        &item,
        &hit(pos.below(), Direction::Up),
        0.0,
    )
    .expect("standing sign fits on stone");
    assert!(state.get_value(WATERLOGGED));

    // Looking north at the wall: the sign faces south, away from it.
    let wall_sign = WallSignBlock::new(&vanilla_blocks::OAK_WALL_SIGN);
    let state = place_state(
        &world,
        &wall_sign,
        &item,
        &hit(wall, Direction::South),
        180.0,
    )
    .expect("wall sign fits on stone");
    assert!(state.get_value(WATERLOGGED));
    assert_eq!(state.get_value(HORIZONTAL_FACING), Direction::South);

    world.set_block(
        pos,
        vanilla_blocks::AIR.default_state(),
        UpdateFlags::UPDATE_ALL,
    );
    let state = place_state(
        &world,
        &wall_sign,
        &item,
        &hit(wall, Direction::South),
        180.0,
    )
    .expect("wall sign fits on stone");
    assert!(!state.get_value(WATERLOGGED), "air does not waterlog");
}

#[test]
fn hanging_signs_placed_into_a_water_source_are_waterlogged() {
    let world = world("hanging_sign_waterlogged_placement");
    let ceiling = BlockPos::new(4, 66, 4);
    let pos = ceiling.below();
    world.set_block(
        ceiling,
        vanilla_blocks::STONE.default_state(),
        UpdateFlags::UPDATE_ALL,
    );
    world.set_block(
        pos,
        vanilla_blocks::WATER.default_state(),
        UpdateFlags::UPDATE_ALL,
    );
    let item = ItemStack::new(&vanilla_items::OAK_HANGING_SIGN);

    let hanging = CeilingHangingSignBlock::new(&vanilla_blocks::OAK_HANGING_SIGN);
    let state = place_state(&world, &hanging, &item, &hit(ceiling, Direction::Down), 0.0)
        .expect("hanging sign fits under stone");
    assert!(state.get_value(WATERLOGGED));
    assert!(
        !state.get_value(ATTACHED),
        "a full face holds the sign by its chains"
    );

    // A wall hanging sign between two walls on the east-west axis.
    let side = BlockPos::new(8, 64, 4);
    let wall_pos = side.east();
    world.set_block(
        side,
        vanilla_blocks::STONE.default_state(),
        UpdateFlags::UPDATE_ALL,
    );
    world.set_block(
        wall_pos.east(),
        vanilla_blocks::STONE.default_state(),
        UpdateFlags::UPDATE_ALL,
    );
    world.set_block(
        wall_pos,
        vanilla_blocks::WATER.default_state(),
        UpdateFlags::UPDATE_ALL,
    );
    let wall_hanging = WallHangingSignBlock::new(&vanilla_blocks::OAK_WALL_HANGING_SIGN);
    let state = place_state(
        &world,
        &wall_hanging,
        &item,
        &hit(side, Direction::East),
        0.0,
    )
    .expect("wall hanging sign attaches to the stone sides");
    assert!(state.get_value(WATERLOGGED));
    assert_eq!(
        state.get_value(HORIZONTAL_FACING).axis(),
        Direction::North.axis()
    );
}

#[test]
fn wall_hanging_signs_never_break_from_neighbor_changes() {
    init_vanilla_registry();
    let behavior = WallHangingSignBlock::new(&vanilla_blocks::OAK_WALL_HANGING_SIGN);
    let state = vanilla_blocks::OAK_WALL_HANGING_SIGN
        .default_state()
        .set_value(HORIZONTAL_FACING, Direction::North);
    let pos = BlockPos::new(0, 64, 0);
    let unsupported = TestLevel::default();
    assert_eq!(
        behavior.update_shape(
            state,
            &unsupported,
            pos,
            Direction::East,
            pos.east(),
            vanilla_blocks::AIR.default_state(),
        ),
        state
    );
}

#[test]
fn hanging_sign_items_pass_through_hanging_signs_to_chain() {
    let world = world("hanging_sign_chaining");
    let pos = BlockPos::new(4, 64, 4);
    world.set_block(
        pos.above(),
        vanilla_blocks::STONE.default_state(),
        UpdateFlags::UPDATE_ALL,
    );
    let state = vanilla_blocks::OAK_HANGING_SIGN.default_state();
    assert!(world.set_block(pos, state, UpdateFlags::UPDATE_ALL));
    let sign: Arc<dyn BlockEntity> = Arc::new(SignBlockEntity::new_hanging(
        Arc::downgrade(&world),
        pos,
        state,
    ));
    assert!(world.set_block_entity(sign));

    let player = TestPlayerBuilder::new(Arc::clone(&world), "Chainer", 2).build();
    let behavior = CeilingHangingSignBlock::new(&vanilla_blocks::OAK_HANGING_SIGN);
    let use_with = |item: ItemRef, face: Direction| {
        player
            .inventory
            .lock()
            .set_item_in_hand(InteractionHand::MainHand, ItemStack::new(item));
        let mut inv =
            InventoryAccess::new(Arc::clone(&player.inventory), InteractionHand::MainHand);
        behavior.use_item_on(
            state,
            &world,
            pos,
            &player,
            InteractionHand::MainHand,
            &hit(pos, face),
            &mut inv,
        )
    };

    assert_eq!(
        use_with(&vanilla_items::OAK_HANGING_SIGN, Direction::Down),
        InteractionResult::Pass
    );
    assert_eq!(
        use_with(&vanilla_items::OAK_HANGING_SIGN, Direction::North),
        InteractionResult::TryEmptyHandInteraction
    );
    assert_eq!(
        use_with(&vanilla_items::STICK, Direction::Down),
        InteractionResult::TryEmptyHandInteraction
    );
}
