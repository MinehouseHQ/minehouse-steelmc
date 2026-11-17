//! `/summon` with entity data: vanilla `SummonCommand.createEntity`.

use steel_registry::{init_vanilla_registry, vanilla_entities};
use steel_utils::{ChunkPos, WorldAabb};

use super::insert_ready_full_chunk;
use super::scoreboard_command::{run, with_server_world};
use crate::behavior::init_behaviors;
use crate::entity::init_entities;

#[test]
fn summon_loads_the_given_data_and_passengers() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    with_server_world("summon_with_data", |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        assert_eq!(
            run(
                server,
                "summon minecraft:pig 4.5 65 4.5 {NoGravity:1b,Passengers:[{id:\"minecraft:chicken\"}]}"
            ),
            Some(1)
        );
        let area = WorldAabb::new(0.0, 60.0, 0.0, 16.0, 80.0, 16.0);
        let pigs = world.get_entities_in_aabb_matching(&area, |entity| {
            entity.entity_type() == &vanilla_entities::PIG
        });
        let [pig] = pigs.as_slice() else {
            panic!("one pig is summoned, found {}", pigs.len());
        };
        assert!(
            (pig.position().x - 4.5).abs() < 1e-9,
            "the pig is placed at the given position"
        );
        assert_eq!(
            run(
                server,
                "data get entity @e[type=minecraft:pig,limit=1] NoGravity"
            ),
            Some(1)
        );
        let passengers = pig.base().passengers();
        assert_eq!(passengers.len(), 1, "the chicken rides the pig");
        assert_eq!(passengers[0].entity_type(), &vanilla_entities::CHICKEN);
        assert_eq!(
            run(server, "summon minecraft:pig 4.5 65 4.5 {UUID:[I;1,2,3,4]}"),
            Some(1)
        );
        assert_eq!(
            run(server, "summon minecraft:pig 4.5 65 4.5 {UUID:[I;1,2,3,4]}"),
            None,
            "a duplicate UUID is rejected"
        );
    });
}

#[test]
fn execute_store_writes_into_an_entitys_data() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    with_server_world("execute_store_entity", |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let pig = "@e[type=minecraft:pig,limit=1]";
        assert_eq!(run(server, "summon minecraft:pig 4.5 65 4.5"), Some(1));
        assert!(run(server, "scoreboard objectives add stored dummy").is_some());
        assert!(run(server, "scoreboard players set value stored 123").is_some());
        assert_eq!(
            run(
                server,
                &format!(
                    "execute store result entity {pig} Air short 1 run scoreboard players get value stored"
                )
            ),
            Some(123)
        );
        assert_eq!(
            run(server, &format!("data get entity {pig} Air")),
            Some(123)
        );
    });
}
