//! The borders of zones and their markup by admins (the Fabric `ZoneWand`).
//!
//! `/zones show` draws the borders of the saved zones with particles for 30 seconds. The
//! stick «Разметка зон» marks a new zone: a right click on a block is a corner, with Shift
//! the last corner is taken back, a left click tells what the draft holds. While an admin
//! holds the stick the draft and the saved zones are drawn for them.
//!
//! The Fabric server hands the shapes to the `MineHouse` client mod, which draws lines and
//! walls. Steel does not know which channels a client listens to, so everyone gets the
//! particles.

use std::{
    collections::BTreeMap,
    f64::consts::TAU,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use glam::DVec3;
use serde_json::{Value, json};
use simdnbt::owned::NbtCompound;
use steel_core::{
    behavior::InteractionResult, entity::Entity as _, player::Player, server::Server, world::World,
};
use steel_registry::{
    data_components::{
        CustomData,
        vanilla_components::{CUSTOM_DATA, ENCHANTMENT_GLINT_OVERRIDE, ITEM_NAME, ItemLore, LORE},
    },
    item_stack::ItemStack,
    items::item::BlockHitResult,
    particle_type::{DustParticleOptions, ParticleData},
    vanilla_items, vanilla_particle_types,
};
use steel_utils::{RgbColor, locks::SyncMutex, types::InteractionHand};
use text_components::{Modifier as _, TextComponent, format::Color};
use uuid::Uuid;

use super::{Zone, commands::is_admin};
use crate::bank::{Bank, bank, gui::line};

const KEY: &str = "minehouse_zone_wand";
/// How long `/zones show` draws the borders, in ticks.
const SHOW_TICKS: u64 = 20 * 30;
/// Particles are drawn this near the player, in blocks.
const NEAR: f64 = 64.0;
const DRAFT_COLOR: i32 = 0x00B0_4CFF;
const CORNER_COLOR: i32 = 0x00FF_4CF0;

/// A zone an admin is marking out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// The corners, `(x, z)`.
    pub points: Vec<(i32, i32)>,
    /// The lowest block of the zone.
    pub y_min: i32,
    /// The highest block of the zone.
    pub y_max: i32,
    /// The dimension.
    pub world: String,
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            y_min: 30,
            y_max: 120,
            world: "minecraft:overworld".to_owned(),
        }
    }
}

impl Draft {
    /// A circle of `radius` blocks around a block: 16 to 64 corners.
    pub fn circle(&mut self, world: &str, x: i32, z: i32, radius: i32) {
        let corners = (radius * 2).clamp(16, 64);
        world.clone_into(&mut self.world);
        self.points = (0..corners)
            .map(|corner| {
                let angle = TAU * f64::from(corner) / f64::from(corners);
                let (sin, cos) = angle.sin_cos();
                (
                    x + round(f64::from(radius) * cos),
                    z + round(f64::from(radius) * sin),
                )
            })
            .collect();
    }

    /// The draft as the corners of a saved zone, to move them.
    pub fn take(&mut self, zone: &Zone) {
        self.points.clone_from(&zone.points);
        self.y_min = zone.y_min;
        self.y_max = zone.y_max;
        self.world.clone_from(&zone.world);
    }

    /// What `zone_save` sends to the site.
    #[must_use]
    pub fn body(&self, num: i32, common: bool, title: Option<&str>) -> Value {
        let points: Vec<[i32; 2]> = self.points.iter().map(|(x, z)| [*x, *z]).collect();
        let mut body = json!({
            "num": num,
            "kind": if common { "common" } else { "lot" },
            "world": self.world,
            "y_min": self.y_min,
            "y_max": self.y_max,
            "points": points,
        });
        if let Some(title) = title {
            body["title"] = json!(title);
        }
        body
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a circle of at most 500 blocks around a block of the world"
)]
const fn round(value: f64) -> i32 {
    value.round() as i32
}

/// Which borders `/zones show` draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Show {
    All,
    One(i32),
}

/// The drafts of the admins and who asked to see the borders.
#[derive(Default)]
pub struct Markup {
    drafts: SyncMutex<BTreeMap<Uuid, Draft>>,
    /// Until which tick a player sees the borders, and which.
    showing: SyncMutex<BTreeMap<Uuid, (u64, Show)>>,
    /// The server tick the borders were last drawn on.
    tick: AtomicU64,
}

