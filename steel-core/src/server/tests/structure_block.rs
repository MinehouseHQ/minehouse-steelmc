//! Structure blocks saving, loading and measuring templates, vanilla
//! `StructureBlockEntity`.

use std::io::Cursor;
use std::sync::Arc;
use std::{env, fs, process, thread};

use glam::IVec3;
use simdnbt::borrow::read_compound;
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, StructureMode};
use steel_registry::{init_vanilla_registry, vanilla_blocks};
use steel_utils::threading::DEBUG_STACK_SIZE;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos, Downcast as _, Identifier, Rotation};
use steel_worldgen::structure::StructureMirror;

use super::insert_ready_full_chunk;
use super::scoreboard_command::with_server_world;
use crate::behavior::init_behaviors;
use crate::block_entity::entities::StructureBlockEntity;
use crate::block_entity::{SharedBlockEntity, init_block_entities};
use crate::entity::init_entities;
use crate::world::World;
use crate::worldgen::template::StructureTemplateManager;

fn init() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
}

fn structure(block_entity: &SharedBlockEntity) -> &StructureBlockEntity {
    block_entity
        .downcast_ref::<StructureBlockEntity>()
        .expect("structure block entity")
}

fn house() -> Identifier {
    Identifier::vanilla_static("test_house")
}

/// Places a structure block at `pos` in `mode`, naming `name`, with its box
/// two blocks east of it and three by two by three blocks big.
fn place(world: &Arc<World>, pos: BlockPos, mode: StructureMode) -> SharedBlockEntity {
    assert!(world.set_block(
        pos,
        vanilla_blocks::STRUCTURE_BLOCK.default_state(),
        UpdateFlags::UPDATE_ALL
    ));
    let block_entity = world
        .get_block_entity(pos)
        .expect("a structure block has a block entity");
    structure(&block_entity).set_mode(world, mode);
    structure(&block_entity).update_settings(|settings| {
        settings.name = Some(house());
        settings.pos = IVec3::new(2, 0, 0);
        settings.size = IVec3::new(3, 2, 3);
    });
    block_entity
}

fn with_world(name: &'static str, test: impl FnOnce(&Arc<World>) + Send + 'static) {
    init();
    // Template placement overflows the default test stack in debug builds.
    thread::Builder::new()
        .stack_size(DEBUG_STACK_SIZE)
        .spawn(move || {
            with_server_world(name, |_server, world| {
                insert_ready_full_chunk(world, ChunkPos::new(0, 0));
                test(world);
            });
        })
        .expect("test thread should spawn")
        .join()
        .expect("test thread panicked");
}

fn build_house(world: &Arc<World>, corner: BlockPos) {
    let set = |offset: (i32, i32, i32), state| {
        world.set_block(
            corner.offset(offset.0, offset.1, offset.2),
            state,
            UpdateFlags::UPDATE_ALL,
        );
    };
    set((0, 0, 0), vanilla_blocks::STONE.default_state());
    set((1, 0, 1), vanilla_blocks::OAK_STAIRS.default_state());
    set((2, 1, 2), vanilla_blocks::CHEST.default_state());
}

#[test]
fn a_saved_structure_loads_into_another_box() {
    with_world("structure_block_round_trip", |world| {
        let generated = env::temp_dir().join(format!("steel-structure-block-{}", process::id()));
        let _ = fs::remove_dir_all(&generated);
        world.set_structure_templates(Arc::new(StructureTemplateManager::new(Some(
            generated.clone(),
        ))));

        let saver = place(world, BlockPos::new(1, 64, 1), StructureMode::Save);
        build_house(world, BlockPos::new(3, 64, 1));
        assert!(structure(&saver).save_structure_to_disk(world));
        let file = generated.join("minecraft/structure/test_house.nbt");
        assert!(
            file.is_file(),
            "the template is written to {}",
            file.display()
        );

        let stored = StructureTemplateManager::new(Some(generated.clone()))
            .get(&house())
            .expect("the saved template reads back");
        assert_eq!(stored.size(), [3, 2, 3]);
        let blocks = stored.nbt.list("blocks").and_then(|list| list.compounds());
        let blocks = blocks.expect("blocks");
        assert_eq!(blocks.len(), 18, "air is part of the template");
        let last = blocks.last().expect("a last block");
        assert_eq!(
            last.compound("nbt")
                .and_then(|nbt| nbt.string("id"))
                .map(ToString::to_string)
                .as_deref(),
            Some("minecraft:chest"),
            "block entities come last, with their id"
        );

        let loader = place(world, BlockPos::new(1, 64, 8), StructureMode::Load);
        assert!(structure(&loader).is_structure_loadable(world));
        assert!(structure(&loader).place_structure_if_same_size(world));
        let corner = BlockPos::new(3, 64, 8);
        assert_eq!(
            world.get_block_state(corner).get_block(),
            &vanilla_blocks::STONE
        );
        assert_eq!(
            world.get_block_state(corner.offset(1, 0, 1)).get_block(),
            &vanilla_blocks::OAK_STAIRS
        );
        assert!(world.get_block_entity(corner.offset(2, 1, 2)).is_some());

        structure(&loader).update_settings(|settings| settings.size = IVec3::ONE);
        assert!(
            !structure(&loader).place_structure_if_same_size(world),
            "a box of another size only takes the template's size"
        );
        assert_eq!(structure(&loader).settings().size, IVec3::new(3, 2, 3));
        let _ = fs::remove_dir_all(&generated);
    });
}

