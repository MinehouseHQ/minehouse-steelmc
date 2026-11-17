use std::io::Cursor;
use std::sync::Arc;

use simdnbt::borrow::read_compound;
use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_entities};
use steel_utils::nbt::parse_snbt_compound;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos};

use super::*;
use crate::behavior::init_behaviors;
use crate::entity::{SharedEntity, init_entities, next_entity_id};
use crate::test_support::{TestWorld, fresh_test_world, insert_ready_full_chunk};

fn world(key: &'static str) -> TestWorld {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    let fixture = fresh_test_world(key);
    insert_ready_full_chunk(&fixture.world, ChunkPos::new(0, 0));
    fixture
}

fn mannequin(world: &Arc<World>, position: DVec3) -> Arc<MannequinEntity> {
    Arc::new(MannequinEntity::new(
        &vanilla_entities::MANNEQUIN,
        next_entity_id(),
        position,
        Arc::downgrade(world),
    ))
}

fn load(mannequin: &MannequinEntity, snbt: &str) {
    let nbt = parse_snbt_compound(snbt).expect("valid snbt");
    let mut bytes = Vec::new();
    nbt.write(&mut bytes);
    let borrowed = read_compound(&mut Cursor::new(&bytes)).expect("nbt");
    mannequin.load_additional((&borrowed).into());
}

fn save(mannequin: &MannequinEntity) -> NbtCompound {
    let mut nbt = NbtCompound::new();
    mannequin.save_additional(&mut nbt);
    nbt
}

#[test]
fn a_new_mannequin_shows_every_layer_and_the_npc_label() {
    let fixture = world("mannequin_defaults");
    let mannequin = mannequin(&fixture.world, DVec3::new(8.5, 70.0, 8.5));

    assert_eq!(mannequin.shown_layers(), ALL_LAYERS);
    assert_eq!(mannequin.main_arm(), HumanoidArm::Right);
    assert_eq!(mannequin.description(), Some(default_description()));

    let saved = save(&mannequin);
    assert_eq!(
        saved
            .list("hidden_layers")
            .and_then(NbtList::strings)
            .map(<[_]>::len),
        Some(0),
        "no layer is hidden"
    );
    assert_eq!(
        saved
            .string("main_hand")
            .map(ToString::to_string)
            .as_deref(),
        Some("right")
    );
    assert_eq!(
        saved.string("pose").map(ToString::to_string).as_deref(),
        Some("standing")
    );
    assert_eq!(saved.byte("immovable"), Some(0));
    assert!(saved.get("description").is_none());
    assert!(saved.get("hide_description").is_none());
    assert!(saved.get("profile").is_some());
}

#[test]
fn layers_hand_pose_and_label_round_trip_through_nbt() {
    let fixture = world("mannequin_round_trip");
    let mannequin = mannequin(&fixture.world, DVec3::new(8.5, 70.0, 8.5));
    load(
        &mannequin,
        "{hidden_layers: [\"hat\", \"cape\"], main_hand: \"left\", pose: \"crouching\", \
         immovable: 1b, description: \"Shopkeeper\"}",
    );

    assert_eq!(mannequin.shown_layers(), ALL_LAYERS & !(1 | 1 << 6));
    assert_eq!(mannequin.main_arm(), HumanoidArm::Left);
    assert_eq!(mannequin.pose(), EntityPose::Sneaking);
    assert!((mannequin.bounding_box().height() - 1.5).abs() < 1e-6);
    assert!(mannequin.is_immovable());
    assert_eq!(
        mannequin.description(),
        Some(TextComponent::plain("Shopkeeper"))
    );

    let saved = save(&mannequin);
    let reloaded = self::mannequin(&fixture.world, DVec3::new(8.5, 70.0, 8.5));
    let mut bytes = Vec::new();
    saved.write(&mut bytes);
    let borrowed = read_compound(&mut Cursor::new(&bytes)).expect("nbt");
    reloaded.load_additional((&borrowed).into());

    assert_eq!(reloaded.shown_layers(), mannequin.shown_layers());
    assert_eq!(reloaded.main_arm(), HumanoidArm::Left);
    assert_eq!(reloaded.pose(), EntityPose::Sneaking);
    assert!(reloaded.is_immovable());
    assert_eq!(reloaded.description(), mannequin.description());
}

#[test]
fn invalid_layers_and_poses_fall_back_and_a_hidden_label_stays_hidden() {
    let fixture = world("mannequin_fallbacks");
    let mannequin = mannequin(&fixture.world, DVec3::new(8.5, 70.0, 8.5));
    load(
        &mannequin,
        "{hidden_layers: [\"hat\", \"tail\"], pose: \"dying\", hide_description: 1b}",
    );

    assert_eq!(mannequin.shown_layers(), ALL_LAYERS);
    assert_eq!(mannequin.pose(), EntityPose::Standing);
    assert_eq!(mannequin.description(), None);
    let saved = save(&mannequin);
    assert_eq!(saved.byte("hide_description"), Some(1));
    assert!(saved.get("description").is_none());
}

#[test]
fn an_immovable_mannequin_hangs_in_the_air_and_a_free_one_falls() {
    let fixture = world("mannequin_gravity");
    let world = &fixture.world;
    assert!(world.set_block(
        BlockPos::new(8, 63, 8),
        vanilla_blocks::STONE.default_state(),
        UpdateFlags::UPDATE_ALL,
    ));

    let free = mannequin(world, DVec3::new(8.5, 70.0, 8.5));
    let pinned = mannequin(world, DVec3::new(10.5, 70.0, 8.5));
    pinned.set_immovable(true);
    for entity in [&free, &pinned] {
        assert!(
            world
                .try_add_entity(Arc::clone(entity) as SharedEntity)
                .is_ok()
        );
    }

    for _ in 0..10 {
        Arc::clone(&free).tick();
        Arc::clone(&pinned).tick();
    }

    assert!(free.position().y < 70.0);
    assert!((pinned.position().y - 70.0).abs() < 1e-9);
}
