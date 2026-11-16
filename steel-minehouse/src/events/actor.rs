//! One named thing on stage (the Fabric `Actor`): a display entity that exists only in the
//! packets sent to the viewers. Nothing enters the world, so nothing is left in it when the
//! server dies mid-show, and whoever is not a viewer doesn't see it.

use std::sync::Arc;

use glam::{DVec3, Quat, Vec3};
use rustc_hash::FxHashSet;
use steel_core::{entity::next_entity_id, player::Player};
use steel_protocol::packets::game::{
    CAddEntity, CEntityPositionSync, CRemoveEntities, CSetEntityData, PositionPath,
};
use steel_registry::{
    RegistryEntry as _,
    data_components::vanilla_components::ITEM_MODEL,
    entity_data::{DataValue, Quaternionf, Vector3f},
    entity_type::EntityTypeRef,
    item_stack::ItemStack,
    vanilla_entities,
    vanilla_entity_data::{
        BlockDisplayEntityData, DisplayEntityData, ItemDisplayEntityData, TextDisplayEntityData,
    },
};
use uuid::Uuid;

use super::script::{ActorDef, ActorKind};
use crate::moderation::format::render;

/// Vanilla `Brightness(15, 15).pack()`.
const FULL_BRIGHT: i32 = 15 << 4 | 15 << 20;
/// Vanilla `Entity.FLAG_GLOWING` of the shared flags.
const FLAG_GLOWING: i8 = 1 << 6;
/// Vanilla `Display.BillboardConstraints.CENTER`.
const BILLBOARD_CENTER: i8 = 3;
/// Over how many ticks the client glides an actor to a new place.
const GLIDE_TICKS: i32 = 2;

enum Shown {
    Text(Box<TextDisplayEntityData>),
    Block(Box<BlockDisplayEntityData>),
    Item(Box<ItemDisplayEntityData>),
}

/// Where an actor is and where it looks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub pos: DVec3,
    pub yaw: f32,
    pub pitch: f32,
}

/// An actor on stage.
pub struct Actor {
    id: i32,
    shown: Shown,
    camera: bool,
    /// Where the model sits from the entity, before the scale and the turn.
    offset: Vec3,
    pose: Pose,
    /// Where the viewers see it this tick instead of `pose` (a shaking camera).
    nudged: Option<Pose>,
    /// What the viewers were last told.
    sent: Pose,
    rotation: Quat,
    scale: Vec3,
    /// Who has the entity.
    viewers: FxHashSet<Uuid>,
}

impl Actor {
    /// Makes the actor at `pos`; viewers get it with the next [`Actor::sync`].
    #[must_use]
    pub fn spawn(def: &ActorDef, pos: DVec3, yaw: f32, scale: Option<Vec3>) -> Self {
        let (mut shown, offset) = match &def.kind {
            ActorKind::Camera => (
                Shown::Item(Box::new(ItemDisplayEntityData::new())),
                Vec3::ZERO,
            ),
            ActorKind::Block(state) => {
                let mut data = Box::new(BlockDisplayEntityData::new());
                data.block_state.set(*state);
                (Shown::Block(data), Vec3::splat(-0.5))
            }
            ActorKind::Item { item, model } => {
                let mut stack = ItemStack::new(item);
                if let Some(model) = model {
                    stack.set(ITEM_MODEL, model.clone());
                }
                let mut data = Box::new(ItemDisplayEntityData::new());
                data.item_stack.set(stack);
                (Shown::Item(data), Vec3::ZERO)
            }
            ActorKind::Text {
                text,
                line_width,
                background,
            } => {
                let mut data = Box::new(TextDisplayEntityData::new());
                data.text.set(Box::new(render(text, &[])));
                data.line_width.set(*line_width);
                data.background_color.set(*background);
                data.display
                    .billboard_render_constraints
                    .set(BILLBOARD_CENTER);
                (Shown::Text(data), Vec3::ZERO)
            }
        };
        let display = display_of(&mut shown);
        display.pos_rot_interpolation_duration.set(GLIDE_TICKS);
        display.view_range.set(def.view_range);
        if def.bright {
            display.brightness_override.set(FULL_BRIGHT);
        }
        if let Some(glow) = def.glow {
            display.base.shared_flags.set(FLAG_GLOWING);
            display.glow_color_override.set(glow);
        }
        if let Some(billboard) = def.billboard {
            display.billboard_render_constraints.set(billboard);
        }
        let pose = Pose {
            pos,
            yaw,
            pitch: 0.0,
        };
        let mut actor = Self {
            id: next_entity_id(),
            shown,
            camera: def.is_camera(),
            offset,
            pose,
            nudged: None,
            sent: pose,
            rotation: Quat::IDENTITY,
            scale: scale.unwrap_or(def.scale),
            viewers: FxHashSet::default(),
        };
        actor.apply_transform(0);
        actor
    }

