//! `/recipe` and the recipe book it changes, run through the server's
//! dispatcher the way vanilla's `RecipeCommand` reports results.

use std::sync::Arc;

use steel_protocol::packet_traits::EncodedPacket;
use steel_registry::REGISTRY;
use steel_registry::packets::play::{C_RECIPE_BOOK_ADD, C_RECIPE_BOOK_REMOVE};
use steel_utils::Identifier;
use steel_utils::locks::SyncMutex;

use super::super::Server;
use super::scoreboard_command::{run, with_server_world};
use super::{chat_packets, test_player_with_packets};
use crate::inventory::recipe_displays::RECIPE_DISPLAYS;
use crate::player::{Player, ResetReason};
use crate::world::World;

fn displays_of(key: &'static str) -> i32 {
    let count = RECIPE_DISPLAYS
        .displays_for_recipe(&Identifier::vanilla_static(key))
        .count();
    i32::try_from(count).expect("display counts fit an int")
}

fn learnable_displays() -> i32 {
    RECIPE_DISPLAYS
        .learnable()
        .iter()
        .map(|recipe| RECIPE_DISPLAYS.displays_for_recipe(recipe.key()).count())
        .sum::<usize>()
        .try_into()
        .expect("display counts fit an int")
}

/// Adds an online player who has joined `world`, with the packets sent to them.
fn join_with_packets(
    server: &Arc<Server>,
    world: &Arc<World>,
    name: &str,
    id: i32,
) -> (Arc<Player>, Arc<SyncMutex<Vec<EncodedPacket>>>) {
    let (player, packets) = test_player_with_packets(server, Arc::clone(world), name, id);
    assert!(server.online_players.insert(Arc::clone(&player)));
    assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));
    let _ = player.mark_joined_world();
    (player, packets)
}

fn join(server: &Arc<Server>, world: &Arc<World>, name: &str, id: i32) -> Arc<Player> {
    join_with_packets(server, world, name, id).0
}

#[test]
fn recipes_are_given_and_taken_once() {
    with_server_world("recipe_give_take", |server, world| {
        let (alex, packets) = join_with_packets(server, world, "Alex", 1);
        let planks = Identifier::vanilla_static("oak_planks");

        assert_eq!(
            run(server, "recipe give Alex minecraft:oak_planks"),
            Some(displays_of("oak_planks"))
        );
        assert!(alex.knows_recipe(&planks));
        assert!(alex.recipe_book().is_highlighted(&planks));
        assert_eq!(chat_packets(&packets, C_RECIPE_BOOK_ADD).len(), 1);
        // Known recipes add nothing, so vanilla fails the command.
        assert_eq!(run(server, "recipe give Alex minecraft:oak_planks"), None);
        assert_eq!(chat_packets(&packets, C_RECIPE_BOOK_ADD).len(), 1);
        // Special and missing recipes are `recipe.notFound`.
        assert_eq!(run(server, "recipe give Alex minecraft:book_cloning"), None);
        assert_eq!(run(server, "recipe give Alex minecraft:not_a_recipe"), None);

        assert_eq!(
            run(server, "recipe take Alex minecraft:oak_planks"),
            Some(displays_of("oak_planks"))
        );
        assert!(!alex.knows_recipe(&planks));
        assert!(!alex.recipe_book().is_highlighted(&planks));
        assert_eq!(chat_packets(&packets, C_RECIPE_BOOK_REMOVE).len(), 1);
        assert_eq!(run(server, "recipe take Alex minecraft:oak_planks"), None);
    });
}

#[test]
fn every_learnable_recipe_is_given_to_every_target() {
    with_server_world("recipe_give_all", |server, world| {
        let alex = join(server, world, "Alex", 1);
        let steve = join(server, world, "Steve", 2);
        let all = learnable_displays();
        assert!(all > 0);

        assert_eq!(
            run(server, "recipe give Alex minecraft:torch"),
            Some(displays_of("torch"))
        );
        assert_eq!(
            run(server, "recipe give @a *"),
            Some(all - displays_of("torch") + all)
        );
        for player in [&alex, &steve] {
            assert_eq!(
                RECIPE_DISPLAYS
                    .learnable()
                    .iter()
                    .filter(|recipe| !player.knows_recipe(recipe.key()))
                    .count(),
                0
            );
        }
        assert_eq!(run(server, "recipe give @a *"), None);
        assert_eq!(run(server, "recipe take Steve *"), Some(all));
        assert_eq!(run(server, "recipe take @a *"), Some(all));
    });
}

#[test]
fn unlocking_a_recipe_completes_its_recipe_advancement() {
    with_server_world("recipe_advancement", |server, world| {
        let alex = join(server, world, "Alex", 1);
        let advancement = Identifier::vanilla_static("recipes/building_blocks/oak_planks");
        assert!(REGISTRY.advancements.by_key(&advancement).is_some());
        let done = |player: &Player| {
            player
                .advancements
                .lock()
                .persisted_progress()
                .into_iter()
                .any(|(key, progress)| key == advancement && progress.is_done())
        };
        assert!(!done(&alex));
        assert_eq!(
            run(server, "recipe give Alex minecraft:oak_planks"),
            Some(displays_of("oak_planks"))
        );
        assert!(done(&alex), "recipe_unlocked completes the advancement");

        // Advancement rewards unlock their recipes.
        let steve = join(server, world, "Steve", 2);
        let stairs = Identifier::vanilla_static("oak_stairs");
        assert!(!steve.knows_recipe(&stairs));
        assert_eq!(
            run(
                server,
                "advancement grant Steve only minecraft:recipes/building_blocks/oak_stairs"
            ),
            Some(1)
        );
        assert!(steve.knows_recipe(&stairs));
    });
}
