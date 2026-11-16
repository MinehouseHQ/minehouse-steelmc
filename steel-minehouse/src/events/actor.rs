//! One named thing on stage (the Fabric `Actor`): display entities that exist only in the
//! packets sent to the viewers. Nothing enters the world, so nothing is left in it when the
//! server dies mid-show, and whoever is not a viewer doesn't see it.
//!
//! Every part of an actor stands at the actor's place and is shaped by its transformation:
//! moving the actor moves every part by the same amount, turning or scaling it rewrites
//! every part around a common pivot, so a model of a hundred blocks turns as one piece.
//!
//! A `player` or `mob` actor is a living entity instead (a mannequin in a player's skin, or
//! any mob): it turns itself, its game walks its legs when it is moved, and it can lie down,
//! swing its arm and hold an item.

use std::{mem, ptr, sync::Arc};

use glam::{DVec3, Quat, Vec3};
use rustc_hash::{FxHashMap, FxHashSet};
use steel_core::{entity::next_entity_id, player::Player};
use steel_protocol::packets::game::{
    CAddEntity, CEntityPositionSync, CRemoveEntities, CRotateHead, CSetEntityData, CSetEquipment,
    EquipmentSlotItem, PositionPath,
};
use steel_registry::{
    RegistryEntry as _,
    blocks::block_state_ext::BlockStateExt as _,
    data_components::vanilla_components::ITEM_MODEL,
    entity_data::{DataValue, EntityPose, Quaternionf, Vector3f},
    entity_type::EntityTypeRef,
    equipment::EquipmentSlot,
    item_stack::ItemStack,
    items::ItemRef,
    resolvable_profile::{PlayerSkinPatch, ResolvableProfile},
    vanilla_entities,
    vanilla_entity_data::{
        BaseEntityData, BlockDisplayEntityData, CreeperEntityData, DisplayEntityData,
        ItemDisplayEntityData, MannequinEntityData, TextDisplayEntityData,
    },
};
use text_components::TextComponent;
use uuid::Uuid;

use super::{
    blocks::BlockSet,
    script::{ActorDef, ActorKind},
};
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
    Body(Box<Body>),
}

/// What a living stand-in is to the viewers' game.
enum BodyData {
    Mannequin(Box<MannequinEntityData>),
    /// Any mob: only what every entity has is told, the rest is the game's own.
    Mob(Box<BaseEntityData>),
}

/// The living entity of a `player` or `mob` actor.
struct Body {
    kind: EntityTypeRef,
    data: BodyData,
    /// What it holds in its main hand.
    hand: Option<ItemStack>,
    /// A mannequin whose skin the viewers' games look up by this name, until the player
    /// themselves is online and the server has their skin to hand out.
    unresolved: Option<String>,
    /// A creeper that swells (`ignite`): told to whoever gets the entity later too.
    swelling: Option<DataValue>,
    swelling_unsent: bool,
}

impl Body {
    const fn base(&mut self) -> &mut BaseEntityData {
        match &mut self.data {
            BodyData::Mannequin(data) => &mut data.avatar.living_entity.base,
            BodyData::Mob(data) => data,
        }
    }

    fn mannequin(name: &str, show_name: bool) -> Self {
        let mut data = Box::new(MannequinEntityData::new());
        // A name that is not a player's can't be looked up: the default skin then.
        if let Ok(profile) =
            ResolvableProfile::dynamic_name(name.to_owned(), PlayerSkinPatch::default())
        {
            data.profile.set(profile);
        }
        data.immovable.set(true);
        // Vanilla writes "NPC" under the name unless the description is empty.
        data.description.set(None);
        if show_name {
            let base = &mut data.avatar.living_entity.base;
            base.custom_name
                .set(Some(Box::new(TextComponent::plain(name.to_owned()))));
            base.custom_name_visible.set(true);
        }
        Self {
            kind: &vanilla_entities::MANNEQUIN,
            data: BodyData::Mannequin(data),
            hand: None,
            unresolved: Some(name.to_owned()),
            swelling: None,
            swelling_unsent: false,
        }
    }

    fn mob(kind: EntityTypeRef) -> Self {
        Self {
            kind,
            data: BodyData::Mob(Box::new(BaseEntityData::new())),
            hand: None,
            unresolved: None,
            swelling: None,
            swelling_unsent: false,
        }
    }