    /// The id of the entity: what a viewer's camera is bound to.
    #[must_use]
    pub const fn id(&self) -> i32 {
        self.id
    }

    #[must_use]
    pub const fn pos(&self) -> DVec3 {
        self.pose.pos
    }

    #[must_use]
    pub const fn yaw(&self) -> f32 {
        self.pose.yaw
    }

    #[must_use]
    pub const fn pitch(&self) -> f32 {
        self.pose.pitch
    }

    #[must_use]
    pub const fn rotation(&self) -> Quat {
        self.rotation
    }

    #[must_use]
    pub const fn scale(&self) -> Vec3 {
        self.scale
    }

    /// Moves the actor; for the camera, also where it looks.
    #[expect(
        clippy::float_cmp,
        reason = "the same yaw written again must not send the shape again"
    )]
    pub fn move_to(&mut self, pos: DVec3, yaw: f32, pitch: f32) {
        let turned = !self.camera && yaw != self.pose.yaw;
        self.pose = Pose { pos, yaw, pitch };
        if turned {
            self.apply_transform(GLIDE_TICKS);
        }
    }

    /// Shows the actor somewhere else for this tick only; `None` puts it back.
    pub const fn nudge(&mut self, pose: Option<Pose>) {
        self.nudged = pose;
    }

    /// Turns and scales the actor; the client blends to the new shape over `ticks`.
    pub fn set_shape(&mut self, rotation: Quat, scale: Vec3, ticks: i32) {
        self.rotation = rotation;
        self.scale = scale;
        self.apply_transform(ticks);
    }

    fn apply_transform(&mut self, ticks: i32) {
        if self.camera {
            return;
        }
        // The entity itself never turns: the yaw is part of the shape, so a model turns
        // around its middle and not around its corner.
        let full = Quat::from_rotation_y((-self.pose.yaw).to_radians()) * self.rotation;
        let translation = full * (self.offset * self.scale);
        let scale = self.scale;
        let display = display_of(&mut self.shown);
        display.translation.set(Vector3f {
            x: translation.x,
            y: translation.y,
            z: translation.z,
        });
        display.left_rotation.set(Quaternionf {
            x: full.x,
            y: full.y,
            z: full.z,
            w: full.w,
        });
        display.scale.set(Vector3f {
            x: scale.x,
            y: scale.y,
            z: scale.z,
        });
        display.transformation_interpolation_duration.set(ticks);
        display
            .transformation_interpolation_start_delta_ticks
            .set_forced(0);
    }

    const fn kind(&self) -> EntityTypeRef {
        match self.shown {
            Shown::Text(_) => &vanilla_entities::TEXT_DISPLAY,
            Shown::Block(_) => &vanilla_entities::BLOCK_DISPLAY,
            Shown::Item(_) => &vanilla_entities::ITEM_DISPLAY,
        }
    }

    fn pack_all(&self) -> Vec<DataValue> {
        match &self.shown {
            Shown::Text(data) => data.pack_all(),
            Shown::Block(data) => data.pack_all(),
            Shown::Item(data) => data.pack_all(),
        }
    }

    fn pack_dirty(&mut self) -> Option<Vec<DataValue>> {
        match &mut self.shown {
            Shown::Text(data) => data.pack_dirty(),
            Shown::Block(data) => data.pack_dirty(),
            Shown::Item(data) => data.pack_dirty(),
        }
    }

    /// What the entity itself is turned to: only a camera turns, see `apply_transform`.
    const fn facing(&self, pose: Pose) -> (f32, f32) {
        if self.camera {
            (pose.yaw, pose.pitch)
        } else {
            (0.0, 0.0)
        }
    }

    /// Brings the viewers up to date: `here` get the actor or what changed of it, whoever
    /// had it and is not `here` any more loses it (`online` is everyone who can be told).
    pub fn sync(&mut self, here: &[&Arc<Player>], online: &[Arc<Player>]) {
        let target = self.nudged.unwrap_or(self.pose);
        let (yaw, pitch) = self.facing(target);
        let changed = self.pack_dirty();
        let moved = target != self.sent;
        self.sent = target;

        let gone: Vec<Uuid> = self
            .viewers
            .iter()
            .filter(|id| !here.iter().any(|player| player.gameprofile.id() == **id))
            .copied()
            .collect();
        for id in gone {
            self.viewers.remove(&id);
            if let Some(player) = online.iter().find(|player| player.gameprofile.id() == id) {
                player.send_packet(CRemoveEntities::single(self.id));
            }
        }

        for player in here {
            if self.viewers.insert(player.gameprofile.id()) {
                player.send_packet(CAddEntity {
                    id: self.id,
                    uuid: Uuid::new_v4(),
                    entity_type: self.kind().id() as i32,
                    position: target.pos,
                    velocity: DVec3::ZERO,
                    x_rot: angle_byte(pitch),
                    y_rot: angle_byte(yaw),
                    head_y_rot: angle_byte(yaw),
                    data: 0,
                });
                player.send_packet(CSetEntityData::new(self.id, self.pack_all()));
                continue;
            }
            if let Some(values) = &changed {
                player.send_packet(CSetEntityData::new(self.id, values.clone()));
            }
            if moved {
                player.send_packet(CEntityPositionSync {
                    entity_id: self.id,
                    path: PositionPath::Linear(target.pos),
                    y_rot: yaw,
                    x_rot: pitch,
                    on_ground: false,
                });
            }
        }
    }

    /// The viewer's game no longer has the entity (they joined again).
    pub fn forget(&mut self, id: Uuid) {
        self.viewers.remove(&id);
    }

    /// Takes the actor away from everyone who has it.
    pub fn remove(&mut self, online: &[Arc<Player>]) {
        for player in online {
            if self.viewers.contains(&player.gameprofile.id()) {
                player.send_packet(CRemoveEntities::single(self.id));
            }
        }
        self.viewers.clear();
    }
}

const fn display_of(shown: &mut Shown) -> &mut DisplayEntityData {
    match shown {
        Shown::Text(data) => &mut data.display,
        Shown::Block(data) => &mut data.display,
        Shown::Item(data) => &mut data.display,
    }
}

/// Degrees as the byte of an entity spawn packet: 256 steps to a full turn.
#[expect(
    clippy::cast_possible_truncation,
    reason = "wrapped to one turn of 256 steps first"
)]
fn angle_byte(degrees: f32) -> i8 {
    ((degrees * 256.0 / 360.0).floor() as i32).rem_euclid(256) as u8 as i8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles_wrap_into_a_byte() {
        assert_eq!(angle_byte(0.0), 0);
        assert_eq!(angle_byte(90.0), 64);
        assert_eq!(angle_byte(180.0), -128);
        assert_eq!(angle_byte(-90.0), -64);
        assert_eq!(angle_byte(450.0), 64);
    }
}
