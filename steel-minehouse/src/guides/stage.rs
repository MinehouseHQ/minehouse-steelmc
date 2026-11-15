//! The miniature a guide is played on (the Fabric `GuideStage`): a little world of display
//! entities in front of one player, seen only by them. Things in it are parts at cell
//! coordinates (one cell is one block of the model, [`SCALE`] of a real one); they drop in,
//! move, turn, change and vanish with the client gliding between the states, so the server
//! sends only a few updates.
//!
//! A script of a step runs against the stage and says what comes next with
//! [`Stage::later`]. In the instant mode everything happens at once with no animation: that
//! is how the stage jumps to a step (the steps before it are replayed instantly) or
//! finishes the current one.

use std::{collections::BTreeMap, mem, str::FromStr as _, sync::Arc};

use glam::{DVec3, Mat3, Mat4, Quat, Vec3};
use steel_core::{entity::next_entity_id, player::Player};
use steel_protocol::packets::game::{
    CAddEntity, CRemoveEntities, CSetEntityData, CSound, SoundSource,
};
use steel_registry::{
    RegistryEntry as _,
    data_components::vanilla_components::{ITEM_MODEL, PROFILE},
    entity_data::{DataValue, Quaternionf, Vector3f},
    entity_type::EntityTypeRef,
    item_stack::ItemStack,
    resolvable_profile::{PlayerSkinPatch, ResolvableProfile},
    sound_event::SoundEvent,
    vanilla_entities,
    vanilla_entity_data::{
        BlockDisplayEntityData, DisplayEntityData, InteractionEntityData, ItemDisplayEntityData,
        TextDisplayEntityData,
    },
    vanilla_items,
};
use steel_utils::{BlockStateId, Identifier};
use text_components::TextComponent;
use uuid::Uuid;

/// The size of one cell of the model in the world.
pub const SCALE: f32 = 0.28;
/// How far the model is turned from facing the player straight on, radians.
pub const TURN: f32 = -0.35;
/// How far the model is tipped towards the player, radians, so it is seen from above.
pub const TILT: f32 = 0.5;
/// How long a part drops in or shrinks away, ticks.
pub const APPEAR_TICKS: i32 = 5;

/// Vanilla `TextDisplay.FLAG_SHADOW`.
const FLAG_SHADOW: i8 = 1;
/// Vanilla `Brightness(15, 15).pack()`.
const FULL_BRIGHT: i32 = 15 << 4 | 15 << 20;
/// Vanilla `Entity.FLAG_GLOWING` of the shared flags.
const FLAG_GLOWING: i8 = 1 << 6;
/// Vanilla `ItemDisplayContext.NONE` and `FIXED`.
const CONTEXT_NONE: i8 = 0;
const CONTEXT_FIXED: i8 = 8;
/// The background of a label: a see-through black.
const LABEL_BACKGROUND: i32 = 0x6000_0000;
/// How loud the sounds of a guide are.
const VOLUME: f32 = 0.6;

/// What a display entity shows.
enum Shown {
    Text(Box<TextDisplayEntityData>),
    Block(Box<BlockDisplayEntityData>),
    Item(Box<ItemDisplayEntityData>),
}

/// A display entity the player has.
pub(super) struct Element {
    pub(super) id: i32,
    shown: Shown,
    spawned: bool,
}

fn setup(display: &mut DisplayEntityData) {
    display.brightness_override.set(FULL_BRIGHT);
    display.view_range.set(0.5);
}

impl Element {
    const fn new(shown: Shown) -> Self {
        Self {
            id: 0,
            shown,
            spawned: false,
        }
    }

    pub(super) fn text(text: TextComponent, line_width: i32, background: i32) -> Self {
        let mut data = Box::new(TextDisplayEntityData::new());
        data.text.set(Box::new(text));
        data.background_color.set(background);
        data.style_flags.set(FLAG_SHADOW);
        data.line_width.set(line_width);
        setup(&mut data.display);
        Self::new(Shown::Text(data))
    }

    fn block(state: BlockStateId) -> Self {
        let mut data = Box::new(BlockDisplayEntityData::new());
        data.block_state.set(state);
        setup(&mut data.display);
        Self::new(Shown::Block(data))
    }

    fn item(stack: ItemStack, context: i8) -> Self {
        let mut data = Box::new(ItemDisplayEntityData::new());
        data.item_stack.set(stack);
        data.item_display.set(context);
        setup(&mut data.display);
        Self::new(Shown::Item(data))
    }