    /// Hands the mannequin the skin of the player it plays once they are online.
    fn resolve(&mut self, online: &[Arc<Player>]) {
        let Some(name) = &self.unresolved else {
            return;
        };
        let Some(player) = online
            .iter()
            .find(|player| player.gameprofile.name().eq_ignore_ascii_case(name))
        else {
            return;
        };
        if let BodyData::Mannequin(data) = &mut self.data {
            data.profile
                .set(ResolvableProfile::from(player.gameprofile.clone()));
        }
        self.unresolved = None;
    }
}

/// Where an actor is and where it looks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub pos: DVec3,
    pub yaw: f32,
    pub pitch: f32,
}

/// One display entity of an actor.
struct Part {
    id: i32,
    shown: Shown,
    /// Where it sits from the actor's place, before the scale and the turn.
    offset: Vec3,
}

/// An actor on stage.
pub struct Actor {
    /// One for most actors, one per block for a `model`.
    parts: Vec<Part>,
    camera: bool,
    /// A `player` or `mob`: the entity itself turns, there is no shape to rewrite.
    living: bool,
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
    pub fn spawn(
        def: &ActorDef,
        sets: &FxHashMap<String, BlockSet>,
        pos: DVec3,
        yaw: f32,
        scale: Option<Vec3>,
    ) -> Self {
        let shapes = match &def.kind {
            ActorKind::Model { blocks, pivot } => sets
                .get(blocks)
                .map_or_else(Vec::new, |set| model_parts(set, *pivot)),
            single => vec![single_part(single)],
        };
        let parts = shapes
            .into_iter()
            .map(|(mut shown, offset)| {
                if let Some(display) = display_of(&mut shown) {
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
                }
                Part {
                    id: next_entity_id(),
                    shown,
                    offset,
                }
            })
            .collect();
        let pose = Pose {
            pos,
            yaw,
            pitch: 0.0,
        };
        let mut actor = Self {
            parts,
            camera: def.is_camera(),
            living: matches!(def.kind, ActorKind::Player { .. } | ActorKind::Mob { .. }),
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

    /// The id of the first entity: what a viewer's camera is bound to. An actor always
    /// has one, except a model whose set is empty.
    #[must_use]
    pub fn id(&self) -> i32 {
        self.parts.first().map_or(0, |part| part.id)
    }

    /// The ids of every entity of the actor.
    pub fn ids(&self) -> impl Iterator<Item = i32> {
        self.parts.iter().map(|part| part.id)
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

    /// The entity of a `player` or `mob` actor.
    #[must_use]
    pub fn body_id(&self) -> Option<i32> {
        self.parts
            .iter()
            .find(|part| matches!(part.shown, Shown::Body(_)))
            .map(|part| part.id)
    }

    fn body(&mut self) -> Option<&mut Body> {
        self.parts
            .iter_mut()
            .find_map(|part| match &mut part.shown {
                Shown::Body(body) => Some(&mut **body),
                _ => None,
            })
    }

    /// A `player` or `mob` lies down, crouches or stands up again; other actors have no
    /// pose.
    pub fn set_body_pose(&mut self, pose: EntityPose) {
        if let Some(body) = self.body() {
            body.base().pose.set(pose);
        }
    }

    /// A creeper swells and flashes; its game never counts down, so nothing explodes.
    /// Whether the actor is a creeper.
    pub fn ignite(&mut self) -> bool {
        let Some(body) = self.body() else {
            return false;
        };
        if !ptr::eq(body.kind, &raw const vanilla_entities::CREEPER) {
            return false;
        }
        let mut creeper = CreeperEntityData::new();
        creeper.swell_dir.set(1);
        body.swelling = creeper
            .pack_dirty()
            .and_then(|values| values.into_iter().next());
        body.swelling_unsent = body.swelling.is_some();
        true
    }

    /// Moves the actor; for the camera, also where it looks.
    #[expect(
        clippy::float_cmp,
        reason = "the same yaw written again must not send the shape again"
    )]
    pub fn move_to(&mut self, pos: DVec3, yaw: f32, pitch: f32) {
        let turned = !self.camera && !self.living && yaw != self.pose.yaw;
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
        if self.camera || self.living {
            return;
        }
        // The entities themselves never turn: the yaw is part of the shape, so a model
        // turns around its middle and not around its corner.
        let full = Quat::from_rotation_y((-self.pose.yaw).to_radians()) * self.rotation;
        let scale = self.scale;
        for part in &mut self.parts {
            let translation = full * (part.offset * scale);
            let Some(display) = display_of(&mut part.shown) else {
                continue;
            };
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
    }

    /// What the entity itself is turned to: only a camera and a living actor turn, see
    /// `apply_transform`.
    const fn facing(&self, pose: Pose) -> (f32, f32) {
        if self.camera || self.living {
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
        for part in &mut self.parts {
            if let Shown::Body(body) = &mut part.shown {
                body.resolve(online);
            }
        }
        let changed: Vec<Option<Vec<DataValue>>> =
            self.parts.iter_mut().map(Part::pack_dirty).collect();
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
                player.send_packet(self.removal());
            }
        }

        for player in here {
            if self.viewers.insert(player.gameprofile.id()) {
                for part in &self.parts {
                    player.send_packet(CAddEntity {
                        id: part.id,
                        uuid: Uuid::new_v4(),
                        entity_type: part.kind().id() as i32,
                        position: target.pos,
                        velocity: DVec3::ZERO,
                        x_rot: angle_byte(pitch),
                        y_rot: angle_byte(yaw),
                        head_y_rot: angle_byte(yaw),
                        data: 0,
                    });
                    player.send_packet(CSetEntityData::new(part.id, part.pack_all()));
                    if let Shown::Body(body) = &part.shown {
                        if let Some(stack) = &body.hand {
                            player.send_packet(CSetEquipment::new(
                                part.id,
                                vec![EquipmentSlotItem {
                                    slot: EquipmentSlot::MainHand,
                                    item_stack: stack.clone(),
                                }],
                            ));
                        }
                        player.send_packet(CRotateHead {
                            entity_id: part.id,
                            head_y_rot: angle_byte(yaw),
                        });
                    }
                }
                continue;
            }
            for (part, values) in self.parts.iter().zip(&changed) {
                if let Some(values) = values {
                    player.send_packet(CSetEntityData::new(part.id, values.clone()));
                }
                if moved {
                    player.send_packet(CEntityPositionSync {
                        entity_id: part.id,
                        path: PositionPath::Linear(target.pos),
                        y_rot: yaw,
                        x_rot: pitch,
                        on_ground: self.living,
                    });
                    if self.living {
                        player.send_packet(CRotateHead {
                            entity_id: part.id,
                            head_y_rot: angle_byte(yaw),
                        });
                    }
                }
            }
        }
    }

    fn removal(&self) -> CRemoveEntities {
        CRemoveEntities {
            entity_ids: self.ids().collect(),
        }
    }

    /// The viewer's game no longer has the entities (they joined again).
    pub fn forget(&mut self, id: Uuid) {
        self.viewers.remove(&id);
    }

    /// Takes the actor away from everyone who has it.
    pub fn remove(&mut self, online: &[Arc<Player>]) {
        for player in online {
            if self.viewers.contains(&player.gameprofile.id()) {
                player.send_packet(self.removal());
            }
        }
        self.viewers.clear();
    }
}

impl Part {
    const fn kind(&self) -> EntityTypeRef {
        match &self.shown {
            Shown::Text(_) => &vanilla_entities::TEXT_DISPLAY,
            Shown::Block(_) => &vanilla_entities::BLOCK_DISPLAY,
            Shown::Item(_) => &vanilla_entities::ITEM_DISPLAY,
            Shown::Body(body) => body.kind,
        }
    }

