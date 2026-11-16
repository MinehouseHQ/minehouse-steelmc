//! A block or item display that is part of an effect (a meteor, a segment of a rift, a piece
//! of a rocket, a scattered item) and not an actor of the script. Like an actor it exists only in the packets
//! sent to the viewers.

use std::sync::Arc;

use glam::{DVec3, Quat, Vec3};
use rustc_hash::FxHashSet;
use steel_core::{entity::next_entity_id, player::Player};
use steel_protocol::packets::game::{
    CAddEntity, CEntityPositionSync, CRemoveEntities, CSetEntityData, PositionPath,
};
use steel_registry::{
    RegistryEntry as _,
    entity_data::{DataValue, Quaternionf, Vector3f},
    entity_type::EntityTypeRef,
    item_stack::ItemStack,
    vanilla_entities,
    vanilla_entity_data::{BlockDisplayEntityData, DisplayEntityData, ItemDisplayEntityData},
};
use steel_utils::BlockStateId;
use uuid::Uuid;

/// Vanilla `Brightness(15, 15).pack()`.
const FULL_BRIGHT: i32 = 15 << 4 | 15 << 20;
/// Vanilla `Entity.FLAG_GLOWING` of the shared flags.
const FLAG_GLOWING: i8 = 1 << 6;

enum Data {
    Block(Box<BlockDisplayEntityData>),
    Item(Box<ItemDisplayEntityData>),
}

impl Data {
    const fn display(&mut self) -> &mut DisplayEntityData {
        match self {
            Self::Block(data) => &mut data.display,
            Self::Item(data) => &mut data.display,
        }
    }

    const fn kind(&self) -> EntityTypeRef {
        match self {
            Self::Block(_) => &vanilla_entities::BLOCK_DISPLAY,
            Self::Item(_) => &vanilla_entities::ITEM_DISPLAY,
        }
    }

    fn pack_all(&self) -> Vec<DataValue> {
        match self {
            Self::Block(data) => data.pack_all(),
            Self::Item(data) => data.pack_all(),
        }
    }

    fn pack_dirty(&mut self) -> Option<Vec<DataValue>> {
        match self {
            Self::Block(data) => data.pack_dirty(),
            Self::Item(data) => data.pack_dirty(),
        }
    }
}

/// A display entity of an effect.
pub struct Prop {
    id: i32,
    data: Data,
    pos: DVec3,
    /// Where the viewers were last told it is.
    sent: DVec3,
    /// Who has the entity.
    viewers: FxHashSet<Uuid>,
}

impl Prop {
    /// Makes a fully lit block display at `pos`; viewers get it with the next
    /// [`Prop::sync`].
    #[must_use]
    pub fn new(state: BlockStateId, pos: DVec3, view_range: f32) -> Self {
        let mut data = Box::new(BlockDisplayEntityData::new());
        data.block_state.set(state);
        data.display.brightness_override.set(FULL_BRIGHT);
        Self::with(Data::Block(data), pos, view_range)
    }

    /// Makes an item display at `pos`, lit like the place it is in.
    #[must_use]
    pub fn item(stack: ItemStack, pos: DVec3, view_range: f32) -> Self {
        let mut data = Box::new(ItemDisplayEntityData::new());
        data.item_stack.set(stack);
        Self::with(Data::Item(data), pos, view_range)
    }

    fn with(mut data: Data, pos: DVec3, view_range: f32) -> Self {
        data.display().view_range.set(view_range);
        Self {
            id: next_entity_id(),
            data,
            pos,
            sent: pos,
            viewers: FxHashSet::default(),
        }
    }

    /// Where the display is.
    #[must_use]
    pub const fn pos(&self) -> DVec3 {
        self.pos
    }

    /// An outline of this RGB color, seen through walls.
    pub fn glow(&mut self, color: i32) {
        let display = self.data.display();
        display.base.shared_flags.set(FLAG_GLOWING);
        display.glow_color_override.set(color);
    }

    /// The box around the entity the client culls it by: a long shape needs a big one.
    pub fn cull_box(&mut self, width: f32, height: f32) {
        let display = self.data.display();
        display.width.set(width);
        display.height.set(height);
    }

    /// The shape of the display; the client blends to it over `ticks`.
    pub fn shape(&mut self, translation: Vec3, rotation: Quat, scale: Vec3, ticks: u32) {
        let display = self.data.display();
        display.translation.set(Vector3f {
            x: translation.x,
            y: translation.y,
            z: translation.z,
        });
        display.left_rotation.set(Quaternionf {
            x: rotation.x,
            y: rotation.y,
            z: rotation.z,
            w: rotation.w,
        });
        display.scale.set(Vector3f {
            x: scale.x,
            y: scale.y,
            z: scale.z,
        });
        display
            .transformation_interpolation_duration
            .set(i32::try_from(ticks).unwrap_or(i32::MAX));
        display
            .transformation_interpolation_start_delta_ticks
            .set_forced(0);
    }

    /// Moves the display; the client glides it there over `ticks`.
    pub fn glide(&mut self, pos: DVec3, ticks: u32) {
        self.data
            .display()
            .pos_rot_interpolation_duration
            .set(i32::try_from(ticks).unwrap_or(i32::MAX));
        self.pos = pos;
    }

    /// Brings `here` up to date: who doesn't have the display gets it, the others get what
    /// changed. Whoever walked away keeps it until it is removed.
    pub fn sync(&mut self, here: &[&Arc<Player>]) {
        let changed = self.data.pack_dirty();
        let moved = self.pos != self.sent;
        self.sent = self.pos;
        for player in here {
            if self.viewers.insert(player.gameprofile.id()) {
                player.send_packet(CAddEntity {
                    id: self.id,
                    uuid: Uuid::new_v4(),
                    entity_type: self.data.kind().id() as i32,
                    position: self.pos,
                    velocity: DVec3::ZERO,
                    x_rot: 0,
                    y_rot: 0,
                    head_y_rot: 0,
                    data: 0,
                });
                player.send_packet(CSetEntityData::new(self.id, self.data.pack_all()));
                continue;
            }
            // The shape first: it carries over how long the move is to take.
            if let Some(values) = &changed {
                player.send_packet(CSetEntityData::new(self.id, values.clone()));
            }
            if moved {
                player.send_packet(CEntityPositionSync {
                    entity_id: self.id,
                    path: PositionPath::Linear(self.pos),
                    y_rot: 0.0,
                    x_rot: 0.0,
                    on_ground: false,
                });
            }
        }
    }

    /// The viewer's game no longer has the entity (they joined again).
    pub fn forget(&mut self, id: Uuid) {
        self.viewers.remove(&id);
    }

    /// Takes the display away from everyone who has it.
    pub fn remove(&mut self, online: &[Arc<Player>]) {
        for player in online {
            if self.viewers.contains(&player.gameprofile.id()) {
                player.send_packet(CRemoveEntities::single(self.id));
            }
        }
        self.viewers.clear();
    }
}
