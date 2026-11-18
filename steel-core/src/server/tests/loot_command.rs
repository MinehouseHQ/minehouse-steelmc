//! `/loot` run through the server's dispatcher: sources roll the vanilla
//! loot tables and targets take the drops the way `LootCommand` hands them.

use std::sync::Arc;

use steel_registry::item_stack::ItemStack;
use steel_registry::{REGISTRY, RegistryExt as _, init_vanilla_registry};
use steel_utils::{BlockPos, ChunkPos, Identifier};

use super::insert_ready_full_chunk;
use super::recipe_command::join_with_packets;
use super::scoreboard_command::{run, with_server_world};
use crate::behavior::init_behaviors;
use crate::block_entity::init_block_entities;
use crate::command::execution::CommandSource;
use crate::command::sender::CommandSender;
use crate::entity::init_entities;
use crate::inventory::container::Container as _;
use crate::inventory::lock::{ContainerLockGuard, ContainerRef};
use crate::server::Server;
use crate::world::World;

const CHEST: BlockPos = BlockPos::new(0, 64, 0);

pub(super) fn setup(world: &Arc<World>, server: &Arc<Server>) {
    insert_ready_full_chunk(world, ChunkPos::new(0, 0));
    assert_eq!(run(server, "setblock 0 64 0 minecraft:chest"), Some(1));
    assert_eq!(run(server, "setblock 1 64 1 minecraft:stone"), Some(1));
}

/// The parse error `command` fails with.
pub(super) fn parse_error(server: &Arc<Server>, command: &str) -> String {
    let source = CommandSource::new(CommandSender::Console, Arc::clone(server));
    let dispatcher = server.command_dispatcher.read();
    let parse = dispatcher.parse(command, source);
    match dispatcher.context_chain(parse) {
        Ok(_) => panic!("{command} should not parse"),
        Err(error) => error.to_string(),
    }
}

pub(super) fn chest_slot(world: &World, slot: usize) -> ItemStack {
    let container = world
        .get_block_entity(CHEST)
        .and_then(ContainerRef::from_block_entity)
        .expect("the chest has a container");
    let id = container.container_id();
    ContainerLockGuard::lock_all(&[&container])
        .get(id)
        .map(|chest| chest.get_item(slot).clone())
        .expect("the chest is locked")
}

fn is_cobblestone(stack: &ItemStack, count: i32) -> bool {
    stack.item().key == Identifier::vanilla_static("cobblestone") && stack.count() == count
}

#[test]
fn insert_merges_mined_drops_into_the_container() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
    with_server_world("loot_insert", |server, world| {
        setup(world, server);
        // Stone mined without silk touch drops one cobblestone.
        assert_eq!(run(server, "loot insert 0 64 0 mine 1 64 1"), Some(1));
        assert!(is_cobblestone(&chest_slot(world, 0), 1));
        assert_eq!(run(server, "loot insert 0 64 0 mine 1 64 1"), Some(1));
        assert!(is_cobblestone(&chest_slot(world, 0), 2));
        assert!(chest_slot(world, 1).is_empty());
        // Silk touch keeps the stone itself.
        assert_eq!(
            run(
                server,
                "loot insert 0 64 0 mine 1 64 1 minecraft:diamond_pickaxe[minecraft:enchantments={'minecraft:silk_touch':1}]"
            ),
            Some(1)
        );
        assert_eq!(
            chest_slot(world, 1).item().key,
            Identifier::vanilla_static("stone")
        );
    });
}

