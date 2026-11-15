use simdnbt::owned::NbtCompound;
use steel_registry::{init_vanilla_registry, vanilla_blocks};
use steel_utils::{BlockPos, ChunkPos, types::UpdateFlags};

use super::*;
use crate::{
    behavior::init_behaviors,
    block_entity::init_block_entities,
    test_support::{TestWorld, fresh_test_world, insert_ready_full_chunk},
};

fn setup_world(key: &'static str) -> TestWorld {
    init_vanilla_registry();
    init_behaviors();
    init_block_entities();
    let fixture = fresh_test_world(key);
    insert_ready_full_chunk(&fixture.world, ChunkPos::new(0, 0));
    fixture
}

const fn options(filter: CloneFilter, mode: CloneMode) -> CloneOptions {
    CloneOptions {
        dimensions: Dimensions {
            from: DimensionChoice::Source,
            to: DimensionChoice::Source,
        },
        filter,
        mode,
        strict: false,
    }
}

fn place(world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
    assert!(world.set_block(pos, state, UpdateFlags::UPDATE_NONE));
}

fn run(
    world: &Arc<World>,
    regions: CloneRegions,
    filter: CloneFilter,
    mode: CloneMode,
) -> Result<i32, CommandSyntaxError> {
    run_with(world, regions, options(filter, mode))
}

fn run_with(
    world: &Arc<World>,
    regions: CloneRegions,
    options: CloneOptions,
) -> Result<i32, CommandSyntaxError> {
    let CloneOptions { filter, mode, .. } = options;
    check_clone(world, world, regions, mode, 32_768)?;
    let selection = Selection {
        filter,
        predicate: None,
    };
    Ok(clone_blocks(world, world, regions, &selection, options))
}

#[test]
fn clone_copies_states_and_block_entity_data() {
    let fixture = setup_world("clone_copy");
    let world = &fixture.world;
    let begin = BlockPos::new(1, 64, 1);
    place(world, begin, vanilla_blocks::STONE.default_state());
    place(world, begin.east(), vanilla_blocks::CHEST.default_state());
    let mut components = NbtCompound::new();
    components.insert("minecraft:repair_cost", 3_i32);
    let mut nbt = NbtCompound::new();
    nbt.insert("LootTable", "minecraft:chests/simple_dungeon");
    nbt.insert("components", components);
    let chest = world
        .get_block_entity(begin.east())
        .expect("a placed chest has a block entity");
    chest
        .load_with_owned_components(&nbt)
        .expect("test NBT must load");

    let destination = BlockPos::new(1, 70, 1);
    let regions = CloneRegions::new(begin, begin.east(), destination);
    assert_eq!(
        run(world, regions, CloneFilter::All, CloneMode::Normal),
        Ok(2)
    );

    assert_eq!(
        world.get_block_state(destination).get_block(),
        &vanilla_blocks::STONE
    );
    let copy = world
        .get_block_entity(destination.east())
        .expect("the cloned chest has a block entity");
    assert!(!Arc::ptr_eq(&copy, &chest));
    assert_eq!(copy.save_without_metadata(), chest.save_without_metadata());
    // A normal clone leaves the source in place.
    assert_eq!(
        world.get_block_state(begin).get_block(),
        &vanilla_blocks::STONE
    );
}

#[test]
fn normal_mode_rejects_overlap_but_force_and_move_allow_it() {
    let fixture = setup_world("clone_overlap");
    let world = &fixture.world;
    let begin = BlockPos::new(2, 64, 2);
    place(world, begin, vanilla_blocks::STONE.default_state());
    place(world, begin.east(), vanilla_blocks::DIRT.default_state());
    let regions = CloneRegions::new(begin, begin.east(), begin.east());

    assert!(run(world, regions, CloneFilter::All, CloneMode::Normal).is_err());
    assert_eq!(
        world.get_block_state(begin.east()).get_block(),
        &vanilla_blocks::DIRT
    );

    // Forced, the stone lands on the dirt and the dirt one block further.
    assert_eq!(
        run(world, regions, CloneFilter::All, CloneMode::Force),
        Ok(2)
    );
    assert_eq!(
        world.get_block_state(begin.east()).get_block(),
        &vanilla_blocks::STONE
    );
    assert_eq!(
        world.get_block_state(begin.east().east()).get_block(),
        &vanilla_blocks::DIRT
    );
}

#[test]
fn move_clears_the_source_and_masked_skips_air() {
    let fixture = setup_world("clone_move_masked");
    let world = &fixture.world;
    let begin = BlockPos::new(3, 64, 3);
    place(world, begin, vanilla_blocks::STONE.default_state());
    let destination = BlockPos::new(3, 70, 3);
    place(
        world,
        destination.east(),
        vanilla_blocks::DIRT.default_state(),
    );

    // The air next to the stone is not cloned over the dirt.
    let regions = CloneRegions::new(begin, begin.east(), destination);
    assert_eq!(
        run(world, regions, CloneFilter::Masked, CloneMode::Move),
        Ok(1)
    );
    assert_eq!(
        world.get_block_state(destination).get_block(),
        &vanilla_blocks::STONE
    );
    assert_eq!(
        world.get_block_state(destination.east()).get_block(),
        &vanilla_blocks::DIRT
    );
    assert!(world.get_block_state(begin).is_air());
}

#[test]
fn oversized_regions_fail_before_any_change() {
    let fixture = setup_world("clone_toobig");
    let world = &fixture.world;
    let begin = BlockPos::new(4, 64, 4);
    place(world, begin, vanilla_blocks::STONE.default_state());
    let destination = BlockPos::new(4, 70, 4);
    let regions = CloneRegions::new(begin, begin.east(), destination);

    assert!(check_clone(world, world, regions, CloneMode::Normal, 1).is_err());
    assert!(world.get_block_state(destination).is_air());
}

#[test]
fn scheduled_block_ticks_follow_the_clone() {
    let fixture = setup_world("clone_ticks");
    let world = &fixture.world;
    let begin = BlockPos::new(5, 64, 5);
    let sand = vanilla_blocks::SAND.default_state();
    place(world, begin, sand);
    world.schedule_block_tick_default(begin, sand.get_block(), 7);
    assert!(world.has_scheduled_block_tick(begin, sand.get_block()));

    // Strict placement skips `onPlace`, so only the copy can schedule sand here.
    let destination = BlockPos::new(5, 70, 5);
    let regions = CloneRegions::new(begin, begin, destination);
    let strict = CloneOptions {
        strict: true,
        ..options(CloneFilter::All, CloneMode::Normal)
    };
    assert_eq!(run_with(world, regions, strict), Ok(1));

    assert!(world.has_scheduled_block_tick(destination, sand.get_block()));
    assert!(world.has_scheduled_block_tick(begin, sand.get_block()));
}
