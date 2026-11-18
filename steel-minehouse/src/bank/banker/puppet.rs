//! The banker's body (the Fabric `Puppeteer` of `MineHouse` Events): an item display per bone of
//! his [`Rig`], playing `idle`, `look_up` when someone comes to talk and `talk` while his words
//! type out. His mannequin stays for the clicks but is invisible, so his name is a text display
//! over the rig (an invisible entity's own name tag isn't drawn).
//!
//! Like the mannequin, the displays exist only in the packets sent to those near.

use std::{f32::consts::PI, iter};

use glam::{DVec3, Mat4, Quat, Vec3};
use steel_core::{entity::next_entity_id, player::Player};
use steel_protocol::packets::game::{CAddEntity, CSetEntityData};
use steel_registry::{
    RegistryEntry as _,
    entity_data::{DataValue, Quaternionf, Vector3f},
    entity_type::EntityTypeRef,
    item_stack::ItemStack,
    vanilla_entities,
    vanilla_entity_data::{DisplayEntityData, ItemDisplayEntityData, TextDisplayEntityData},
    vanilla_items,
};
use text_components::TextComponent;
use uuid::Uuid;

use super::rig::{self, Pose, Rig};
use crate::bank::gui;

/// Ticks between frames; the client glides the bones in between.
pub const STEP: u64 = 2;
/// Over how many frames one animation blends into the next.
const BLEND: u64 = 5;
/// How high the name hangs over his feet, in blocks.
const LABEL_HEIGHT: f32 = 2.15;
/// Vanilla `Display.BillboardConstraints.CENTER`.
const BILLBOARD_CENTER: i8 = 3;
/// A see-through black behind the name.
const LABEL_BACKGROUND: i32 = 0x4000_0000;
/// How long he looks up at whoever came before he talks (`look_up` in `banker.bbmodel`).
const LOOK_UP_TICKS: u32 = 32;

/// What a banker does now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Doing {
    Idle,
    LookUp,
    Talk,
}

impl Doing {
    /// What the banker does while `age` ticks into a talk whose page `typed` out already.
    pub const fn in_talk(age: u32, typed: bool) -> Self {
        if age < LOOK_UP_TICKS {
            Self::LookUp
        } else if typed {
            Self::Idle
        } else {
            Self::Talk
        }
    }

    /// Of two talks to the same banker the one that began last is looked up at first.
    pub fn busier(self, other: Self) -> Self {
        let rank = |doing| match doing {
            Self::LookUp => 2,
            Self::Talk => 1,
            Self::Idle => 0,
        };
        if rank(other) > rank(self) {
            other
        } else {
            self
        }
    }

    const fn anim(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::LookUp => "look_up",
            Self::Talk => "talk",
        }
    }
}

/// The displays of one banker.
pub struct Puppet {
    rig: &'static Rig,
    /// The id of each bone's display and the bone.
    parts: Vec<(i32, usize)>,
    label: i32,
    doing: Doing,
    start: u64,
    pose: Pose,
    blend_from: Option<Pose>,
}

/// A bone display's transformation: `matrix` from the banker's feet.
fn shaped(data: &mut DisplayEntityData, matrix: &Mat4, ticks: i32) {
    // The rig only turns and moves its bones, which is what the client puts together again.
    let (scale, rotation, translation): (Vec3, Quat, Vec3) = matrix.to_scale_rotation_translation();
    data.translation.set(Vector3f {
        x: translation.x,
        y: translation.y,
        z: translation.z,
    });
    data.left_rotation.set(Quaternionf {
        x: rotation.x,
        y: rotation.y,
        z: rotation.z,
        w: rotation.w,
    });
    data.scale.set(Vector3f {
        x: scale.x,
        y: scale.y,
        z: scale.z,
    });
    data.transformation_interpolation_duration.set(ticks);
    data.transformation_interpolation_start_delta_ticks
        .set_forced(0);
}

impl Puppet {
    /// A banker's body, unless his rig can't be read.
    pub fn new(tick: u64) -> Option<Self> {
        let rig = Rig::banker()?;
        Some(Self {
            rig,
            parts: rig
                .models()
                .map(|(bone, _)| (next_entity_id(), bone))
                .collect(),
            label: next_entity_id(),
            doing: Doing::Idle,
            start: tick,
            pose: rig.rest_pose(),
            blend_from: None,
        })
    }