    pub(super) const fn display(&mut self) -> &mut DisplayEntityData {
        match &mut self.shown {
            Shown::Text(data) => &mut data.display,
            Shown::Block(data) => &mut data.display,
            Shown::Item(data) => &mut data.display,
        }
    }

    const fn kind(&self) -> EntityTypeRef {
        match self.shown {
            Shown::Text(_) => &vanilla_entities::TEXT_DISPLAY,
            Shown::Block(_) => &vanilla_entities::BLOCK_DISPLAY,
            Shown::Item(_) => &vanilla_entities::ITEM_DISPLAY,
        }
    }

    pub(super) fn set_text(&mut self, text: TextComponent) {
        if let Shown::Text(data) = &mut self.shown {
            data.text.set(Box::new(text));
        }
    }

    /// Where the entity is drawn from its place, gliding there over `ticks`.
    pub(super) fn transform(&mut self, matrix: &Mat4, ticks: i32) {
        // Every matrix of a guide is a turn of a stretch along the axes, which is what the
        // client puts together again from these three.
        let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
        let display = self.display();
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
        display.transformation_interpolation_duration.set(ticks);
        display
            .transformation_interpolation_start_delta_ticks
            .set_forced(0);
    }

    fn glow(&mut self, rgb: u32) {
        let display = self.display();
        display
            .base
            .shared_flags
            .set(if rgb == 0 { 0 } else { FLAG_GLOWING });
        display
            .glow_color_override
            .set(i32::try_from(rgb).ok().filter(|_| rgb != 0).unwrap_or(-1));
    }

    fn packed(&mut self) -> Option<Vec<DataValue>> {
        match &mut self.shown {
            Shown::Text(data) => data.pack_dirty(),
            Shown::Block(data) => data.pack_dirty(),
            Shown::Item(data) => data.pack_dirty(),
        }
    }

    /// Sends the entity, or what changed of it, to the player.
    pub(super) fn sync(&mut self, player: &Player, at: DVec3) {
        if !self.spawned {
            self.spawned = true;
            self.id = next_entity_id();
            player.send_packet(add_entity(self.id, self.kind(), at));
        }
        if let Some(values) = self.packed() {
            player.send_packet(CSetEntityData::new(self.id, values));
        }
    }
}

fn add_entity(id: i32, kind: EntityTypeRef, at: DVec3) -> CAddEntity {
    CAddEntity {
        id,
        uuid: Uuid::new_v4(),
        entity_type: kind.id() as i32,
        position: at,
        velocity: DVec3::ZERO,
        x_rot: 0,
        y_rot: 0,
        head_y_rot: 0,
        data: 0,
    }
}

/// Sends a box that takes clicks: `width` wide and `height` tall, its bottom middle at `at`.
/// Returns the id of the entity.
pub(super) fn click_box(player: &Player, at: DVec3, width: f32, height: f32) -> i32 {
    let id = next_entity_id();
    let mut data = InteractionEntityData::new();
    data.width.set(width);
    data.height.set(height);
    player.send_packet(add_entity(id, &vanilla_entities::INTERACTION, at));
    if let Some(values) = data.pack_dirty() {
        player.send_packet(CSetEntityData::new(id, values));
    }
    id
}

/// A piece of paper drawn with the item model `model` of the server pack: a block of ours
/// that a block display can't show.
#[must_use]
pub fn model_stack(model: &str) -> ItemStack {
    let mut stack = ItemStack::new(&vanilla_items::PAPER);
    if let Ok(model) = Identifier::from_str(model) {
        stack.set(ITEM_MODEL, model);
    }
    stack
}

/// How a piece lies in its part.
enum Place {
    /// In the cell's space; it turns with the part.
    Local(Mat4),
    /// Always turned to the player: only the place `at` comes from the model.
    Facing { at: Vec3, scale: f32 },
}

/// One display entity of a part.
pub struct Piece {
    element: Element,
    place: Place,
    /// A model of the pack: its model can be changed.
    model: bool,
}

impl Piece {
    /// A block; `local` puts it in the cell's space.
    #[must_use]
    pub fn block(state: BlockStateId, local: Mat4) -> Self {
        Self {
            element: Element::block(state),
            place: Place::Local(local),
            model: false,
        }
    }