    fn pack_all(&self) -> Vec<DataValue> {
        match &self.shown {
            Shown::Text(data) => data.pack_all(),
            Shown::Block(data) => data.pack_all(),
            Shown::Item(data) => data.pack_all(),
            Shown::Body(body) => {
                let mut values = match &body.data {
                    BodyData::Mannequin(data) => data.pack_all(),
                    BodyData::Mob(data) => data.pack_all(),
                };
                values.extend(body.swelling.clone());
                values
            }
        }
    }

    fn pack_dirty(&mut self) -> Option<Vec<DataValue>> {
        match &mut self.shown {
            Shown::Text(data) => data.pack_dirty(),
            Shown::Block(data) => data.pack_dirty(),
            Shown::Item(data) => data.pack_dirty(),
            Shown::Body(body) => {
                let mut values = match &mut body.data {
                    BodyData::Mannequin(data) => data.pack_dirty(),
                    BodyData::Mob(data) => data.pack_dirty(),
                }
                .unwrap_or_default();
                if mem::take(&mut body.swelling_unsent) {
                    values.extend(body.swelling.clone());
                }
                (!values.is_empty()).then_some(values)
            }
        }
    }
}

/// What a one-piece actor is drawn as, and where from its place.
fn single_part(kind: &ActorKind) -> (Shown, Vec3) {
    match kind {
        ActorKind::Camera | ActorKind::Model { .. } => (
            Shown::Item(Box::new(ItemDisplayEntityData::new())),
            Vec3::ZERO,
        ),
        ActorKind::Block(state) => {
            let mut data = Box::new(BlockDisplayEntityData::new());
            data.block_state.set(*state);
            (Shown::Block(data), Vec3::splat(-0.5))
        }
        ActorKind::Player { name, names, hand } => {
            (body_part(Body::mannequin(name, *names), *hand), Vec3::ZERO)
        }
        ActorKind::Mob { entity, hand } => (body_part(Body::mob(entity), *hand), Vec3::ZERO),
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
    }
}

/// A living stand-in as the show keeps it: weightless, silent, holding its item.
fn body_part(mut body: Body, hand: Option<ItemRef>) -> Shown {
    body.hand = hand.map(ItemStack::new);
    let base = body.base();
    base.no_gravity.set(true);
    base.silent.set(true);
    Shown::Body(Box::new(body))
}

/// A block display per block of the set, placed around the pivot: the one the script
/// names, or the middle of the set's bounding box.
fn model_parts(set: &BlockSet, pivot: Option<Vec3>) -> Vec<(Shown, Vec3)> {
    let pivot = pivot.unwrap_or_else(|| {
        let (mut min, mut max) = (Vec3::MAX, Vec3::MIN);
        for (offset, _) in set.entries() {
            min = min.min(offset.as_vec3());
            max = max.max(offset.as_vec3() + Vec3::ONE);
        }
        (min + max) * 0.5
    });
    set.entries()
        .iter()
        .filter(|(_, state)| !state.is_air())
        .map(|(offset, state)| {
            let mut data = Box::new(BlockDisplayEntityData::new());
            data.block_state.set(*state);
            (Shown::Block(data), offset.as_vec3() - pivot)
        })
        .collect()
}

const fn display_of(shown: &mut Shown) -> Option<&mut DisplayEntityData> {
    match shown {
        Shown::Text(data) => Some(&mut data.display),
        Shown::Block(data) => Some(&mut data.display),
        Shown::Item(data) => Some(&mut data.display),
        Shown::Body(_) => None,
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
    use serde_json::json;
    use steel_registry::init_vanilla_registry;

    use super::*;

    fn set() -> BlockSet {
        let value = json!({"blocks": [
            {"p": [0, 0, 0], "s": "minecraft:stone"},
            {"p": [1, 0, 0], "s": "minecraft:gold_block"},
            {"p": [1, 1, 0], "s": "minecraft:air"},
        ]});
        let serde_json::Value::Object(object) = value else {
            unreachable!("an object literal")
        };
        BlockSet::from_json(&object).expect("a set")
    }

    #[test]
    fn a_model_is_a_display_per_block_around_its_middle() {
        init_vanilla_registry();
        let parts = model_parts(&set(), None);
        // Air takes no display but counts for the middle: the box is 2 x 2 x 1.
        let offsets: Vec<Vec3> = parts.iter().map(|(_, offset)| *offset).collect();
        assert_eq!(
            offsets,
            [Vec3::new(-1.0, -1.0, -0.5), Vec3::new(0.0, -1.0, -0.5)]
        );
        let named = model_parts(&set(), Some(Vec3::ZERO));
        assert_eq!(named[1].1, Vec3::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn angles_wrap_into_a_byte() {
        assert_eq!(angle_byte(0.0), 0);
        assert_eq!(angle_byte(90.0), 64);
        assert_eq!(angle_byte(180.0), -128);
        assert_eq!(angle_byte(-90.0), -64);
        assert_eq!(angle_byte(450.0), 64);
    }
}