impl Markup {
    /// Changes the draft of a player, starting an empty one if there is none.
    pub fn edit<T>(&self, player: Uuid, change: impl FnOnce(&mut Draft) -> T) -> T {
        change(self.drafts.lock().entry(player).or_default())
    }

    /// The draft of a player, if they have one.
    #[must_use]
    pub fn draft(&self, player: Uuid) -> Option<Draft> {
        self.drafts.lock().get(&player).cloned()
    }

    /// Throws the draft of a player away.
    pub fn clear(&self, player: Uuid) {
        self.drafts.lock().remove(&player);
    }

    /// Draws the borders of a zone (`None`: of all) for a player for half a minute.
    pub fn show(&self, player: Uuid, num: Option<i32>) {
        let until = self.tick.load(Ordering::Relaxed) + SHOW_TICKS;
        let show = num.map_or(Show::All, Show::One);
        self.showing.lock().insert(player, (until, show));
    }

    pub(super) fn forget(&self, player: Uuid) {
        self.showing.lock().remove(&player);
    }
}

/// The stick «Разметка зон».
#[must_use]
pub fn wand() -> ItemStack {
    let mut stack = ItemStack::new(&vanilla_items::BLAZE_ROD);
    let mut tag = NbtCompound::new();
    tag.insert(KEY, 1_i8);
    if let Some(data) = CustomData::try_from_compound(tag) {
        stack.set(CUSTOM_DATA, data);
    }
    stack.set(ITEM_NAME, TextComponent::plain("Разметка зон"));
    if let Ok(lore) = ItemLore::new(vec![
        line("ПКМ по блоку: вершина зоны", Color::Gray),
        line("Shift+ПКМ: убрать последнюю", Color::Gray),
        line("/zones save <номер>: сохранить", Color::DarkGray),
    ]) {
        stack.set(LORE, lore);
    }
    stack.set(ENCHANTMENT_GLINT_OVERRIDE, true);
    stack
}

fn is_wand(stack: &ItemStack) -> bool {
    stack.is(&vanilla_items::BLAZE_ROD)
        && stack
            .get(CUSTOM_DATA)
            .is_some_and(|data| data.as_compound().byte(KEY) == Some(1))
}

fn holds_wand(player: &Player) -> bool {
    is_wand(
        player
            .inventory
            .lock()
            .get_item_in_hand(InteractionHand::MainHand),
    )
}

/// A right click on a block with the stick: a corner more, or with Shift one less.
pub(crate) fn clicked(
    player: &Arc<Player>,
    world: &Arc<World>,
    hand: InteractionHand,
    hit: &BlockHitResult,
) -> Option<InteractionResult> {
    if hand != InteractionHand::MainHand || !holds_wand(player) || !is_admin(player) {
        return None;
    }
    let bank = bank()?;
    let undo = player.is_crouching();
    let (x, z) = (hit.block_pos.x(), hit.block_pos.z());
    let corners = bank.zones.markup.edit(player.gameprofile.id(), |draft| {
        draft.world = world.key.to_string();
        if undo {
            draft.points.pop();
        } else {
            draft.points.push((x, z));
        }
        draft.points.len()
    });
    let text = if undo {
        TextComponent::plain(format!("Вершин: {corners} (убрана последняя)")).color(Color::Yellow)
    } else {
        TextComponent::plain(format!("Вершина {corners}: {x}, {z}")).color(Color::Green)
    };
    player.send_overlay_message(&text);
    Some(InteractionResult::Success)
}

/// A left click on a block with the stick: the block stays and the admin is told what the
/// draft holds.
pub(crate) fn struck(player: &Player) -> bool {
    if !holds_wand(player) || !is_admin(player) {
        return false;
    }
    let Some(bank) = bank() else {
        return false;
    };
    let draft = bank
        .zones
        .markup
        .draft(player.gameprofile.id())
        .unwrap_or_default();
    player.send_message(
        &TextComponent::plain(format!(
            "Черновик: {} вершин, высоты {}–{}. Сохранить: /zones save <номер> [название], \
             круг спавна: /zones circle <радиус>.",
            draft.points.len(),
            draft.y_min,
            draft.y_max
        ))
        .color(Color::Gray),
    );
    true
}

