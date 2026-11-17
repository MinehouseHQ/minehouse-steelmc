//! Pick block on an entity and the spectator menu's teleport, from vanilla
//! `ServerGamePacketListenerImpl`.

use std::sync::Arc;

use steel_protocol::packets::game::{SPickItemFromEntity, STeleportToEntity};

use crate::command::execution::CommandSource;
use crate::command::print_for_avatar;
use crate::command::sender::CommandSender;
use crate::entity::Entity;
use crate::player::Player;
use crate::server::Server;

use super::ENTITY_INTERACTION_RANGE_BUFFER;

impl Player {
    /// Vanilla `handlePickItemFromEntity`: picks the entity's pick result
    /// and, for a game master asking for data on an avatar, reports its
    /// profile like `/fetchprofile entity`.
    pub fn handle_pick_item_from_entity(
        self: &Arc<Self>,
        packet: &SPickItemFromEntity,
        server: &Arc<Server>,
    ) {
        let world = self.get_world();
        let Some(entity) = world.get_accessible_entity_by_id(packet.id) else {
            return;
        };
        if !self.is_within_entity_interaction_range_with_buffer(
            entity.bounding_box(),
            ENTITY_INTERACTION_RANGE_BUFFER,
        ) {
            return;
        }

        if let Some(item_stack) = entity.pick_result()
            && !item_stack.is_empty()
        {
            self.try_pick_item(item_stack);
        }

        if packet.include_data && self.can_use_game_master_blocks() {
            let source =
                CommandSource::new(CommandSender::Player(Arc::clone(self)), Arc::clone(server));
            print_for_avatar(&source, entity.as_ref());
        }
    }

    /// Vanilla `handleTeleportToEntityPacket`: a spectator jumps to the
    /// entity with the packet's UUID in any world of their domain, taking its
    /// rotation.
    pub fn handle_teleport_to_entity(
        self: &Arc<Self>,
        packet: &STeleportToEntity,
        server: &Server,
    ) {
        if !self.is_spectator() {
            return;
        }
        let domain = self.get_world().domain().to_owned();
        for world in server.worlds.worlds_in_domain(&domain) {
            if let Some(entity) = world.get_entity_by_uuid(&packet.uuid) {
                self.teleport_to_world(&world, entity.position(), entity.rotation());
                return;
            }
        }
    }
}
