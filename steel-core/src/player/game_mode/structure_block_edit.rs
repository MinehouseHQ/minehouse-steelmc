//! Vanilla `ServerGamePacketListenerImpl.handleSetStructureBlock`.

use glam::IVec3;
use steel_protocol::packets::game::{
    SSetStructureBlock, StructureBlockMirror, StructureBlockUpdateType,
};
use steel_utils::{Downcast as _, translations};
use steel_worldgen::structure::StructureMirror;
use text_components::TextComponent;

use crate::block_entity::BlockEntity as _;
use crate::block_entity::entities::{StructureBlockEntity, parse_structure_name};
use crate::player::Player;

const fn mirror(mirror: StructureBlockMirror) -> StructureMirror {
    match mirror {
        StructureBlockMirror::None => StructureMirror::None,
        StructureBlockMirror::LeftRight => StructureMirror::LeftRight,
        StructureBlockMirror::FrontBack => StructureMirror::FrontBack,
    }
}

impl Player {
    /// Vanilla `handleSetStructureBlock`: saves what a game master entered on
    /// a structure block's screen, then saves, loads or measures the
    /// structure for the button pressed.
    pub fn handle_set_structure_block(&self, packet: SSetStructureBlock) {
        if !self.can_use_game_master_blocks() {
            return;
        }
        let world = self.get_world();
        let Some(block_entity) = world.get_block_entity(packet.pos) else {
            return;
        };
        let Some(structure) = block_entity.downcast_ref::<StructureBlockEntity>() else {
            return;
        };
        structure.set_mode(&world, packet.mode);
        structure.update_settings(|settings| {
            settings.name = parse_structure_name(&packet.name);
            settings.pos = IVec3::from_array(packet.offset);
            settings.size = IVec3::from_array(packet.size);
            settings.mirror = mirror(packet.mirror);
            settings.rotation = packet.rotation;
            settings.metadata = packet.data;
            settings.ignore_entities = packet.ignore_entities;
            settings.strict = packet.strict;
            settings.show_air = packet.show_air;
            settings.show_bounding_box = packet.show_bounding_box;
            settings.integrity = packet.integrity;
            settings.seed = packet.seed;
        });
        let name = structure.settings().name.map(|name| name.to_string());
        let message = match name {
            Some(name) => match packet.update_type {
                StructureBlockUpdateType::SaveArea => Some(
                    if structure.save_structure_to_disk(&world) {
                        &translations::STRUCTURE_BLOCK_SAVE_SUCCESS
                    } else {
                        &translations::STRUCTURE_BLOCK_SAVE_FAILURE
                    }
                    .message([name])
                    .component(),
                ),
                StructureBlockUpdateType::LoadArea => Some(
                    if !structure.is_structure_loadable(&world) {
                        &translations::STRUCTURE_BLOCK_LOAD_NOT_FOUND
                    } else if structure.place_structure_if_same_size(&world) {
                        &translations::STRUCTURE_BLOCK_LOAD_SUCCESS
                    } else {
                        &translations::STRUCTURE_BLOCK_LOAD_PREPARE
                    }
                    .message([name])
                    .component(),
                ),
                StructureBlockUpdateType::ScanArea => Some(if structure.detect_size(&world) {
                    translations::STRUCTURE_BLOCK_SIZE_SUCCESS
                        .message([name])
                        .component()
                } else {
                    TextComponent::from(&translations::STRUCTURE_BLOCK_SIZE_FAILURE)
                }),
                StructureBlockUpdateType::UpdateData => None,
            },
            None => Some(
                translations::STRUCTURE_BLOCK_INVALID_STRUCTURE_NAME
                    .message([packet.name])
                    .component(),
            ),
        };
        if let Some(message) = message {
            self.send_message(&message);
        }
        structure.set_changed();
        world.send_block_updated(packet.pos);
    }
}
