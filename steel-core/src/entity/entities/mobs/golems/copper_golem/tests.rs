use std::io::Cursor;
use std::sync::Weak;

use glam::DVec3;
use simdnbt::borrow::read_compound;
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, ChestType, Direction, Pose};
use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_entities};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos, Downcast as _, WorldAabb};

use super::{
    CopperGolemEntity, WeatherState, next_weather_state, previous_weather_state,
    weather_state_by_id,
};
use crate::behavior::blocks::next_pose;
use crate::behavior::init_behaviors;
use crate::entity::Entity;
use crate::test_support::{fresh_test_world, insert_ready_full_chunk};

fn new_golem() -> CopperGolemEntity {
    CopperGolemEntity::new(&vanilla_entities::COPPER_GOLEM, 1, DVec3::ZERO, Weak::new())
}

#[test]
fn weather_states_step_and_stop_at_the_ends() {
    assert_eq!(
        next_weather_state(WeatherState::Unaffected),
        WeatherState::Exposed
    );
    assert_eq!(
        next_weather_state(WeatherState::Oxidized),
        WeatherState::Oxidized
    );
    assert_eq!(
        previous_weather_state(WeatherState::Exposed),
        WeatherState::Unaffected
    );
    assert_eq!(
        previous_weather_state(WeatherState::Unaffected),
        WeatherState::Unaffected
    );
    assert_eq!(weather_state_by_id(7), WeatherState::Unaffected);
}

#[test]
fn statue_poses_cycle_back_to_standing() {
    assert_eq!(next_pose(Pose::Standing), Pose::Sitting);
    assert_eq!(next_pose(Pose::Star), Pose::Standing);
}

#[test]
fn weather_state_and_wax_round_trip() {
    init_vanilla_registry();
    let golem = new_golem();
    golem.set_weather_state(WeatherState::Weathered);
    golem.data.lock().next_weathering_tick = super::IGNORE_WEATHERING_TICK;
    let mut nbt = NbtCompound::new();
    golem.save_additional(&mut nbt);
    assert_eq!(nbt.long("next_weather_age"), Some(-2));
    assert_eq!(
        nbt.string("weather_state")
            .map(|name| name.to_str().into_owned()),
        Some("weathered".to_owned())
    );

    let mut bytes = Vec::new();
    nbt.write(&mut bytes);
    let borrowed = read_compound(&mut Cursor::new(&bytes))
        .unwrap_or_else(|error| panic!("reborrow failed: {error}"));
    let loaded = new_golem();
    loaded.load_additional((&borrowed).into());
    assert_eq!(loaded.weather_state(), WeatherState::Weathered);
    assert!(loaded.is_waxed());
}

#[test]
fn pumpkin_on_copper_builds_a_golem_and_leaves_a_copper_chest() {
    init_vanilla_registry();
    init_behaviors();
    let fixture = fresh_test_world("copper_golem_build");
    let world = &fixture.world;
    let copper_pos = BlockPos::new(4, 64, 4);
    insert_ready_full_chunk(world, ChunkPos::from_block_pos(copper_pos));
    assert!(world.set_block(
        copper_pos,
        vanilla_blocks::EXPOSED_COPPER.default_state(),
        UpdateFlags::UPDATE_ALL,
    ));
    let pumpkin = vanilla_blocks::CARVED_PUMPKIN
        .default_state()
        .set_value(&BlockStateProperties::HORIZONTAL_FACING, Direction::East);
    assert!(world.set_block(copper_pos.above(), pumpkin, UpdateFlags::UPDATE_ALL));

    let chest = world.get_block_state(copper_pos);
    assert_eq!(chest.get_block(), &vanilla_blocks::EXPOSED_COPPER_CHEST);
    assert_eq!(
        chest.get_value(&BlockStateProperties::FACING),
        Direction::East
    );
    assert_eq!(
        chest.get_value(&BlockStateProperties::CHEST_TYPE),
        ChestType::Single
    );
    assert!(world.get_block_state(copper_pos.above()).is_air());

    let area = WorldAabb::new(0.0, 60.0, 0.0, 10.0, 70.0, 10.0);
    let golems = world.get_entities_in_aabb_matching(&area, |entity| {
        entity.entity_type() == &vanilla_entities::COPPER_GOLEM
    });
    assert_eq!(golems.len(), 1);
    let golem = golems
        .first()
        .and_then(|entity| entity.downcast_ref::<CopperGolemEntity>())
        .expect("the built golem is a copper golem");
    assert_eq!(golem.weather_state(), WeatherState::Exposed);
}