/// The color of a zone's border, `0xRRGGBB`.
#[must_use]
pub fn color(zone: &Zone) -> i32 {
    if zone.common {
        return 0x00FF_FFFF;
    }
    match zone.status.as_str() {
        "auction" => 0x00FF_B000,
        "leased" if zone.mine => 0x003A_D1C6,
        "leased" => 0x00E0_4848,
        _ => 0x0055_E05A,
    }
}

/// One polygon to draw.
#[derive(Debug, PartialEq, Eq)]
struct Shape {
    rgb: i32,
    /// The unsaved markup of the admin: its corners are drawn too, and it stays open while it
    /// has fewer than three.
    draft: bool,
    points: Vec<(i32, i32)>,
}

/// What is drawn for a player in `world`: with the stick all zones and the draft, else the
/// zones `/zones show` asked for.
fn shapes(
    zones: &[Zone],
    world: &str,
    wand: bool,
    show: Option<Show>,
    draft: Option<&Draft>,
) -> Vec<Shape> {
    let only = match show {
        Some(Show::One(num)) if !wand => Some(num),
        _ => None,
    };
    let mut shapes: Vec<Shape> = zones
        .iter()
        .filter(|zone| zone.world == world && only.is_none_or(|num| zone.num == num))
        .map(|zone| Shape {
            rgb: color(zone),
            draft: false,
            points: zone.points.clone(),
        })
        .collect();
    if let Some(draft) = draft.filter(|draft| wand && !draft.points.is_empty())
        && draft.world == world
    {
        shapes.push(Shape {
            rgb: DRAFT_COLOR,
            draft: true,
            points: draft.points.clone(),
        });
    }
    shapes
}

/// A particle every block along the edges of a shape, only near `(x, z)`.
fn outline(shape: &Shape, x: f64, z: f64) -> Vec<(f64, f64)> {
    let corners = shape.points.len();
    let closed = !shape.draft || corners >= 3;
    let edges = if closed {
        corners
    } else {
        corners.saturating_sub(1)
    };
    let mut dots = Vec::new();
    for edge in 0..edges {
        let (from, to) = (
            shape.points[edge],
            shape.points[(edge + 1) % corners.max(1)],
        );
        let (ax, az) = (f64::from(from.0) + 0.5, f64::from(from.1) + 0.5);
        let (bx, bz) = (f64::from(to.0) + 0.5, f64::from(to.1) + 0.5);
        let steps = (bx - ax).hypot(bz - az).ceil().max(1.0);
        let mut step = 0.0;
        while step <= steps {
            let (dot_x, dot_z) = (ax + (bx - ax) * step / steps, az + (bz - az) * step / steps);
            if (dot_x - x).powi(2) + (dot_z - z).powi(2) <= NEAR * NEAR {
                dots.push((dot_x, dot_z));
            }
            step += 1.0;
        }
    }
    dots
}

fn dust(world: &Arc<World>, player: &Player, rgb: i32, scale: f32, at: DVec3) {
    let particle = ParticleData::new(
        &vanilla_particle_types::DUST,
        DustParticleOptions::new(RgbColor::new(rgb), scale),
    );
    world.send_particles_to(player, particle, true, false, at, 1, DVec3::ZERO, 0.0);
}

