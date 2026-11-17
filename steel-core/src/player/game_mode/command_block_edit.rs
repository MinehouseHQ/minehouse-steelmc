<<<<<<< HEAD
//! Vanilla `ServerGamePacketListenerImpl.handleSetCommandBlock` and
//! `handleSetCommandMinecart`.

use std::sync::Arc;

use steel_protocol::packets::game::{
    CommandBlockPacketMode, SSetCommandBlock, SSetCommandMinecart,
};
=======
//! Vanilla `ServerGamePacketListenerImpl.handleSetCommandBlock`.

use std::sync::Arc;

use steel_protocol::packets::game::{CommandBlockPacketMode, SSetCommandBlock};
>>>>>>> origin/master
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_game_rules::COMMAND_BLOCKS_WORK;
use steel_utils::types::UpdateFlags;
use steel_utils::{Downcast as _, translations};
use text_components::TextComponent;

use crate::block_entity::entities::{CommandBlockEntity, CommandBlockMode};
<<<<<<< HEAD
use crate::entity::entities::CommandBlockMinecartEntity;
=======
>>>>>>> origin/master
use crate::player::Player;

impl Player {
    /// Vanilla `handleSetCommandBlock`: saves what a game master entered on a
    /// command block's screen, turning it into the chosen kind of command block.
    pub fn handle_set_command_block(&self, packet: SSetCommandBlock) {
        if !self.can_use_game_master_blocks() {
            self.send_message(&TextComponent::translated(
                translations::ADV_MODE_NOT_ALLOWED.msg(),
            ));
            return;
        }
        let world = self.get_world();
        let pos = packet.pos;
        let Some(block_entity) = world.get_block_entity(pos) else {
            return;
        };
        let Some(entity) = block_entity.downcast_ref::<CommandBlockEntity>() else {
            return;
        };

        let old_mode = entity.mode();
        let current_state = world.get_block_state(pos);
        let direction = current_state.get_value(&BlockStateProperties::FACING);
        let block = match packet.mode {
            CommandBlockPacketMode::Sequence => &vanilla_blocks::CHAIN_COMMAND_BLOCK,
            CommandBlockPacketMode::Auto => &vanilla_blocks::REPEATING_COMMAND_BLOCK,
            CommandBlockPacketMode::Redstone => &vanilla_blocks::COMMAND_BLOCK,
        };
        let state = block
            .default_state()
            .set_value(&BlockStateProperties::FACING, direction)
            .set_value(&BlockStateProperties::CONDITIONAL, packet.conditional);
        if state != current_state {
            world.set_block(pos, state, UpdateFlags::UPDATE_CLIENTS);
            // The block change replaced the entity; vanilla puts the edited one back.
            world.set_block_entity(Arc::clone(&block_entity));
        }

        let command_block = entity.command_block();
        entity.set_command(packet.command.clone());
        command_block.set_track_output(packet.track_output);
        if !packet.track_output {
            command_block.set_last_output(None);
        }
        entity.set_automatic(&world, packet.automatic);
        if old_mode != packet_mode(packet.mode) {
            entity.on_mode_switch(&world);
        }

        let enabled = world.get_game_rule(&COMMAND_BLOCKS_WORK);
        if enabled {
            // Vanilla `onUpdated`.
            world.send_block_updated(pos);
        }
<<<<<<< HEAD
        self.confirm_command_set(packet.command, enabled);
    }

    /// Vanilla `handleSetCommandMinecart`: saves what a game master entered on
    /// a command block minecart's screen.
    pub fn handle_set_command_minecart(&self, packet: &SSetCommandMinecart) {
        if !self.can_use_game_master_blocks() {
            self.send_message(&TextComponent::translated(
                translations::ADV_MODE_NOT_ALLOWED.msg(),
            ));
            return;
        }
        let world = self.get_world();
        let Some(entity) = world.get_entity_by_id(packet.entity) else {
            return;
        };
        let Some(cart) = entity.downcast_ref::<CommandBlockMinecartEntity>() else {
            return;
        };
        let command_block = cart.command_block();
        command_block.set_command(packet.command.clone());
        command_block.set_track_output(packet.track_output);
        if !packet.track_output {
            command_block.set_last_output(None);
        }
        let enabled = world.get_game_rule(&COMMAND_BLOCKS_WORK);
        if enabled {
            cart.sync_command_block();
        }
        self.confirm_command_set(packet.command.clone(), enabled);
    }

    /// Tells the editor the command was saved, or that command blocks are off.
    fn confirm_command_set(&self, command: String, enabled: bool) {
        if !command.is_empty() {
=======
        if !packet.command.is_empty() {
>>>>>>> origin/master
            let message = if enabled {
                &translations::ADV_MODE_SET_COMMAND_SUCCESS
            } else {
                &translations::ADV_MODE_SET_COMMAND_DISABLED
            };
<<<<<<< HEAD
            self.send_message(&message.message([command]).component());
=======
            self.send_message(&message.message([packet.command]).component());
>>>>>>> origin/master
        }
    }
}

const fn packet_mode(mode: CommandBlockPacketMode) -> CommandBlockMode {
    match mode {
        CommandBlockPacketMode::Sequence => CommandBlockMode::Sequence,
        CommandBlockPacketMode::Auto => CommandBlockMode::Auto,
        CommandBlockPacketMode::Redstone => CommandBlockMode::Redstone,
    }
}
