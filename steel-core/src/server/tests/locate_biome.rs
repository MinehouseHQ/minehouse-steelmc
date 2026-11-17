//! `/locate biome`: vanilla `LocateCommand.locateBiome`.

use steel_registry::init_vanilla_registry;

use super::scoreboard_command::{run, with_server_world};
use crate::behavior::init_behaviors;

#[test]
fn locate_biome_finds_the_worlds_biome_and_rejects_others() {
    init_vanilla_registry();
    init_behaviors();
    with_server_world("locate_biome", |server, _world| {
        assert!(
            run(server, "locate biome minecraft:plains").is_some(),
            "the test world's generator places plains everywhere"
        );
        assert!(
            run(server, "locate biome #minecraft:is_overworld").is_some(),
            "a tag matching plains is found too"
        );
        assert_eq!(
            run(server, "locate biome minecraft:desert"),
            None,
            "a biome the generator never places is not found"
        );
    });
}
