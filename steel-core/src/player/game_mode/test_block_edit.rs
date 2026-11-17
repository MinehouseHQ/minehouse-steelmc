//! Vanilla `ServerGamePacketListenerImpl.handleSetTestBlock`.

use steel_protocol::packets::game::SSetTestBlock;
use steel_utils::Downcast as _;

use crate::block_entity::BlockEntity as _;
use crate::block_entity::entities::TestBlockEntity;
use crate::player::Player;

impl Player {
    /// Vanilla `handleSetTestBlock`: saves the mode and message a game master
    /// chose on a test block's screen.
    pub fn handle_set_test_block(&self, packet: SSetTestBlock) {
        if !self.can_use_game_master_blocks() {
            return;
        }
        let world = self.get_world();
        let Some(block_entity) = world.get_block_entity(packet.pos) else {
            return;
        };
        let Some(test) = block_entity.downcast_ref::<TestBlockEntity>() else {
            return;
        };
        test.set_mode(&world, packet.mode);
        test.set_message(packet.message);
        test.set_changed();
        world.send_block_updated(packet.pos);
    }
}
