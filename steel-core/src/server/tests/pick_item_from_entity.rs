//! Pick block on entities and the spectator menu's teleport, from vanilla
//! `ServerGamePacketListenerImpl.handlePickItemFromEntity` and
//! `handleTeleportToEntityPacket`.

use std::sync::Arc;

use steel_protocol::packets::game::{SPickItemFromEntity, STeleportToEntity};
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_items};
use steel_utils::types::GameType;
use steel_utils::{ChunkPos, WorldAabb};

use super::insert_ready_full_chunk;
use super::recipe_command::join_with_packets;
use super::scoreboard_command::{run, with_server_world};
use crate::behavior::init_behaviors;
use crate::entity::{Entity as _, SharedEntity, init_entities};
use crate::inventory::container::Container as _;
use crate::player::Player;
use crate::server::Server;
use crate::world::World;

/// Summons `entity` without gravity at `x`, on the player's row.
fn summon_at(server: &Arc<Server>, world: &World, entity: &str, x: f64) -> SharedEntity {
    assert_eq!(
        run(
            server,
            &format!("summon {entity} {x} 65 0.5 {{NoGravity:1b}}")
        ),
        Some(1)
    );
    let area = WorldAabb::new(x - 0.1, 64.0, 0.4, x + 0.1, 67.0, 0.6);
    let mut entities = world.get_entities_in_aabb_matching(&area, |entity| {
        entity.as_player().is_none() && (entity.position().x - x).abs() < 1e-9
    });
    assert_eq!(entities.len(), 1, "one {entity} stands at x = {x}");
    entities.remove(0)
}

/// Summons `entity` without gravity right next to the player's spot.
fn summon_next_to_player(server: &Arc<Server>, world: &World, entity: &str) -> SharedEntity {
    summon_at(server, world, entity, 1.5)
}

fn standing_player(server: &Arc<Server>, world: &Arc<World>, game_mode: GameType) -> Arc<Player> {
    insert_ready_full_chunk(world, ChunkPos::new(0, 0));
    let (player, _packets) = join_with_packets(server, world, "Alex", 10_000);
    assert_eq!(run(server, "tp Alex 0.5 65 0.5"), Some(1));
    assert!(player.set_game_mode(game_mode) || player.game_mode() == game_mode);
    player
}

fn pick(player: &Arc<Player>, server: &Arc<Server>, entity: &SharedEntity) {
    player.handle_pick_item_from_entity(
        &SPickItemFromEntity {
            id: entity.id(),
            include_data: false,
        },
        server,
    );
}

fn held(player: &Player) -> ItemStack {
    player.inventory.lock().get_selected_item().clone()
}

fn setup() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
}

#[test]
fn creative_pick_on_a_mob_gives_its_spawn_egg() {
    setup();
    with_server_world("pick_entity_mob", |server, world| {
        let player = standing_player(server, world, GameType::Creative);
        let cow = summon_next_to_player(server, world, "minecraft:cow");

        pick(&player, server, &cow);

        assert!(held(&player).is(&vanilla_items::COW_SPAWN_EGG));
    });
}

#[test]
fn pick_results_follow_each_entity_like_vanilla() {
    setup();
    let cases: [(&str, EntityTypeRef, ItemRef); 5] = [
        (
            "minecraft:command_block_minecart",
            &vanilla_entities::COMMAND_BLOCK_MINECART,
            &vanilla_items::COMMAND_BLOCK_MINECART,
        ),
        (
            "minecraft:spawner_minecart",
            &vanilla_entities::SPAWNER_MINECART,
            &vanilla_items::MINECART,
        ),
        (
            "minecraft:oak_boat",
            &vanilla_entities::OAK_BOAT,
            &vanilla_items::OAK_BOAT,
        ),
        (
            "minecraft:armor_stand",
            &vanilla_entities::ARMOR_STAND,
            &vanilla_items::ARMOR_STAND,
        ),
        (
            "minecraft:end_crystal",
            &vanilla_entities::END_CRYSTAL,
            &vanilla_items::END_CRYSTAL,
        ),
    ];
    with_server_world("pick_entity_kinds", |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        for (index, (id, entity_type, item)) in cases.into_iter().enumerate() {
            let x = 3.0 * f64::from(u8::try_from(index).expect("few cases")) + 0.5;
            let entity = summon_at(server, world, id, x);
            assert_eq!(entity.entity_type(), entity_type);
            let picked = entity
                .pick_result()
                .unwrap_or_else(|| panic!("{id} has a pick result"));
            assert!(picked.is(item), "{id} picks {}", item.key);
        }
        let marker = summon_at(server, world, "minecraft:marker", 15.5);
        assert!(marker.pick_result().is_none(), "a marker picks nothing");
    });
}