    /// The ids of every display.
    pub fn ids(&self) -> impl Iterator<Item = i32> + '_ {
        self.parts
            .iter()
            .map(|(id, _)| *id)
            .chain(iter::once(self.label))
    }

    /// Each bone display's frame for a banker turned to `yaw`.
    fn frames(&self, yaw: f32) -> Vec<(i32, Mat4)> {
        let frames = self.rig.matrices(&self.pose);
        // The model looks at −Z, an item display draws its model turned half round.
        let root = Mat4::from_rotation_y((-yaw).to_radians() + PI);
        let flip = Mat4::from_rotation_y(PI);
        self.parts
            .iter()
            .map(|(id, bone)| (*id, root * frames[*bone] * flip))
            .collect()
    }

    /// Shows the body to `player`: the banker stands at `position`, turned to `yaw`.
    pub fn spawn(&self, player: &Player, position: DVec3, yaw: f32, name: &str) {
        let models: Vec<&'static str> = self.rig.models().map(|(_, model)| model).collect();
        for ((id, matrix), model) in self.frames(yaw).into_iter().zip(models) {
            let mut data = ItemDisplayEntityData::new();
            let mut stack = ItemStack::new(&vanilla_items::PAPER);
            gui::model(&mut stack, model);
            data.item_stack.set(stack);
            data.display.view_range.set(1.5);
            shaped(&mut data.display, &matrix, 0);
            add(player, id, &vanilla_entities::ITEM_DISPLAY, position);
            player.send_packet(CSetEntityData::new(id, data.pack_all()));
        }
        let mut label = TextDisplayEntityData::new();
        label
            .text
            .set(Box::new(TextComponent::plain(name.to_owned())));
        label.background_color.set(LABEL_BACKGROUND);
        label
            .display
            .billboard_render_constraints
            .set(BILLBOARD_CENTER);
        label.display.view_range.set(0.5);
        label.display.translation.set(Vector3f {
            x: 0.0,
            y: LABEL_HEIGHT,
            z: 0.0,
        });
        add(
            player,
            self.label,
            &vanilla_entities::TEXT_DISPLAY,
            position,
        );
        player.send_packet(CSetEntityData::new(self.label, label.pack_all()));
    }

    /// A tick: what the banker does now, and on every [`STEP`]th tick the bones' new shapes
    /// for the viewers (empty in between).
    pub fn tick(&mut self, doing: Doing, tick: u64, yaw: f32) -> Vec<(i32, Vec<DataValue>)> {
        if doing != self.doing {
            self.doing = doing;
            self.start = tick;
            self.blend_from = Some(self.pose.clone());
        }
        if !tick.is_multiple_of(STEP) {
            return Vec::new();
        }
        let elapsed = tick - self.start;
        #[expect(
            clippy::cast_precision_loss,
            reason = "the ticks of one animation are few"
        )]
        let seconds = elapsed as f32 / 20.0;
        let mut target = self.rig.anim(self.doing.anim()).map_or_else(
            || self.rig.rest_pose(),
            |anim| self.rig.sample(anim, seconds),
        );
        if let Some(from) = &self.blend_from {
            if elapsed < BLEND * STEP {
                #[expect(clippy::cast_precision_loss, reason = "a few ticks")]
                let f = (((elapsed + STEP) as f32) / ((BLEND * STEP) as f32)).min(1.0);
                target = rig::mix(from, &target, f * f * (3.0 - 2.0 * f));
            } else {
                self.blend_from = None;
            }
        }
        self.pose = target;
        #[expect(clippy::cast_possible_truncation, reason = "STEP is 2")]
        let ticks = STEP as i32;
        self.frames(yaw)
            .into_iter()
            .filter_map(|(id, matrix)| {
                let mut data = ItemDisplayEntityData::new();
                shaped(&mut data.display, &matrix, ticks);
                data.pack_dirty().map(|values| (id, values))
            })
            .collect()
    }
}

fn add(player: &Player, id: i32, kind: EntityTypeRef, position: DVec3) {
    player.send_packet(CAddEntity {
        id,
        uuid: Uuid::new_v4(),
        entity_type: kind.id() as i32,
        position,
        velocity: DVec3::ZERO,
        x_rot: 0,
        y_rot: 0,
        head_y_rot: 0,
        data: 0,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn he_looks_up_then_talks_then_rests() {
        assert_eq!(Doing::in_talk(0, false), Doing::LookUp);
        assert_eq!(Doing::in_talk(LOOK_UP_TICKS, false), Doing::Talk);
        assert_eq!(Doing::in_talk(LOOK_UP_TICKS, true), Doing::Idle);
        assert_eq!(Doing::Talk.busier(Doing::LookUp), Doing::LookUp);
        assert_eq!(Doing::Talk.busier(Doing::Idle), Doing::Talk);
    }
}