    /// An item hanging at `at` of the cell, facing the player, `size` cells big.
    #[must_use]
    pub fn item(stack: ItemStack, at: Vec3, size: f32) -> Self {
        Self {
            element: Element::item(stack, CONTEXT_FIXED),
            place: Place::Facing { at, scale: size },
            model: false,
        }
    }

    /// An item model of the pack as a full block of the cell.
    #[must_use]
    pub fn model(model: &str) -> Self {
        Self {
            element: Element::item(model_stack(model), CONTEXT_NONE),
            place: Place::Local(Mat4::from_translation(Vec3::splat(0.5))),
            model: true,
        }
    }

    /// A text at `at` of the cell, facing the player.
    #[must_use]
    pub fn text(text: TextComponent, scale: f32, at: Vec3) -> Self {
        Self {
            element: Element::text(text, 240, LABEL_BACKGROUND),
            place: Place::Facing { at, scale },
            model: false,
        }
    }
}

/// One thing in the model. Its cell is (x, y, z); the yaw turns it about the cell's middle.
struct Part {
    pieces: Vec<Piece>,
    x: f32,
    y: f32,
    z: f32,
    yaw: f32,
    /// Dropping in and vanishing: how far above its place it is and how big it is.
    lift: f32,
    grow: f32,
}

type Action = Box<dyn FnOnce(&mut Stage) + Send>;

struct Timer {
    at: u64,
    order: u64,
    action: Action,
}

/// The model of a guide in front of a player.
pub struct Stage {
    player: Arc<Player>,
    /// Where the entities stand; everything is drawn from here.
    center: DVec3,
    /// From the panel (x right, y up, z towards the player) to the world. A turn only.
    face: Mat4,
    /// From the cells of the model to the world.
    model: Mat4,
    /// Everything happens at once, with no animation and no sound.
    pub(super) instant: bool,
    parts: BTreeMap<String, Part>,
    /// The entities of parts that are shrinking away.
    dying: Vec<i32>,
    timers: Vec<Timer>,
    now: u64,
    order: u64,
}

impl Stage {
    pub(super) fn new(
        player: Arc<Player>,
        center: DVec3,
        right: Vec3,
        to_player: Vec3,
        width: u8,
        depth: u8,
    ) -> Self {
        let face = Mat4::from_mat3(Mat3::from_cols(right, Vec3::Y, to_player));
        let model = face
            * Mat4::from_rotation_x(TILT)
            * Mat4::from_rotation_y(TURN)
            * Mat4::from_scale(Vec3::splat(SCALE))
            * Mat4::from_translation(Vec3::new(
                -f32::from(width) / 2.0,
                0.0,
                -f32::from(depth) / 2.0,
            ));
        Self {
            player,
            center,
            face,
            model,
            instant: false,
            parts: BTreeMap::new(),
            dying: Vec::new(),
            timers: Vec::new(),
            now: 0,
            order: 0,
        }
    }

    /// The player the guide is shown to.
    #[must_use]
    pub const fn player(&self) -> &Arc<Player> {
        &self.player
    }

    pub(super) const fn center(&self) -> DVec3 {
        self.center
    }

    /// Where a point of the panel is from the center.
    pub(super) fn face_point(&self, x: f32, y: f32, z: f32) -> Vec3 {
        self.face.transform_point3(Vec3::new(x, y, z))
    }

    /// A text of the panel at `at`, `scale` big.
    pub(super) fn face_matrix(&self, at: Vec3, scale: f32) -> Mat4 {
        Mat4::from_translation(at) * self.face * Mat4::from_scale(Vec3::splat(scale))
    }

    // --- Time ---------------------------------------------------------------------------

    /// Runs `action` in `ticks` (right now when instant).
    pub fn later(&mut self, ticks: u32, action: impl FnOnce(&mut Self) + Send + 'static) {
        if self.instant || ticks == 0 {
            action(self);
        } else {
            self.timers.push(Timer {
                at: self.now + u64::from(ticks),
                order: self.order,
                action: Box::new(action),
            });
            self.order += 1;
        }
    }

    pub(super) const fn busy(&self) -> bool {
        !self.timers.is_empty()
    }

    /// The first of the timers that are due by `until`.
    fn next_due(&mut self, until: u64) -> Option<Action> {
        let index = self
            .timers
            .iter()
            .enumerate()
            .filter(|(_, timer)| timer.at <= until)
            .min_by_key(|(_, timer)| (timer.at, timer.order))
            .map(|(index, _)| index)?;
        Some(self.timers.swap_remove(index).action)
    }

