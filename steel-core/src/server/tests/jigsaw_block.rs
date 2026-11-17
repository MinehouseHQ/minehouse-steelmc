//! Jigsaw blocks: their saved settings and the "Generate" button, vanilla
//! `JigsawBlockEntity.generate`.

use std::io::Cursor;
use std::sync::Arc;
use std::thread;

use simdnbt::borrow::read_compound;
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, FrontAndTop};
use steel_registry::template_pool::JointType;
use steel_registry::{init_vanilla_registry, vanilla_blocks};
use steel_utils::threading::DEBUG_STACK_SIZE;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos, Downcast as _, Identifier};

use super::insert_ready_full_chunk;
use super::scoreboard_command::with_server_world;
use crate::behavior::init_behaviors;
use crate::block_entity::entities::{JigsawBlockEntity, JigsawSettings, default_joint};
use crate::block_entity::{BlockEntity as _, SharedBlockEntity, init_block_entities};
use crate::entity::init_entities;
use crate::world::World;

fn init() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
}

fn jigsaw(block_entity: &SharedBlockEntity) -> &JigsawBlockEntity {
    block_entity
        .downcast_ref::<JigsawBlockEntity>()
        .expect("jigsaw block entity")
}

/// Places a jigsaw facing east at `pos` that grows the melee spawner
/// connector of trial chambers from its `spawner_connector` jigsaw.
fn place_connector_jigsaw(world: &Arc<World>, pos: BlockPos) -> SharedBlockEntity {
    let state = vanilla_blocks::JIGSAW
        .default_state()
        .set_value(&BlockStateProperties::ORIENTATION, FrontAndTop::EastUp);
    assert!(world.set_block(pos, state, UpdateFlags::UPDATE_ALL));
    let block_entity = world
        .get_block_entity(pos)
        .expect("a jigsaw has a block entity");
    let mut settings = jigsaw(&block_entity).settings();
    settings.pool = Identifier::vanilla_static("trial_chambers/spawner/melee");
    settings.target = Identifier::vanilla_static("spawner_connector");
    jigsaw(&block_entity).set_settings(settings);
    block_entity
}

fn with_world(name: &'static str, test: impl FnOnce(&Arc<World>) + Send + 'static) {
    init();
    // Structure placement overflows the default test stack in debug builds.
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

#[test]
fn generate_places_the_pool_in_front_of_the_jigsaw() {
    with_world("jigsaw_generate", |world| {
        let pos = BlockPos::new(4, 64, 8);
        // The named jigsaw lands in front of the block, one lower: vanilla
        // sinks a start piece by its ground level delta of 1.
        let front = BlockPos::new(5, 63, 8);
        let block_entity = place_connector_jigsaw(world, pos);
        world.set_block(
            front,
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        assert!(jigsaw(&block_entity).generate(world, 0, false));
        assert_eq!(
            world.get_block_state(front).get_block(),
            &vanilla_blocks::AIR,
            "the start jigsaw turns into its final state"
        );

        world.set_block(
            front,
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        assert!(jigsaw(&block_entity).generate(world, 0, true));
        assert_eq!(
            world.get_block_state(front).get_block(),
            &vanilla_blocks::JIGSAW,
            "kept jigsaws stay in the world"
        );
        let kept = world.get_block_entity(front).expect("kept jigsaw entity");
        assert_eq!(
            jigsaw(&kept).settings().name,
            Identifier::vanilla_static("spawner_connector")
        );

        let mut settings = jigsaw(&block_entity).settings();
        settings.pool = Identifier::vanilla_static("no_such_pool");
        jigsaw(&block_entity).set_settings(settings);
        assert!(!jigsaw(&block_entity).generate(world, 0, false));
    });
}

#[test]
fn jigsaw_settings_save_under_vanilla_keys() {
    with_world("jigsaw_nbt", |world| {
        let pos = BlockPos::new(2, 64, 2);
        let block_entity = place_connector_jigsaw(world, pos);
        let mut settings = jigsaw(&block_entity).settings();
        assert_eq!(settings.joint, JointType::Aligned, "sideways jigsaws align");
        settings.name = Identifier::vanilla_static("bottom");
        settings.final_state = "minecraft:stone".to_owned();
        settings.joint = JointType::Rollable;
        settings.placement_priority = 3;
        settings.selection_priority = -2;
        jigsaw(&block_entity).set_settings(settings.clone());

        let mut nbt = NbtCompound::new();
        block_entity.save_additional(&mut nbt);
        assert_eq!(
            nbt.string("pool").map(ToString::to_string).as_deref(),
            Some("minecraft:trial_chambers/spawner/melee")
        );
        assert_eq!(
            nbt.string("joint").map(ToString::to_string).as_deref(),
            Some("rollable")
        );
        assert_eq!(nbt.int("placement_priority"), Some(3));

        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let borrowed = read_compound(&mut Cursor::new(bytes.as_slice())).expect("nbt");
        let state = world.get_block_state(pos);
        let loaded = JigsawBlockEntity::new(Arc::downgrade(world), pos, state);
        loaded.load_additional(&borrowed);
        assert_eq!(loaded.settings(), settings);

        let mut bytes = Vec::new();
        NbtCompound::new().write(&mut bytes);
        let empty = read_compound(&mut Cursor::new(bytes.as_slice())).expect("empty nbt");
        loaded.load_additional(&empty);
        assert_eq!(loaded.settings(), JigsawSettings::new(state));
        let up = vanilla_blocks::JIGSAW
            .default_state()
            .set_value(&BlockStateProperties::ORIENTATION, FrontAndTop::UpNorth);
        assert_eq!(default_joint(up), JointType::Rollable);
    });
}
