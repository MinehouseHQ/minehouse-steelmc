//! Operator-only client requests: the game rules screen, the difficulty
//! lock, the F3 + I data queries, the tick time chart and the low disk
//! space warning.

use std::time::Duration;

use steel_protocol::packets::game::{
    DEDICATED_SERVER_TICK_TIME, GameRuleEntry, SBlockEntityTagQuery, SEntityTagQuery,
    SLockDifficulty, SSetGameRule,
};
use steel_registry::packets::play::{
    C_DEBUG_SAMPLE, C_LOW_DISK_SPACE_WARNING, C_SYSTEM_CHAT, C_TAG_QUERY,
};
use steel_registry::vanilla_game_rules::{KEEP_INVENTORY, RESPAWN_RADIUS};
use steel_registry::{init_vanilla_registry, vanilla_entities};
use steel_utils::codec::VarInt;
use steel_utils::serial::ReadFrom as _;
use steel_utils::{BlockPos, ChunkPos, Identifier, WorldAabb};

use super::chat_packets;
use super::insert_ready_full_chunk;
use super::packet_body;
use super::recipe_command::join_with_packets;
use super::scoreboard_command::{run, with_server_world};
use crate::behavior::init_behaviors;
use crate::entity::init_entities;
use crate::permission::{
    OP_GROUP, PermissionEntry, PermissionKey, PermissionMetadataSet, PermissionSet,
    PermissionSubjectState,
};
use crate::player::Player;
use crate::server::Server;
use crate::server::debug_subscribers::TickTimeLogger;

/// Makes `player` an operator who may run `/gamerule` and `/difficulty`.
fn make_operator(server: &Server, player: &Player) {
    server.player_permission_states.write().set(
        player.gameprofile.id(),
        PermissionSubjectState::new(vec![OP_GROUP.to_owned()], PermissionSet::new()),
    );
    let granted = ["minecraft.command.gamerule", "minecraft.command.difficulty"].map(|key| {
        PermissionEntry::allow(PermissionKey::parse(key).expect("valid permission key"))
    });
    player.set_permission_state(
        vec![OP_GROUP.to_owned()],
        PermissionSet::new(),
        PermissionMetadataSet::new(),
        PermissionSet::from_entries(granted),
        PermissionMetadataSet::new(),
    );
}

fn setup() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
}

fn rule(key: &'static str, value: &str) -> GameRuleEntry {
    GameRuleEntry {
        key: Identifier::vanilla_static(key),
        value: value.to_owned(),
    }
}

#[test]
fn game_rules_screen_sets_valid_values_for_operators_only() {
    setup();
    with_server_world("set_game_rule_packet", |server, world| {
        let (alex, packets) = join_with_packets(server, world, "Alex", 10_000);
        let edit = SSetGameRule {
            entries: vec![
                rule("keep_inventory", "true"),
                rule("respawn_radius", "-1"),
                rule("respawn_radius", "3 "),
                rule("no_such_rule", "1"),
            ],
        };

        alex.handle_set_game_rule(edit.clone());
        assert!(!world.get_game_rule(&KEEP_INVENTORY), "Alex is no operator");

        make_operator(server, &alex);
        packets.lock().clear();
        alex.handle_set_game_rule(edit);

        assert!(world.get_game_rule(&KEEP_INVENTORY));
        assert_eq!(
            world.get_game_rule(&RESPAWN_RADIUS),
            10,
            "out-of-range and trailing-text values are rejected"
        );
        assert_eq!(
            chat_packets(&packets, C_SYSTEM_CHAT).len(),
            1,
            "operators hear about the one applied change"
        );
    });
}

#[test]
fn locking_the_difficulty_needs_permission() {
    setup();
    with_server_world("lock_difficulty_packet", |server, world| {
        let (alex, _packets) = join_with_packets(server, world, "Alex", 10_000);
        let locked = || world.level_data.read().data().difficulty_locked;

        alex.handle_lock_difficulty(&SLockDifficulty { locked: true });
        assert!(!locked());

        make_operator(server, &alex);
        alex.handle_lock_difficulty(&SLockDifficulty { locked: true });
        assert!(locked());
    });
}

