//! `/execute if|unless items` and `slots`, run through the dispatcher.

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::init_vanilla_registry;

use super::loot_command::setup;
use super::scoreboard_command::{run, with_server_world};
use crate::behavior::init_behaviors;
use crate::block_entity::init_block_entities;
use crate::entity::init_entities;

#[test]
fn items_counts_matching_items_in_container_slots() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
    with_server_world("execute_if_items_block", |server, world| {
        setup(world, server);
        assert_eq!(
            run(
                server,
                "item replace block 0 64 0 container.0 with minecraft:cobblestone 5"
            ),
            Some(1)
        );
        assert_eq!(
            run(
                server,
                "item replace block 0 64 0 container.3 with minecraft:cobblestone 2"
            ),
            Some(1)
        );
        assert_eq!(
            run(
                server,
                "execute if items block 0 64 0 container.* minecraft:cobblestone"
            ),
            Some(7)
        );
        assert_eq!(
            run(
                server,
                "execute if items block 0 64 0 container.0 minecraft:cobblestone"
            ),
            Some(5)
        );
        assert_eq!(
            run(
                server,
                "execute if items block 0 64 0 container.* minecraft:stone"
            ),
            None
        );
        assert_eq!(
            run(
                server,
                "execute unless items block 0 64 0 container.* minecraft:stone"
            ),
            Some(1)
        );
        // A chest has 27 container slots.
        assert_eq!(
            run(server, "execute if slots block 0 64 0 container.*"),
            Some(27)
        );
        // The stone is not a container.
        assert_eq!(
            run(server, "execute if items block 1 64 1 container.* *"),
            None
        );
        // The fork runs what follows only when the condition holds.
        assert_eq!(
            run(
                server,
                "execute if items block 0 64 0 container.0 minecraft:cobblestone run setblock 2 64 2 minecraft:stone"
            ),
            Some(1)
        );
        assert!(
            world
                .get_block_state(steel_utils::BlockPos::new(2, 64, 2))
                .get_block()
                .key
                .path
                == "stone"
        );
    });
}

#[test]
fn items_reads_entity_slots() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
    with_server_world("execute_if_items_entity", |server, world| {
        setup(world, server);
        assert_eq!(run(server, "summon minecraft:zombie 0.5 70 0.5"), Some(1));
        assert_eq!(
            run(
                server,
                "execute if items entity @e[type=minecraft:zombie] weapon.mainhand *"
            ),
            None
        );
        assert_eq!(
            run(
                server,
                "loot replace entity @e[type=minecraft:zombie] weapon.mainhand mine 1 64 1"
            ),
            Some(1)
        );
        assert_eq!(
            run(
                server,
                "execute if items entity @e[type=minecraft:zombie] weapon.mainhand minecraft:cobblestone"
            ),
            Some(1)
        );
        assert_eq!(
            run(
                server,
                "execute if slots entity @e[type=minecraft:zombie] weapon.mainhand"
            ),
            Some(1)
        );
    });
}
