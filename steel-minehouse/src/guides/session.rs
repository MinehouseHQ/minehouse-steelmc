//! A guide open for one player (the Fabric `GuideSession`): the stage in front of them, a
//! caption over it and the buttons under it (back, again, next, close). The steps play on
//! their own one after another.

use std::sync::Arc;

use glam::{DVec3, Vec3};
use steel_core::{entity::Entity as _, player::Player, world::World};
use steel_protocol::packets::game::CRemoveEntities;
use steel_registry::sound_events;
use text_components::{Modifier as _, TextComponent, format::Color};

use super::{
    Guide,
    stage::{self, Element, Stage},
};

/// Walking this far from where the guide was opened closes it.
const LEAVE_DISTANCE: f64 = 8.0;
/// A finished guide closes by itself after this long.
const LINGER_TICKS: u32 = 20 * 60;
/// The pause after a step before the next one starts, ticks.
const PAUSE_TICKS: u32 = 40;
/// The reading time of a character of a caption, ticks.
const READ_TICKS: f32 = 2.2;
/// How far in front of the eyes the near edge of the model is, and how far under them
/// its floor: like a table within reach.
const REACH: f32 = 1.7;
const DROP: f64 = 0.8;
/// The size of a button to click, and of its text.
const BUTTON_WIDTH: f32 = 0.56;
const BUTTON_HEIGHT: f32 = 0.26;
const BUTTON_SCALE: f32 = 0.4;
const CAPTION_SCALE: f32 = 0.55;
const CAPTION_BACKGROUND: i32 = i32::from_be_bytes([0xA0, 0x10, 0x10, 0x10]);
const BUTTON_BACKGROUND: i32 = i32::from_be_bytes([0xC0, 0x20, 0x20, 0x20]);
const TITLE_COLOR: Color = Color::Rgb(0xFF, 0xC8, 0x57);
/// Both hands click at once; a second click this soon is the same one.
const CLICK_TICKS: u64 = 4;

/// What a button of the panel does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Button {
    Back,
    Again,
    Next,
    Close,
}

const BUTTONS: [(Button, &str, f32); 4] = [
    (Button::Back, "« Назад", -0.93),
    (Button::Again, "⟳ Сначала", -0.31),
    (Button::Next, "Дальше »", 0.31),
    (Button::Close, "✕ Закрыть", 0.93),
];

/// How long a step is shown before the pause: its own time, or the time to read its caption.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "a caption is a few hundred characters"
)]
pub(super) fn step_ticks(ticks: u32, caption: &str) -> u32 {
    let read = (caption.chars().count() as f32 * READ_TICKS).round() as u32;
    ticks.max(read)
}

/// The caption of a step: the title with the number of the step, and the words under it.
pub(super) fn caption(guide: &Guide, index: usize) -> TextComponent {
    let head = format!("{}  ·  {}/{}", guide.title, index + 1, guide.steps.len());
    let words = guide.steps.get(index).map_or("", |step| step.caption);
    TextComponent::plain(head)
        .color(TITLE_COLOR)
        .bold(true)
        .add_child(
            TextComponent::plain(format!("\n{words}"))
                .color(Color::White)
                .bold(false),
        )
}

pub(super) struct Session {
    pub(super) guide: &'static Guide,
    world: Arc<World>,
    origin: DVec3,
    stage: Stage,
    caption: Element,
    /// The texts of the buttons and the boxes that take their clicks.
    fixed: Vec<i32>,
    buttons: Vec<(i32, Button)>,
    step: usize,
    step_ticks: u32,
    end_ticks: u32,
    last_click: Option<u64>,
    ticks: u64,
}