#[test]
fn survival_pick_selects_a_matching_stack_and_never_adds_one() {
    setup();
    with_server_world("pick_entity_survival", |server, world| {
        let player = standing_player(server, world, GameType::Survival);
        let minecart = summon_next_to_player(server, world, "minecraft:minecart");

        pick(&player, server, &minecart);
        assert!(held(&player).is_empty(), "survival pick adds nothing");

        player
            .inventory
            .lock()
            .set_item(20, ItemStack::new(&vanilla_items::MINECART));
        pick(&player, server, &minecart);

        assert!(held(&player).is(&vanilla_items::MINECART));
        assert!(player.inventory.lock().get_item(20).is_empty());
    });
}

#[test]
fn creative_pick_with_a_full_hotbar_keeps_the_replaced_stack() {
    setup();
    with_server_world("pick_entity_full_hotbar", |server, world| {
        let player = standing_player(server, world, GameType::Creative);
        let armor_stand = summon_next_to_player(server, world, "minecraft:armor_stand");
        {
            let mut inventory = player.inventory.lock();
            for slot in 0..9 {
                inventory.set_item(slot, ItemStack::new(&vanilla_items::STONE));
            }
            inventory.set_selected_slot(4);
        }

        pick(&player, server, &armor_stand);

        let inventory = player.inventory.lock();
        assert_eq!(inventory.get_selected_slot(), 4);
        assert!(
            inventory
                .get_selected_item()
                .is(&vanilla_items::ARMOR_STAND)
        );
        assert!(
            inventory.get_item(9).is(&vanilla_items::STONE),
            "the replaced stone moves to the first free slot"
        );
    });
}

#[test]
fn spectators_teleport_to_the_chosen_entity() {
    setup();
    with_server_world("teleport_to_entity", |server, world| {
        let player = standing_player(server, world, GameType::Survival);
        assert_eq!(
            run(
                server,
                "summon minecraft:pig 7.5 66 3.5 {NoGravity:1b,Rotation:[90f,0f]}"
            ),
            Some(1)
        );
        let area = WorldAabb::new(7.0, 65.0, 3.0, 8.0, 68.0, 4.0);
        let pig = world
            .get_entities_in_aabb_matching(&area, |entity| {
                entity.entity_type() == &vanilla_entities::PIG
            })
            .remove(0);
        let request = STeleportToEntity { uuid: pig.uuid() };

        player.handle_teleport_to_entity(&request, server);
        assert!(
            (player.position().x - 0.5).abs() < 1e-9,
            "only spectators may teleport"
        );

        assert!(player.set_game_mode(GameType::Spectator));
        player.handle_teleport_to_entity(&request, server);

        assert_eq!(player.position(), pig.position());
        assert!((player.rotation().0 - 90.0).abs() < 1e-3);
    });
}

#[test]
fn fetchprofile_entity_reports_mannequins() {
    setup();
    with_server_world("fetchprofile_mannequin", |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        assert_eq!(
            run(server, "summon minecraft:mannequin 0.5 65 0.5"),
            Some(1)
        );
        assert_eq!(
            run(
                server,
                "fetchprofile entity @e[type=minecraft:mannequin,limit=1]"
            ),
            Some(1)
        );
    });
}