    pub(super) fn tick(&mut self) {
        self.now += 1;
        while let Some(action) = self.next_due(self.now) {
            action(self);
        }
    }

    /// Runs everything still to come at once, so the stage is where the step ends.
    pub(super) fn finish(&mut self) {
        let was = self.instant;
        self.instant = true;
        while let Some(action) = self.next_due(u64::MAX) {
            action(self);
        }
        self.instant = was;
    }

    /// Empties the model (the caption and the buttons are not parts and stay).
    pub(super) fn clear(&mut self) {
        self.timers.clear();
        let mut entity_ids = mem::take(&mut self.dying);
        for part in mem::take(&mut self.parts).into_values() {
            entity_ids.extend(part.pieces.iter().map(|piece| piece.element.id));
        }
        self.kill(entity_ids);
    }

    fn kill(&self, entity_ids: Vec<i32>) {
        if !entity_ids.is_empty() {
            self.player.send_packet(CRemoveEntities { entity_ids });
        }
    }

    /// Plays a sound at the model.
    pub fn sound(&self, sound: &'static SoundEvent, pitch: f32) {
        if !self.instant {
            self.player.send_packet(CSound::new(
                sound,
                SoundSource::Master,
                self.center,
                VOLUME,
                pitch,
                0,
            ));
        }
    }

    // --- Parts --------------------------------------------------------------------------

    /// Whether the model has the part `id`.
    #[must_use]
    pub fn has(&self, id: &str) -> bool {
        self.parts.contains_key(id)
    }

    /// A block at a cell, dropping in.
    pub fn block(&mut self, id: &str, state: BlockStateId, x: f32, y: f32, z: f32) {
        self.group(id, x, y, z, vec![Piece::block(state, Mat4::IDENTITY)]);
    }

    /// A stretched block: `from` and `size` in cells, from the part's cell.
    pub fn stretched(&mut self, id: &str, state: BlockStateId, cell: Vec3, from: Vec3, size: Vec3) {
        let local = Mat4::from_translation(from) * Mat4::from_scale(size);
        self.group(id, cell.x, cell.y, cell.z, vec![Piece::block(state, local)]);
    }

    /// An item hanging in the middle of a cell, facing the player.
    pub fn item(&mut self, id: &str, stack: ItemStack, x: f32, y: f32, z: f32, size: f32) {
        let piece = Piece::item(stack, Vec3::splat(0.5), size);
        self.group(id, x, y, z, vec![piece]);
    }

    /// A label over a cell, facing the player.
    pub fn label(&mut self, id: &str, text: TextComponent, x: f32, y: f32, z: f32) {
        let piece = Piece::text(text, 0.9, Vec3::new(0.5, 0.2, 0.5));
        self.group(id, x, y, z, vec![piece]);
    }

    /// A block drawn with an item model of the server pack.
    pub fn model(&mut self, id: &str, model: &str, x: f32, y: f32, z: f32) {
        self.group(id, x, y, z, vec![Piece::model(model)]);
    }

    /// A part put together by hand, dropping in. A part that had the id goes at once.
    pub fn group(&mut self, id: &str, x: f32, y: f32, z: f32, pieces: Vec<Piece>) {
        let mut part = Part {
            pieces,
            x,
            y,
            z,
            yaw: 0.0,
            lift: 0.0,
            grow: 1.0,
        };
        if !self.instant {
            part.lift = 0.6;
            part.grow = 0.05;
        }
        self.render(&mut part, 0);
        if let Some(old) = self.parts.insert(id.to_owned(), part) {
            self.kill(old.pieces.iter().map(|piece| piece.element.id).collect());
        }
        if !self.instant {
            let id = id.to_owned();
            self.later(1, move |stage| {
                stage.with(&id, APPEAR_TICKS, |part| {
                    part.lift = 0.0;
                    part.grow = 1.0;
                });
            });
        }
    }

    /// The viewer's own head, to put them in the scene.
    #[must_use]
    pub fn player_head(&self) -> ItemStack {
        let mut skull = ItemStack::new(&vanilla_items::PLAYER_HEAD);
        skull.set(
            PROFILE,
            ResolvableProfile::static_full(
                self.player.gameprofile.clone(),
                PlayerSkinPatch::default(),
            ),
        );
        skull
    }