impl Bank {
    /// Draws the borders for those who hold the stick or asked for them; twice a second.
    pub(super) fn tick_markup(&self, server: &Server, tick: u64) {
        let markup = &self.zones.markup;
        markup.tick.store(tick, Ordering::Relaxed);
        markup.showing.lock().retain(|_, (until, _)| *until >= tick);
        let zones = self.zones.all();
        for player in server.get_players() {
            let uuid = player.gameprofile.id();
            let wand = holds_wand(&player) && is_admin(&player);
            let show = markup.showing.lock().get(&uuid).map(|(_, show)| *show);
            if !wand && show.is_none() {
                continue;
            }
            let world = player.get_world();
            let draft = markup.draft(uuid).filter(|_| wand);
            let position = player.position();
            let height = position.y + 0.3;
            for shape in shapes(&zones, &world.key.to_string(), wand, show, draft.as_ref()) {
                for (x, z) in outline(&shape, position.x, position.z) {
                    dust(&world, &player, shape.rgb, 1.2, DVec3::new(x, height, z));
                }
                if !shape.draft {
                    continue;
                }
                for (x, z) in &shape.points {
                    for level in 0..4 {
                        let at = DVec3::new(
                            f64::from(*x) + 0.5,
                            height + f64::from(level) * 0.6,
                            f64::from(*z) + 0.5,
                        );
                        dust(&world, &player, CORNER_COLOR, 2.0, at);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(num: i32, status: &str, world: &str) -> Zone {
        let listed = json!({
            "id": format!("z-{num}"), "num": num, "kind": if num == 0 { "common" } else { "lot" },
            "world": world, "points": [[0, 0], [4, 0], [4, 4], [0, 4]], "y_min": 60,
            "y_max": 100, "start_price": 5, "status": status,
        });
        Zone::parse(&listed).unwrap_or_else(|| unreachable!())
    }

    #[test]
    fn the_stick_is_known_by_its_tag() {
        assert!(is_wand(&wand()));
        assert!(!is_wand(&ItemStack::new(&vanilla_items::BLAZE_ROD)));
        assert!(!is_wand(&ItemStack::new(&vanilla_items::STICK)));
    }

    #[test]
    fn a_circle_has_sixteen_to_sixty_four_corners() {
        let mut draft = Draft::default();
        draft.circle("minecraft:the_nether", 100, -50, 3);
        assert_eq!(draft.points.len(), 16);
        assert_eq!(draft.points[0], (103, -50));
        assert_eq!(draft.points[4], (100, -47));
        assert_eq!(draft.world, "minecraft:the_nether");
        draft.circle("minecraft:overworld", 0, 0, 20);
        assert_eq!(draft.points.len(), 40);
        draft.circle("minecraft:overworld", 0, 0, 500);
        assert_eq!(draft.points.len(), 64);
    }

    #[test]
    fn a_draft_is_saved_with_its_kind_and_title() {
        let mut draft = Draft::default();
        draft.take(&zone(3, "free", "minecraft:overworld"));
        assert_eq!(
            draft.body(3, false, None),
            json!({
                "num": 3, "kind": "lot", "world": "minecraft:overworld", "y_min": 60,
                "y_max": 100, "points": [[0, 0], [4, 0], [4, 4], [0, 4]],
            })
        );
        let common = draft.body(0, true, Some("Площадь"));
        assert_eq!(common["kind"], "common");
        assert_eq!(common["title"], "Площадь");
    }

    #[test]
    fn show_draws_one_zone_or_all_of_the_world() {
        let zones = [
            zone(0, "free", "minecraft:overworld"),
            zone(3, "auction", "minecraft:overworld"),
            zone(4, "leased", "minecraft:the_nether"),
        ];
        let world = "minecraft:overworld";
        assert_eq!(shapes(&zones, world, false, Some(Show::All), None).len(), 2);
        let one = shapes(&zones, world, false, Some(Show::One(3)), None);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].rgb, 0x00FF_B000);
        let draft = Draft {
            points: vec![(1, 1), (2, 2)],
            ..Draft::default()
        };
        // The stick shows every zone and the draft, whatever `/zones show` asked for.
        let with_wand = shapes(&zones, world, true, Some(Show::One(3)), Some(&draft));
        assert_eq!(with_wand.len(), 3);
        assert!(with_wand[2].draft);
        // A draft of another dimension is not drawn here.
        let nether = shapes(&zones, "minecraft:the_nether", true, None, Some(&draft));
        assert_eq!(nether.len(), 1);
    }

    #[test]
    fn an_outline_follows_the_edges_near_the_player() {
        let square = Shape {
            rgb: 0,
            draft: false,
            points: vec![(0, 0), (4, 0), (4, 4), (0, 4)],
        };
        // Four edges of four blocks, five dots each.
        assert_eq!(outline(&square, 2.0, 2.0).len(), 20);
        assert_eq!(outline(&square, 500.0, 2.0), []);
        let open = Shape {
            rgb: 0,
            draft: true,
            points: vec![(0, 0), (4, 0)],
        };
        // A draft of two corners is one edge, not a closed shape.
        assert_eq!(outline(&open, 2.0, 0.0).len(), 5);
        let dot = Shape {
            rgb: 0,
            draft: true,
            points: vec![(0, 0)],
        };
        assert_eq!(outline(&dot, 0.0, 0.0), []);
    }
}
