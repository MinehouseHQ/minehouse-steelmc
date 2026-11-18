//! Packets vanilla sends to the players tracking an entity: the transient
//! block after a falling block lands (`FallingBlockEntity.tick`) and the
//! projectile power bundled with a fireball's motion (`ServerEntity.sendChanges`).

use std::sync::Arc;

use glam::DVec3;
use steel_registry::packets::play::{
    C_ADD_TRANSIENT_BLOCK, C_PROJECTILE_POWER, C_SET_ENTITY_MOTION,
};
use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_entities};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos, WorldAabb};

use super::chat_packets;
use super::insert_ready_full_chunk;
use super::recipe_command::join_with_packets;
use super::scoreboard_command::{run, with_server_world};
use crate::behavior::init_behaviors;
use crate::entity::entities::FallingBlockEntity;
use crate::entity::{Entity as _, EntityChangeSenders, init_entities};
use crate::world::World;

fn setup() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
}

/// Runs one tracker update, broadcasting movement like the world tick does.
fn send_tracker_changes(world: &Arc<World>) {
    world.entity_tracker().send_changes(
        |_| Vec::new(),
        |_| None,
        EntityChangeSenders {
            movement: |entity_id, packet| {
                world.broadcast_movement_sync_to_entity_trackers(entity_id, packet, None);
            },
            self_movement: |_, _| {},
            entity_data: |_, _| {},
            attributes: |_, _| {},
            mob_effects: |_, _| {},
            equipment: |_, _| {},
            passengers: |_, _| {},
            entity_link: |_, _| {},
        },
    );
}

#[test]
fn landing_falling_block_sends_a_transient_block() {
    setup();
    with_server_world("transient_block_packet", |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let (alex, packets) = join_with_packets(server, world, "Alex", 10_000);
        alex.chunk_sender()
            .lock()
            .mark_chunk_sent_for_test(ChunkPos::new(0, 0));
        let sand = vanilla_blocks::SAND.default_state();
        let pos = BlockPos::new(3, 70, 3);
        let floor = BlockPos::new(3, 67, 3);
        let stone = vanilla_blocks::STONE.default_state();
        assert!(world.set_block(floor, stone, UpdateFlags::UPDATE_ALL));
        assert!(world.set_block(pos, sand, UpdateFlags::UPDATE_ALL));
        let falling = FallingBlockEntity::fall(world, pos, sand);
        packets.lock().clear();

        for _ in 0..240 {
            if falling.is_removed() {
                break;
            }
            falling.set_old_position_to_current();
            falling.advance_tick_count();
            Arc::clone(&falling).tick();
        }

        assert!(falling.is_removed(), "the sand landed");
        assert_eq!(world.get_block_state(floor.above()), sand);
        assert_eq!(chat_packets(&packets, C_ADD_TRANSIENT_BLOCK).len(), 1);
    });
}

#[test]
fn fireball_motion_comes_with_its_acceleration_power() {
    setup();
    with_server_world("projectile_power_packet", |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let (alex, packets) = join_with_packets(server, world, "Alex", 10_000);
        alex.chunk_sender()
            .lock()
            .mark_chunk_sent_for_test(ChunkPos::new(0, 0));
        assert_eq!(
            run(
                server,
                "summon minecraft:fireball 4.5 66 4.5 {acceleration_power:0.25d,Motion:[0.0d,0.0d,0.0d]}"
            ),
            Some(1)
        );
        assert_eq!(
            run(
                server,
                "summon minecraft:arrow 8.5 66 8.5 {NoGravity:1b,Motion:[0.0d,0.0d,0.0d]}"
            ),
            Some(1)
        );
        let area = WorldAabb::new(0.0, 60.0, 0.0, 16.0, 72.0, 16.0);
        let entities = world.get_entities_in_aabb_matching(&area, |entity| {
            entity.entity_type() == &vanilla_entities::FIREBALL
                || entity.entity_type() == &vanilla_entities::ARROW
        });
        assert_eq!(entities.len(), 2);
        for entity in &entities {
            entity.set_velocity(DVec3::new(0.5, 0.0, 0.0));
        }
        packets.lock().clear();

        send_tracker_changes(world);

        assert_eq!(
            chat_packets(&packets, C_SET_ENTITY_MOTION).len(),
            2,
            "both projectiles sync their motion"
        );
        assert_eq!(
            chat_packets(&packets, C_PROJECTILE_POWER).len(),
            1,
            "only the fireball sends its power"
        );
    });
}
