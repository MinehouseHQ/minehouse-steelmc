//! `/attribute` run through the server's dispatcher, returning values the
//! way vanilla's `AttributeCommand` does.

use steel_registry::{init_vanilla_registry, vanilla_attributes};
use steel_utils::{ChunkPos, Identifier};

use super::insert_ready_full_chunk;
use super::recipe_command::join_with_packets;
use super::scoreboard_command::{run, with_server_world};
use crate::behavior::init_behaviors;
use crate::entity::{LivingEntity as _, init_entities};

#[test]
fn base_values_are_read_set_and_reset() {
    init_vanilla_registry();
    with_server_world("attribute_base", |server, world| {
        let (alex, _) = join_with_packets(server, world, "Alex", 1);
        let health = "attribute Alex minecraft:max_health";

        assert_eq!(run(server, &format!("{health} get")), Some(20));
        assert_eq!(run(server, &format!("{health} base get 10")), Some(200));
        assert_eq!(run(server, &format!("{health} base set 8.5")), Some(1));
        assert_eq!(run(server, &format!("{health} base get")), Some(8));
        assert_eq!(
            alex.attributes()
                .lock()
                .get_base_value(vanilla_attributes::MAX_HEALTH),
            Some(8.5)
        );
        assert_eq!(run(server, &format!("{health} base reset")), Some(1));
        assert_eq!(run(server, &format!("{health} get")), Some(20));
    });
}

#[test]
fn modifiers_are_added_read_and_removed_once() {
    init_vanilla_registry();
    with_server_world("attribute_modifiers", |server, world| {
        let (alex, _) = join_with_packets(server, world, "Alex", 1);
        let health = "attribute Alex minecraft:max_health modifier";

        assert_eq!(
            run(server, &format!("{health} add test:boost 5 add_value")),
            Some(1)
        );
        // An id can only be on the attribute once.
        assert_eq!(
            run(server, &format!("{health} add test:boost 1 add_value")),
            None
        );
        assert_eq!(
            run(server, "attribute Alex minecraft:max_health get"),
            Some(25)
        );
        assert_eq!(
            run(server, &format!("{health} value get test:boost 2")),
            Some(10)
        );
        assert!(alex.attributes().lock().has_modifier(
            vanilla_attributes::MAX_HEALTH,
            &Identifier::new_static("test", "boost")
        ));

        assert_eq!(run(server, &format!("{health} remove test:boost")), Some(1));
        assert_eq!(run(server, &format!("{health} remove test:boost")), None);
        assert_eq!(run(server, &format!("{health} value get test:boost")), None);
        assert_eq!(
            run(server, "attribute Alex minecraft:max_health get"),
            Some(20)
        );
    });
}

#[test]
fn entities_without_the_attribute_fail() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    with_server_world("attribute_failures", |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        join_with_packets(server, world, "Alex", 1);
        assert_eq!(
            run(server, "summon minecraft:experience_orb 0.5 65 0.5"),
            Some(1)
        );

        // An orb is not living.
        assert_eq!(
            run(
                server,
                "attribute @e[type=minecraft:experience_orb,limit=1] minecraft:max_health get"
            ),
            None
        );
        // Players have no follow range; only mobs do.
        assert_eq!(
            run(server, "attribute Alex minecraft:follow_range base reset"),
            None
        );
        assert_eq!(
            run(server, "attribute Alex minecraft:follow_range get"),
            None
        );
    });
}