impl Session {
    pub(super) fn open(player: &Arc<Player>, guide: &'static Guide) -> Self {
        let yaw = player.rotation().0.to_radians();
        let forward = Vec3::new(-yaw.sin(), 0.0, yaw.cos());
        let right = Vec3::new(-yaw.cos(), 0.0, -yaw.sin());
        let half_depth = f32::from(guide.depth) * stage::SCALE / 2.0;
        let position = player.position();
        let eyes = DVec3::new(position.x, player.get_eye_y(), position.z);
        let center =
            eyes + (forward * (REACH + half_depth)).as_dvec3() - DVec3::new(0.0, DROP, 0.0);
        let stage = Stage::new(
            Arc::clone(player),
            center,
            right,
            -forward,
            guide.width,
            guide.depth,
        );

        // Over the far edge of the model, which the tilt lifts.
        let top = 0.55 + half_depth * stage::TILT.sin() + 0.65;
        let mut caption = Element::text(TextComponent::plain(""), 300, CAPTION_BACKGROUND);
        let at = stage.face_point(0.0, top, -half_depth * 0.5);
        caption.transform(&stage.face_matrix(at, CAPTION_SCALE), 0);
        caption.sync(player, center);

        // Right under the caption, in sight and in reach.
        let y = top - 0.3;
        let z = -half_depth * 0.5 + 0.05;
        let mut fixed = Vec::new();
        let mut buttons = Vec::new();
        for (button, name, x) in BUTTONS {
            let text = TextComponent::plain(name).color(Color::White);
            let mut label = Element::text(text, 200, BUTTON_BACKGROUND);
            label.transform(
                &stage.face_matrix(stage.face_point(x, y, z), BUTTON_SCALE),
                0,
            );
            label.sync(player, center);
            fixed.push(label.id);
            let at = center + stage.face_point(x, y - 0.04, z).as_dvec3();
            let hit = stage::click_box(player, at, BUTTON_WIDTH, BUTTON_HEIGHT);
            fixed.push(hit);
            buttons.push((hit, button));
        }

        let mut session = Self {
            guide,
            world: player.get_world(),
            origin: position,
            stage,
            caption,
            fixed,
            buttons,
            step: 0,
            step_ticks: 0,
            end_ticks: 0,
            last_click: None,
            ticks: 0,
        };
        session.start(0);
        session
    }

    /// The button the entity `id` is, if it is one of this guide.
    pub(super) fn button(&self, id: i32) -> Option<Button> {
        self.buttons
            .iter()
            .find(|(entity, _)| *entity == id)
            .map(|&(_, button)| button)
    }

    /// A click on a button. `false` once the guide should close.
    pub(super) fn click(&mut self, button: Button) -> bool {
        if self
            .last_click
            .is_some_and(|last| self.ticks - last < CLICK_TICKS)
        {
            return true;
        }
        self.last_click = Some(self.ticks);
        self.stage.sound(&sound_events::UI_BUTTON_CLICK, 1.0);
        match button {
            Button::Back => self.go(self.step.saturating_sub(1)),
            Button::Again => self.go(0),
            Button::Next => {
                if self.step + 1 < self.guide.steps.len() {
                    self.stage.finish();
                    self.start(self.step + 1);
                }
            }
            Button::Close => return false,
        }
        true
    }

    /// Shows a step: the ones before it are replayed instantly, this one plays.
    fn go(&mut self, target: usize) {
        let target = target.min(self.guide.steps.len().saturating_sub(1));
        self.stage.clear();
        self.stage.instant = true;
        for step in self.guide.steps.iter().take(target) {
            (step.script)(&mut self.stage);
        }
        self.stage.finish();
        self.stage.instant = false;
        self.start(target);
    }

    fn start(&mut self, index: usize) {
        let Some(step) = self.guide.steps.get(index) else {
            return;
        };
        self.step = index;
        self.step_ticks = 0;
        self.end_ticks = 0;
        self.caption.set_text(caption(self.guide, index));
        self.caption.sync(self.stage.player(), self.stage.center());
        if index > 0 {
            self.stage.sound(&sound_events::ITEM_BOOK_PAGE_TURN, 1.0);
        }
        (step.script)(&mut self.stage);
    }

    /// One server tick; `false` once the guide should close.
    pub(super) fn tick(&mut self) -> bool {
        self.ticks += 1;
        let player = self.stage.player();
        if !player.is_alive()
            || !Arc::ptr_eq(&player.get_world(), &self.world)
            || player.position().distance_squared(self.origin) > LEAVE_DISTANCE * LEAVE_DISTANCE
        {
            return false;
        }
        self.stage.tick();
        self.step_ticks += 1;
        let Some(step) = self.guide.steps.get(self.step) else {
            return false;
        };
        let length = step_ticks(step.ticks, step.caption);
        if self.step_ticks >= length + PAUSE_TICKS && !self.stage.busy() {
            if self.step + 1 < self.guide.steps.len() {
                self.start(self.step + 1);
            } else {
                self.end_ticks += 1;
                if self.end_ticks > LINGER_TICKS {
                    return false;
                }
            }
        }
        true
    }

    /// Takes everything of the guide away from the player.
    pub(super) fn close(mut self) {
        self.stage.clear();
        let mut entity_ids = self.fixed;
        entity_ids.push(self.caption.id);
        self.stage
            .player()
            .send_packet(CRemoveEntities { entity_ids });
    }
}