#[test]
fn replace_fills_the_counted_slots_and_clears_the_rest() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
    with_server_world("loot_replace_block", |server, world| {
        setup(world, server);
        assert_eq!(run(server, "loot insert 0 64 0 mine 1 64 1"), Some(1));
        // One drop over three slots: the cobblestone, then two empty stacks,
        // all of which vanilla counts.
        assert_eq!(
            run(
                server,
                "loot replace block 0 64 0 container.0 3 mine 1 64 1"
            ),
            Some(3)
        );
        assert!(is_cobblestone(&chest_slot(world, 0), 1));
        // Without a count, only as many slots as there are drops.
        assert_eq!(
            run(server, "loot replace block 0 64 0 container.5 mine 1 64 1"),
            Some(1)
        );
        assert!(is_cobblestone(&chest_slot(world, 5), 1));
        // A chest has 27 slots and the slot must name exactly one.
        assert_eq!(
            run(server, "loot replace block 0 64 0 container.30 mine 1 64 1"),
            None
        );
        assert!(
            parse_error(server, "loot replace block 0 64 0 container.* mine 1 64 1")
                .contains("slot.only_single_allowed")
        );
        assert!(
            parse_error(server, "loot replace block 0 64 0 nope mine 1 64 1")
                .contains("slot.unknown")
        );
        // The stone is not a container.
        assert_eq!(
            run(server, "loot replace block 1 64 1 container.0 mine 1 64 1"),
            None
        );
    });
}

#[test]
fn sources_without_a_table_fail() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
    with_server_world("loot_sources", |server, world| {
        setup(world, server);
        // Air has no loot table.
        assert_eq!(run(server, "loot spawn 0 70 0 mine 3 70 3"), None);
        // Markers are built without one.
        assert_eq!(run(server, "summon minecraft:marker 0.5 70 0.5"), Some(1));
        assert_eq!(
            run(
                server,
                "loot spawn 0 70 0 kill @e[type=minecraft:marker,limit=1]"
            ),
            None
        );
        // Unknown ids and inline tables do not parse.
        assert!(
            parse_error(server, "loot spawn 0 70 0 loot minecraft:nope")
                .contains("argument.resource_or_id.no_such_element")
        );
        assert!(
            parse_error(server, "loot spawn 0 70 0 loot {pools:[]}")
                .contains("argument.resource_or_id.failed_to_parse")
        );
    });
}

#[test]
fn kill_and_chest_sources_spawn_their_drops() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
    with_server_world("loot_spawn", |server, world| {
        setup(world, server);
        assert!(
            REGISTRY
                .loot_tables
                .by_key(&Identifier::vanilla_static("entities/pig"))
                .is_some()
        );
        assert_eq!(run(server, "summon minecraft:pig 0.5 70 0.5"), Some(1));
        // A pig drops one to three porkchops.
        let killed = run(
            server,
            "loot spawn 0 70 0 kill @e[type=minecraft:pig,limit=1]",
        );
        assert!(
            killed.is_some_and(|count| (1..=3).contains(&count)),
            "{killed:?}"
        );
        // Every simple dungeon chest has items.
        let chest = run(
            server,
            "loot spawn 0 70 0 loot minecraft:chests/simple_dungeon",
        );
        assert!(chest.is_some_and(|count| count > 0), "{chest:?}");
    });
}

#[test]
fn give_and_entity_replace_take_the_drops() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
    with_server_world("loot_give", |server, world| {
        setup(world, server);
        let (player, _packets) = join_with_packets(server, world, "Looter", 900_001);
        assert_eq!(run(server, "loot give Looter mine 1 64 1"), Some(1));
        let held = player
            .inventory
            .lock()
            .iter()
            .any(|stack| is_cobblestone(stack, 1));
        assert!(held);

        assert_eq!(run(server, "summon minecraft:zombie 0.5 70 0.5"), Some(1));
        assert_eq!(
            run(
                server,
                "loot replace entity @e[type=minecraft:zombie] weapon.mainhand mine 1 64 1"
            ),
            Some(1)
        );
        // Copy the zombie's hand into the chest to look at it.
        assert_eq!(
            run(
                server,
                "item replace block 0 64 0 container.10 from entity @e[type=minecraft:zombie] weapon.mainhand"
            ),
            Some(1)
        );
        assert!(is_cobblestone(&chest_slot(world, 10), 1));
    });
}
