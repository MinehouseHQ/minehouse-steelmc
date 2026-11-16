use std::io::Cursor;
use std::sync::Weak;

use glam::DVec3;
use simdnbt::borrow::read_compound;
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::equipment::EquipmentSlot;
use steel_registry::fluid::FluidStateExt as _;
use steel_registry::item_stack::ItemStack;
use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_entities, vanilla_items};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos, Downcast as _, WorldAabb};

use super::{BABY_SCALE, HappyGhastEntity, is_tempted_by};
use crate::behavior::{BLOCK_BEHAVIORS, init_behaviors};
use crate::block_entity::init_block_entities;
use crate::entity::{AgeableMob, Animal, Entity, EntityPose, LivingEntity, init_entities};
use crate::test_support::{fresh_test_world, insert_ready_full_chunk};

fn test_ghast() -> HappyGhastEntity {
    init_vanilla_registry();
    HappyGhastEntity::new(&vanilla_entities::HAPPY_GHAST, 1, DVec3::ZERO, Weak::new())
}

#[test]
fn ghastling_is_scaled_down_with_its_own_eye_height() {
    let ghast = test_ghast();
    let adult = ghast.dimensions_for_pose(EntityPose::Standing);
    ghast.set_age(-24000);
    assert!(AgeableMob::is_baby(&ghast));
    let baby = ghast.dimensions_for_pose(EntityPose::Standing);
    assert!((baby.width - adult.width * BABY_SCALE).abs() < 1e-6);
    assert!((baby.eye_height - 0.468_75).abs() < 1e-6);
}

#[test]
fn only_grown_ghasts_wear_a_harness_and_fly_riders() {
    let ghast = test_ghast();
    assert!(ghast.can_use_slot(EquipmentSlot::Body));
    assert!(ghast.is_flying_vehicle());
    ghast.set_age(-24000);
    assert!(!ghast.can_use_slot(EquipmentSlot::Body));
    assert!(!ghast.is_flying_vehicle());
    assert!(ghast.can_breathe_underwater());
}

#[test]
fn snowballs_tempt_an_unharnessed_adult_and_never_breed_it() {
    let ghast = test_ghast();
    let snowball = ItemStack::new(&vanilla_items::SNOWBALL);
    assert!(ghast.is_food(&snowball));
    assert!(is_tempted_by(&ghast, &snowball));
    assert!(!ghast.can_fall_in_love());
}

#[test]
fn still_timeout_is_saved_and_raises_the_synced_flag() {
    let ghast = test_ghast();
    assert!(!ghast.is_on_still_timeout());
    let mut nbt = NbtCompound::new();
    nbt.insert("still_timeout", 7);
    let mut bytes = Vec::new();
    nbt.write(&mut bytes);
    let borrowed = read_compound(&mut Cursor::new(&bytes))
        .unwrap_or_else(|error| panic!("test nbt should reborrow: {error}"));
    ghast.load_additional((&borrowed).into());
    assert_eq!(ghast.server_still_timeout(), 7);
    assert!(ghast.stays_still());
    let mut saved = NbtCompound::new();
    ghast.save_additional(&mut saved);
    assert_eq!(saved.int("still_timeout"), Some(7));
}

#[test]
fn soaked_dried_ghast_hatches_a_ghastling_facing_its_way() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
    let fixture = fresh_test_world("dried_ghast_hatch");
    insert_ready_full_chunk(&fixture.world, ChunkPos::new(0, 0));
    let pos = BlockPos::new(4, 64, 4);
    let soaked = vanilla_blocks::DRIED_GHAST
        .default_state()
        .set_value(&BlockStateProperties::WATERLOGGED, true)
        .set_value(&BlockStateProperties::DRIED_GHAST_HYDRATION_LEVELS, 3_u8);
    fixture
        .world
        .set_block(pos, soaked, UpdateFlags::UPDATE_NONE);
    BLOCK_BEHAVIORS
        .get_behavior(&vanilla_blocks::DRIED_GHAST)
        .tick(soaked, &fixture.world, pos);

    assert!(
        fixture
            .world
            .get_block_state(pos)
            .get_fluid_state()
            .is_water()
    );
    let center = DVec3::new(4.5, 64.0, 4.5);
    let ghasts = fixture.world.get_entities_in_aabb_matching(
        &WorldAabb::from_min_max(center - 2.0, center + 2.0),
        |entity| entity.downcast_ref::<HappyGhastEntity>().is_some(),
    );
    assert_eq!(ghasts.len(), 1);
    let ghast = ghasts[0]
        .downcast_ref::<HappyGhastEntity>()
        .expect("a happy ghast");
    assert!(AgeableMob::is_baby(ghast));
    assert!(ghast.position().distance(center) < 1e-6);
}