#[test]
fn a_rising_signal_loads_the_template() {
    with_world("structure_block_redstone", |world| {
        let saver = place(world, BlockPos::new(1, 64, 1), StructureMode::Save);
        build_house(world, BlockPos::new(3, 64, 1));
        assert!(structure(&saver).save_structure(world, false));

        let loader_pos = BlockPos::new(1, 64, 8);
        let loader = place(world, loader_pos, StructureMode::Load);
        world.set_block(
            loader_pos.below(),
            vanilla_blocks::REDSTONE_BLOCK.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        assert!(structure(&loader).is_powered());
        assert_eq!(
            world.get_block_state(BlockPos::new(3, 64, 8)).get_block(),
            &vanilla_blocks::STONE
        );
    });
}

#[test]
fn detect_size_takes_the_box_between_corners() {
    with_world("structure_block_detect", |world| {
        let saver = place(world, BlockPos::new(2, 64, 2), StructureMode::Save);
        place(world, BlockPos::new(8, 70, 8), StructureMode::Corner);
        assert!(structure(&saver).detect_size(world));
        let settings = structure(&saver).settings();
        assert_eq!(settings.pos, IVec3::ONE);
        assert_eq!(settings.size, IVec3::splat(5));
    });
}

#[test]
fn structure_settings_load_from_vanilla_keys() {
    with_world("structure_block_nbt", |world| {
        let pos = BlockPos::new(2, 64, 2);
        let block_entity = place(world, pos, StructureMode::Save);
        let mut nbt = NbtCompound::new();
        nbt.insert("name", "minecraft:tower");
        nbt.insert("posX", 100);
        nbt.insert("sizeY", -4);
        nbt.insert("mode", "CORNER");
        nbt.insert("rotation", "CLOCKWISE_90");
        nbt.insert("mirror", "LEFT_RIGHT");
        nbt.insert("ignoreEntities", 0_i8);
        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let borrowed = read_compound(&mut Cursor::new(bytes.as_slice())).expect("nbt");
        block_entity.load_additional(&borrowed);

        let settings = structure(&block_entity).settings();
        assert_eq!(settings.name, Some(Identifier::vanilla_static("tower")));
        assert_eq!(settings.pos, IVec3::new(48, 1, 0), "offsets clamp to 48");
        assert_eq!(settings.size.y, 0);
        assert_eq!(settings.mode, StructureMode::Corner);
        assert_eq!(settings.rotation, Rotation::Clockwise90);
        assert_eq!(settings.mirror, StructureMirror::LeftRight);
        assert!(!settings.ignore_entities);
        assert_eq!(
            world
                .get_block_state(pos)
                .try_get_value(&BlockStateProperties::STRUCTUREBLOCK_MODE),
            Some(StructureMode::Corner),
            "the block shows the loaded mode"
        );

        let mut saved = NbtCompound::new();
        block_entity.save_additional(&mut saved);
        let text = |key: &str| saved.string(key).map(ToString::to_string);
        assert_eq!(text("mode").as_deref(), Some("CORNER"));
        assert_eq!(text("rotation").as_deref(), Some("CLOCKWISE_90"));
        assert_eq!(text("mirror").as_deref(), Some("LEFT_RIGHT"));
        assert_eq!(saved.byte("showboundingbox"), Some(1));
        assert_eq!(saved.long("seed"), Some(0));
    });
}
