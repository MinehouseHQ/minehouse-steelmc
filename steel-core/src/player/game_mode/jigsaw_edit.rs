//! Vanilla `ServerGamePacketListenerImpl.handleSetJigsawBlock` and
//! `handleJigsawGenerate`.

use steel_protocol::packets::game::{SJigsawGenerate, SSetJigsawBlock};
use steel_utils::Downcast as _;

use crate::block_entity::entities::{JigsawBlockEntity, JigsawSettings};
use crate::player::Player;

impl Player {
    /// Vanilla `handleSetJigsawBlock`: saves what a game master entered on a
    /// jigsaw block's screen.
    pub fn handle_set_jigsaw_block(&self, packet: SSetJigsawBlock) {
        if !self.can_use_game_master_blocks() {
            return;
        }
        let world = self.get_world();
        let Some(block_entity) = world.get_block_entity(packet.pos) else {
            return;
        };
        let Some(jigsaw) = block_entity.downcast_ref::<JigsawBlockEntity>() else {
            return;
        };
        jigsaw.set_settings(JigsawSettings {
            name: packet.name,
            target: packet.target,
            pool: packet.pool,
            final_state: packet.final_state,
            joint: packet.joint,
            placement_priority: packet.placement_priority,
            selection_priority: packet.selection_priority,
        });
        world.send_block_updated(packet.pos);
    }

    /// Vanilla `handleJigsawGenerate`: grows the jigsaw's structure.
    pub fn handle_jigsaw_generate(&self, packet: &SJigsawGenerate) {
        if !self.can_use_game_master_blocks() {
            return;
        }
        let world = self.get_world();
        let Some(block_entity) = world.get_block_entity(packet.pos) else {
            return;
        };
        if let Some(jigsaw) = block_entity.downcast_ref::<JigsawBlockEntity>() {
            jigsaw.generate(&world, packet.levels, packet.keep_jigsaws);
        }
    }
}