    /// Takes a part away: it shrinks and goes.
    pub fn remove(&mut self, id: &str) {
        let Some(mut part) = self.parts.remove(id) else {
            return;
        };
        let entity_ids: Vec<i32> = part.pieces.iter().map(|piece| piece.element.id).collect();
        if self.instant {
            self.kill(entity_ids);
            return;
        }
        part.grow = 0.05;
        self.render(&mut part, APPEAR_TICKS - 1);
        self.dying.extend(&entity_ids);
        self.later(APPEAR_TICKS.unsigned_abs(), move |stage| {
            stage.dying.retain(|id| !entity_ids.contains(id));
            stage.kill(entity_ids);
        });
    }

    /// Changes the part `id`, if it is there, and draws it again over `ticks`.
    fn with(&mut self, id: &str, ticks: i32, change: impl FnOnce(&mut Part)) {
        let Some(mut part) = self.parts.remove(id) else {
            return;
        };
        change(&mut part);
        self.render(&mut part, if self.instant { 0 } else { ticks });
        self.parts.insert(id.to_owned(), part);
    }

    /// Moves a part to a cell (and a yaw), taking `ticks`.
    pub fn move_to(&mut self, id: &str, x: f32, y: f32, z: f32, yaw: f32, ticks: i32) {
        self.with(id, ticks, |part| {
            part.x = x;
            part.y = y;
            part.z = z;
            part.yaw = yaw;
        });
    }

    /// Moves a part by `dx` and `dz` cells, taking `ticks`.
    pub fn move_by(&mut self, id: &str, dx: f32, dz: f32, ticks: i32) {
        self.with(id, ticks, |part| {
            part.x += dx;
            part.z += dz;
        });
    }

    /// Changes the block of every block piece of a part.
    pub fn set_state(&mut self, id: &str, state: BlockStateId) {
        self.with(id, 0, |part| {
            for piece in &mut part.pieces {
                if let Shown::Block(data) = &mut piece.element.shown {
                    data.block_state.set(state);
                }
            }
        });
    }

    /// Changes the model of every model piece of a part.
    pub fn set_model(&mut self, id: &str, model: &str) {
        self.with(id, 0, |part| {
            for piece in part.pieces.iter_mut().filter(|piece| piece.model) {
                if let Shown::Item(data) = &mut piece.element.shown {
                    data.item_stack.set(model_stack(model));
                }
            }
        });
    }

    /// Changes the text of every text piece of a part.
    pub fn set_text(&mut self, id: &str, text: &TextComponent) {
        self.with(id, 0, |part| {
            for piece in &mut part.pieces {
                piece.element.set_text(text.clone());
            }
        });
    }

    /// A glowing outline of a color, or none (0).
    pub fn glow(&mut self, id: &str, rgb: u32) {
        self.with(id, 0, |part| {
            for piece in &mut part.pieces {
                piece.element.glow(rgb);
            }
        });
    }

    /// Swells a part a little and back: "look here".
    pub fn pulse(&mut self, id: &str) {
        if self.instant {
            return;
        }
        self.with(id, 3, |part| part.grow = 1.25);
        let id = id.to_owned();
        self.later(4, move |stage| stage.with(&id, 4, |part| part.grow = 1.0));
    }

    /// Sends where the pieces of a part are now; they glide there over `ticks`.
    fn render(&self, part: &mut Part, ticks: i32) {
        let cell = Vec3::new(part.x, part.y + part.lift, part.z);
        for piece in &mut part.pieces {
            let matrix = match &piece.place {
                Place::Facing { at, scale } => {
                    let at = self.model.transform_point3(cell + *at);
                    self.face_matrix(at, scale * part.grow * SCALE * 2.0)
                }
                Place::Local(local) => {
                    self.model
                        * Mat4::from_translation(cell + Vec3::new(0.5, 0.0, 0.5))
                        * Mat4::from_quat(Quat::from_rotation_y(part.yaw))
                        // Grows about the middle of the cell's bottom.
                        * Mat4::from_scale(Vec3::splat(part.grow))
                        * Mat4::from_translation(Vec3::new(-0.5, 0.0, -0.5))
                        * *local
                }
            };
            piece.element.transform(&matrix, ticks);
            piece.element.sync(&self.player, self.center);
        }
    }
}