#[test]
fn data_queries_answer_operators() {
    setup();
    with_server_world("tag_query_packets", |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let (alex, packets) = join_with_packets(server, world, "Alex", 10_000);
        assert_eq!(run(server, "summon minecraft:pig 4.5 65 4.5"), Some(1));
        let area = WorldAabb::new(4.0, 64.0, 4.0, 5.0, 67.0, 5.0);
        let pig = world
            .get_entities_in_aabb_matching(&area, |entity| {
                entity.entity_type() == &vanilla_entities::PIG
            })
            .remove(0);
        let entity_query = SEntityTagQuery {
            transaction_id: 7,
            entity_id: pig.id(),
        };
        let block_query = SBlockEntityTagQuery {
            transaction_id: 8,
            pos: BlockPos::new(0, 70, 0),
        };

        alex.handle_entity_tag_query(&entity_query);
        alex.handle_block_entity_tag_query(&block_query);
        assert!(chat_packets(&packets, C_TAG_QUERY).is_empty());

        make_operator(server, &alex);
        alex.handle_entity_tag_query(&entity_query);
        alex.handle_block_entity_tag_query(&block_query);
        assert_eq!(
            chat_packets(&packets, C_TAG_QUERY).len(),
            2,
            "both queries are answered, the plain block with an empty tag"
        );
    });
}

#[test]
fn tick_time_chart_streams_to_subscribed_operators() {
    setup();
    with_server_world("debug_sample_packets", |server, world| {
        let (alex, packets) = join_with_packets(server, world, "Alex", 10_000);
        alex.request_debug_subscriptions(vec![DEDICATED_SERVER_TICK_TIME]);
        server.tick_debug_subscribers();
        assert!(
            !server.has_debug_subscriber(DEDICATED_SERVER_TICK_TIME),
            "Alex is no operator"
        );

        make_operator(server, &alex);
        server.tick_debug_subscribers();
        assert!(server.has_debug_subscriber(DEDICATED_SERVER_TICK_TIME));

        let mut tick_time = TickTimeLogger::new();
        tick_time.tick_ended(server);
        tick_time.slept(Duration::from_millis(5));
        packets.lock().clear();
        tick_time.packets_drained(server);

        let samples = chat_packets(&packets, C_DEBUG_SAMPLE);
        assert_eq!(samples.len(), 1);
        let (_, mut body) = packet_body(&samples[0]);
        let length = VarInt::read(&mut body).expect("sample length");
        assert_eq!(length.0, 4, "vanilla TpsDebugDimensions has four values");
        let values: Vec<i64> = (0..4)
            .map(|_| i64::read(&mut body).expect("sample value"))
            .collect();
        assert_eq!(values[3], 5_000_000, "idle time in nanoseconds");
        assert_eq!(VarInt::read(&mut body).expect("sample type").0, 0);

        alex.request_debug_subscriptions(Vec::new());
        server.tick_debug_subscribers();
        assert!(!server.has_debug_subscriber(DEDICATED_SERVER_TICK_TIME));
    });
}

#[test]
fn low_disk_space_warning_reaches_admins_only() {
    setup();
    with_server_world("low_disk_space_warning", |server, world| {
        let (alex, alex_packets) = join_with_packets(server, world, "Alex", 10_000);
        let (_steve, steve_packets) = join_with_packets(server, world, "Steve", 10_001);
        make_operator(server, &alex);

        server.send_low_disk_space_warning();

        assert_eq!(
            chat_packets(&alex_packets, C_LOW_DISK_SPACE_WARNING).len(),
            1
        );
        assert!(chat_packets(&steve_packets, C_LOW_DISK_SPACE_WARNING).is_empty());
    });
}
