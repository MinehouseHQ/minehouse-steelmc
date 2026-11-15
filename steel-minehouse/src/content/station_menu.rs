//! The station map (a port of the Fabric `StationMenu`). A player sitting in a cart that
//! stands on a station sees the network in front of them, drawn with display entities only
//! they get: the lines as colored strips, the stations as dots with their names, forks
//! where the line color changes as white rings (transfers). The name of a station shows
//! only while the player looks near its dot (the current station is always signed). A
//! click anywhere on the map sends the cart to the station looked at, a held right button
//! drags the map, and the mouse wheel zooms it towards the point looked at: the client
//! sends a wheel turn as a hotbar slot change, and the slot it had is given back when the
//! map closes.

use std::{f32::consts::PI, sync::Arc};

use glam::{DVec3, Quat, Vec3};
use rustc_hash::FxHashMap;
use steel_core::{
    behavior::InteractionResult,
    entity::{Entity, next_entity_id},
    player::{Player, input_observer::PlayerInputObserver},
    world::World,
};
use steel_protocol::packets::game::{CAddEntity, CRemoveEntities, CSetEntityData, CSetHeldSlot};
use steel_registry::{
    RegistryEntry as _,
    blocks::BlockRef,
    dye_color::DyeColor,
    entity_data::{DataValue, Quaternionf, Vector3f},
    entity_type::EntityTypeRef,
    items::item::BlockHitResult,
    vanilla_blocks, vanilla_entities,
    vanilla_entity_data::{
        BlockDisplayEntityData, DisplayEntityData, InteractionEntityData, TextDisplayEntityData,
    },
};
use steel_utils::{BlockPos, BlockStateId, Identifier, locks::SyncMutex, types::InteractionHand};
use text_components::{Modifier as _, TextComponent, format::Color};
use uuid::Uuid;

use super::{
    anvil_repair,
    map_view::MapView,
    metro, nametags,
    rail_graph::{self, Edge},
    ride, stations, trains,
    transfers::{self, Track},
};

/// The size of the map on the panel, blocks.
const WIDTH: f32 = 2.6;
const HEIGHT: f32 = 1.6;
/// How far in front of the eyes the panel hangs.
const DISTANCE: f64 = 1.2;
/// The panel is laid out at 2.4 blocks and shrunk to [`DISTANCE`], so it looks the same
/// size but stays clear of walls.
const SHRINK: f32 = (DISTANCE / 2.4) as f32;
const LINE: f32 = 0.05;
const GRAY: u32 = 0x009E_9E9E;
const WHITE: u32 = 0x00FF_FFFF;
/// How close to a dot (panel units) the look has to land to pick that station.
const LOOK_RADIUS: f32 = 0.3;
/// Ticks a zoom step glides over.
const GLIDE: i32 = 2;
/// A held right button repeats every 4 ticks; this long without a repeat means it was let
/// go.
const RELEASE: i64 = 8;
/// A right press that moved the map less than this (panel units) was a click, not a drag.
const CLICK_SLOP: f32 = 0.06;
/// Vanilla `TextDisplay.FLAG_SHADOW`.
const FLAG_SHADOW: i8 = 1;
/// Vanilla `Brightness(15, 15).pack()`.
const FULL_BRIGHT: i32 = 15 << 4 | 15 << 20;

fn colored(text: &str, rgb: u32) -> TextComponent {
    let [_, red, green, blue] = rgb.to_be_bytes();
    TextComponent::plain(text.to_owned()).color(Color::Rgb(red, green, blue))
}

/// What a display entity shows.
enum Shown {
    Text(Box<TextDisplayEntityData>),
    Block(Box<BlockDisplayEntityData>),
}

/// A display entity one player has.
struct Element {
    id: i32,
    shown: Shown,
}

impl Element {
    fn glow(display: &mut DisplayEntityData) {
        display.brightness_override.set(FULL_BRIGHT);
        display.width.set(WIDTH + 1.0);
        display.height.set(HEIGHT + 1.0);
        display.view_range.set(0.5);
        display.transformation_interpolation_duration.set(GLIDE);
    }

    fn text(text: TextComponent) -> Self {
        let mut data = Box::new(TextDisplayEntityData::new());
        data.text.set(Box::new(text));
        data.background_color.set(0);
        data.style_flags.set(FLAG_SHADOW);
        data.line_width.set(400);
        Self::glow(&mut data.display);
        Self {
            id: next_entity_id(),
            shown: Shown::Text(data),
        }
    }

    fn block(state: BlockStateId) -> Self {
        let mut data = Box::new(BlockDisplayEntityData::new());
        data.block_state.set(state);
        Self::glow(&mut data.display);
        Self {
            id: next_entity_id(),
            shown: Shown::Block(data),
        }
    }

    const fn display(&mut self) -> &mut DisplayEntityData {
        match &mut self.shown {
            Shown::Text(data) => &mut data.display,
            Shown::Block(data) => &mut data.display,
        }
    }

    const fn kind(&self) -> EntityTypeRef {
        match self.shown {
            Shown::Text(_) => &vanilla_entities::TEXT_DISPLAY,
            Shown::Block(_) => &vanilla_entities::BLOCK_DISPLAY,
        }
    }

    fn set_text(&mut self, text: TextComponent) {
        if let Shown::Text(data) = &mut self.shown {
            data.text.set(Box::new(text));
        }
    }

    fn set_block(&mut self, state: BlockStateId) {
        if let Shown::Block(data) = &mut self.shown {
            data.block_state.set(state);
        }
    }

    fn transform(&mut self, translation: Vec3, rotation: Quat, scale: Vec3) {
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
    }

    fn packed(&mut self) -> Option<Vec<DataValue>> {
        match &mut self.shown {
            Shown::Text(data) => data.pack_dirty(),
            Shown::Block(data) => data.pack_dirty(),
        }
    }

    fn spawn(&mut self, player: &Player, at: DVec3) {
        player.send_packet(add_entity(self.id, self.kind(), at));
        if let Some(values) = self.packed() {
            player.send_packet(CSetEntityData::new(self.id, values));
        }
    }

    /// Sends what changed. A moved element glides to its new place.
    fn sync(&mut self, player: &Player) {
        let display = self.display();
        if display.translation.is_dirty()
            || display.scale.is_dirty()
            || display.left_rotation.is_dirty()
        {
            display
                .transformation_interpolation_start_delta_ticks
                .set_forced(0);
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

/// The panel: centered in front of the player's eyes, facing them; map x to the right,
/// north up.
struct Layout {
    center: DVec3,
    forward: DVec3,
    right: DVec3,
    /// From panel space (x right, y up, z towards the player) to the world's.
    rotation: Quat,
    view: MapView,
}

/// Where the player's eyes are. A rider is over the middle of the cart; the cart is asked,
/// because a cart just put in its place has not carried its rider there yet.
fn eyes(player: &Player) -> DVec3 {
    let position = player
        .vehicle()
        .map_or_else(|| player.position(), |cart| cart.position());
    DVec3::new(position.x, player.get_eye_y(), position.z)
}

impl Layout {
    fn new(player: &Player, view: MapView) -> Self {
        let yaw = player.rotation().0.to_radians();
        let forward = DVec3::new(-f64::from(yaw.sin()), 0.0, f64::from(yaw.cos()));
        Self {
            // Upright at eye level, whatever the pitch: looking up or down does not move
            // it.
            center: eyes(player) + forward * DISTANCE,
            forward,
            right: DVec3::new(-f64::from(yaw.cos()), 0.0, -f64::from(yaw.sin())),
            rotation: Quat::from_rotation_y(PI - yaw),
            view,
        }
    }

    /// Where on the panel the player's look lands, `None` if they look away from it.
    fn look_at(&self, player: &Player) -> Option<(f32, f32)> {
        let eye = eyes(player);
        let look = player.look_angle();
        let facing = look.dot(self.forward);
        if facing < 0.1 {
            return None;
        }
        let hit = eye + look * ((self.center - eye).dot(self.forward) / facing);
        let offset = hit - self.center;
        Some((
            (offset.dot(self.right) / f64::from(SHRINK)) as f32,
            (offset.y / f64::from(SHRINK)) as f32,
        ))
    }

    /// A panel point as an offset from the panel's middle in the world.
    fn at(&self, point: Vec3) -> Vec3 {
        self.rotation * point * SHRINK
    }
}

/// Something drawn at a world point: a dot, a ring, a name. Hidden while its point is off
/// the panel.
struct Mark {
    x: f64,
    z: f64,
    element: Element,
    dy: f32,
    depth: f32,
    size: f32,
    text: TextComponent,
    visible: bool,
}

impl Mark {
    fn new(pos: BlockPos, text: TextComponent, dy: f32, depth: f32, size: f32) -> Self {
        Self {
            x: f64::from(pos.x()),
            z: f64::from(pos.z()),
            element: Element::text(TextComponent::default()),
            dy,
            depth,
            size,
            text,
            visible: false,
        }
    }

    fn show(&mut self) {
        let text = if self.visible {
            self.text.clone()
        } else {
            TextComponent::default()
        };
        self.element.set_text(text);
    }

    fn place(&mut self, layout: &Layout) {
        let (u, v) = (layout.view.u(self.x), layout.view.v(self.z));
        // A dot may stick out a little past the edge rather than vanish while still half
        // on the panel.
        self.visible = layout.view.inside(u, v, 0.06);
        self.element.transform(
            layout.at(Vec3::new(u, v + self.dy, self.depth)),
            layout.rotation,
            Vec3::splat(SHRINK * self.size),
        );
        self.show();
    }

    fn set_text(&mut self, text: TextComponent) {
        self.text = text;
        self.show();
    }
}

/// A piece of line between two track points, cut to the panel's edge.
struct Strip {
    from: (f64, f64),
    to: (f64, f64),
    element: Element,
    state: BlockStateId,
}

impl Strip {
    fn place(&mut self, layout: &Layout) {
        let view = &layout.view;
        let cut = view.clip((
            view.u(self.from.0),
            view.v(self.from.1),
            view.u(self.to.0),
            view.v(self.to.1),
        ));
        let visible = cut.and_then(|(u1, v1, u2, v2)| {
            let (du, dv) = (u2 - u1, v2 - v1);
            let length = du.hypot(dv);
            (length >= 1.0e-4).then_some((u1, v1, du, dv, length))
        });
        let Some((u, v, du, dv, length)) = visible else {
            self.element.set_block(vanilla_blocks::AIR.default_state());
            return;
        };
        self.element.set_block(self.state);
        let turn = Quat::from_rotation_z(dv.atan2(du));
        let start = Vec3::new(u, v, -0.01) + turn * Vec3::new(-LINE / 2.0, -LINE / 2.0, 0.0);
        self.element.transform(
            layout.at(start),
            layout.rotation * turn,
            Vec3::new(length + LINE, LINE, 0.01) * SHRINK,
        );
    }
}

/// A station on the map: its dot and its name (shown when picked), as indexes of marks.
struct Station {
    rail: BlockPos,
    dot: usize,
    label: usize,
    signed: TextComponent,
}

/// What a player has in front of them.
struct Panel {
    layout: Layout,
    fixed: Vec<Element>,
    button: i32,
    strips: Vec<Strip>,
    marks: Vec<Mark>,
    stations: Vec<Station>,
    shown: Option<usize>,
    /// The right button held on the map: the tick of its last repeat.
    held: Option<i64>,
    grabbed: Option<(f32, f32)>,
    moved: f32,
}

impl Panel {
    fn place(&mut self) {
        for strip in &mut self.strips {
            strip.place(&self.layout);
        }
        for mark in &mut self.marks {
            mark.place(&self.layout);
        }
    }

    fn spawn(&mut self, player: &Player) {
        let center = self.layout.center;
        for element in self.elements() {
            element.spawn(player, center);
        }
        let click_height = (HEIGHT + 0.2) * SHRINK;
        let mut button = InteractionEntityData::new();
        button.width.set((WIDTH + 0.2) * SHRINK);
        button.height.set(click_height);
        player.send_packet(add_entity(
            self.button,
            &vanilla_entities::INTERACTION,
            center - DVec3::new(0.0, f64::from(click_height) / 2.0, 0.0),
        ));
        if let Some(values) = button.pack_dirty() {
            player.send_packet(CSetEntityData::new(self.button, values));
        }
    }

    fn elements(&mut self) -> impl Iterator<Item = &mut Element> {
        self.fixed
            .iter_mut()
            .chain(self.strips.iter_mut().map(|strip| &mut strip.element))
            .chain(self.marks.iter_mut().map(|mark| &mut mark.element))
    }

    fn sync(&mut self, player: &Player) {
        for element in self.elements() {
            element.sync(player);
        }
    }

    fn close(&mut self, player: &Player) {
        let mut entity_ids: Vec<i32> = self.elements().map(|element| element.id).collect();
        entity_ids.push(self.button);
        player.send_packet(CRemoveEntities { entity_ids });
    }

    /// The station under the look.
    fn pick(&self, at: Option<(f32, f32)>) -> Option<usize> {
        let (u, v) = at.filter(|&(u, v)| self.layout.view.inside(u, v, LOOK_RADIUS))?;
        let view = &self.layout.view;
        let mut best = None;
        let mut best_distance = LOOK_RADIUS * LOOK_RADIUS;
        for (index, station) in self.stations.iter().enumerate() {
            let Some(dot) = self.marks.get(station.dot).filter(|dot| dot.visible) else {
                continue;
            };
            let (du, dv) = (view.u(dot.x) - u, view.v(dot.z) - v);
            let distance = du * du + dv * dv;
            if distance <= best_distance {
                best_distance = distance;
                best = Some(index);
            }
        }
        best
    }

    /// Once a tick: drags the map while the button is held and signs the station under
    /// the look. Says whether a held button was let go without dragging: a click.
    fn look(&mut self, player: &Player, now: i64) -> bool {
        let at = self.layout.look_at(player);
        let mut clicked = false;
        if let Some(held) = self.held {
            if now - held > RELEASE {
                self.held = None;
                clicked = self.moved < CLICK_SLOP;
            } else if let (Some(at), Some(grabbed)) = (at, self.grabbed) {
                // The map follows the look, so the point grabbed stays under it.
                let (du, dv) = (at.0 - grabbed.0, at.1 - grabbed.1);
                self.moved += du.hypot(dv);
                if self.layout.view.pan(du, dv) {
                    self.place();
                }
            }
            self.grabbed = at;
        }
        let best = self.pick(at);
        if best != self.shown {
            let label = |panel: &Self, index: Option<usize>| {
                index
                    .and_then(|index| panel.stations.get(index))
                    .map(|station| (station.label, station.signed.clone()))
            };
            if let Some((mark, _)) = label(self, self.shown)
                && let Some(mark) = self.marks.get_mut(mark)
            {
                mark.set_text(TextComponent::default());
            }
            if let Some((mark, signed)) = label(self, best)
                && let Some(mark) = self.marks.get_mut(mark)
            {
                mark.set_text(signed);
            }
            self.shown = best;
        }
        self.sync(player);
        clicked
    }

    /// The right button pressed or still held on the map (it repeats while held).
    fn hold(&mut self, player: &Player, now: i64) {
        if self.held.is_none() {
            self.grabbed = self.layout.look_at(player);
            self.moved = 0.0;
        }
        self.held = Some(now);
    }

    /// A wheel turn: zooms towards where the player looks.
    fn zoom(&mut self, player: &Player, notches: i32) {
        let at = self.layout.look_at(player);
        if self.layout.view.zoom(notches, at) {
            self.place();
            self.sync(player);
        }
    }

    fn target(&self) -> Option<BlockPos> {
        self.shown
            .and_then(|index| self.stations.get(index))
            .map(|station| station.rail)
    }
}

/// The map a player has open, or the note that there is none to show at this station.
struct Open {
    cart: Uuid,
    rail: BlockPos,
    panel: Option<Panel>,
    slot: u8,
    seen: i64,
}

impl Open {
    /// Closes the map and gives back the hotbar slot the wheel moved.
    fn close(&mut self, player: &Player) {
        let Some(panel) = &mut self.panel else {
            return;
        };
        panel.close(player);
        let mut inventory = player.inventory.lock();
        if inventory.get_selected_slot() != self.slot {
            inventory.set_selected_slot(self.slot);
            drop(inventory);
            player.send_packet(CSetHeldSlot {
                slot: i32::from(self.slot),
            });
        }
    }
}

static MENUS: SyncMutex<Option<FxHashMap<Uuid, Open>>> = SyncMutex::new(None);

fn line_of(world: &World, edge: &Edge) -> Option<DyeColor> {
    stations::with(|stations| {
        let line = |rail: BlockPos| {
            stations
                .get(&world.key, rail.below())
                .and_then(|station| station.line)
        };
        let (from, to) = (line(edge.from), line(edge.to));
        match (from, to) {
            (Some(from), None) => Some(from),
            (Some(from), Some(to)) if from == to => Some(from),
            _ => to,
        }
    })
}

fn concrete(line: Option<DyeColor>) -> BlockRef {
    match line.unwrap_or(DyeColor::LightGray) {
        DyeColor::White => &vanilla_blocks::WHITE_CONCRETE,
        DyeColor::Orange => &vanilla_blocks::ORANGE_CONCRETE,
        DyeColor::Magenta => &vanilla_blocks::MAGENTA_CONCRETE,
        DyeColor::LightBlue => &vanilla_blocks::LIGHT_BLUE_CONCRETE,
        DyeColor::Yellow => &vanilla_blocks::YELLOW_CONCRETE,
        DyeColor::Lime => &vanilla_blocks::LIME_CONCRETE,
        DyeColor::Pink => &vanilla_blocks::PINK_CONCRETE,
        DyeColor::Gray => &vanilla_blocks::GRAY_CONCRETE,
        DyeColor::LightGray => &vanilla_blocks::LIGHT_GRAY_CONCRETE,
        DyeColor::Cyan => &vanilla_blocks::CYAN_CONCRETE,
        DyeColor::Purple => &vanilla_blocks::PURPLE_CONCRETE,
        DyeColor::Blue => &vanilla_blocks::BLUE_CONCRETE,
        DyeColor::Brown => &vanilla_blocks::BROWN_CONCRETE,
        DyeColor::Green => &vanilla_blocks::GREEN_CONCRETE,
        DyeColor::Red => &vanilla_blocks::RED_CONCRETE,
        DyeColor::Black => &vanilla_blocks::BLACK_CONCRETE,
    }
}

/// The lines of the network as strips, and as tracks for finding the transfers.
fn draw_lines(world: &World, edges: &[Edge]) -> (Vec<Strip>, Vec<Track>) {
    let mut strips = Vec::new();
    let mut tracks = Vec::new();
    for edge in edges {
        let line = line_of(world, edge);
        let state = concrete(line).default_state();
        for pair in edge.points.windows(2) {
            let [a, b] = pair else {
                continue;
            };
            if a.x() == b.x() && a.z() == b.z() {
                continue;
            }
            strips.push(Strip {
                from: (f64::from(a.x()), f64::from(a.z())),
                to: (f64::from(b.x()), f64::from(b.z())),
                element: Element::block(state),
                state,
            });
        }
        tracks.push(Track {
            line,
            points: edge.points.clone(),
        });
    }
    (strips, tracks)
}

/// The dot and the name of every station, the one the cart stands at first.
fn draw_stations(
    world: &World,
    rail: BlockPos,
    reachable: &[BlockPos],
    marks: &mut Vec<Mark>,
) -> Vec<Station> {
    let mut drawn = Vec::new();
    for &station in [rail].iter().chain(reachable) {
        let here = station == rail;
        let known = stations::with(|stations| stations.get(&world.key, station.below()).cloned())
            .unwrap_or_default();
        let rgb = known.line.map_or(WHITE, metro::line_rgb);
        let dot = marks.len();
        marks.push(Mark::new(
            station,
            colored("●", if here { WHITE } else { rgb }),
            -0.1,
            0.0,
            if here { 0.9 } else { 0.7 },
        ));

        let mut name = known
            .name
            .unwrap_or_else(|| format!("({}, {})", station.x(), station.z()));
        if here {
            name.push_str(" · вы здесь");
        }
        let signed = colored(&name, if here { 0x00FF_E066 } else { WHITE });
        // Other stations stay unsigned until looked at, so names never pile up on a busy
        // map. The current station is signed under its dot, so its longer label does not
        // run into a neighbor's.
        let label = marks.len();
        marks.push(Mark::new(
            station,
            if here {
                signed.clone()
            } else {
                TextComponent::default()
            },
            if here { -0.26 } else { 0.07 },
            0.012,
            0.3,
        ));
        if !here {
            drawn.push(Station {
                rail: station,
                dot,
                label,
                signed,
            });
        }
    }
    drawn
}

fn bounds(rail: BlockPos, edges: &[Edge]) -> ((f64, f64), (f64, f64)) {
    let (mut min_x, mut max_x, mut min_z, mut max_z) = (rail.x(), rail.x(), rail.z(), rail.z());
    for point in edges.iter().flat_map(|edge| &edge.points) {
        min_x = min_x.min(point.x());
        max_x = max_x.max(point.x());
        min_z = min_z.min(point.z());
        max_z = max_z.max(point.z());
    }
    (
        (f64::from(min_x), f64::from(max_x)),
        (f64::from(min_z), f64::from(max_z)),
    )
}

/// Draws the network around station rail `rail` for `player`. `None` where the network
/// knows no other station yet.
fn open(player: &Player, world: &World, rail: BlockPos) -> Option<Panel> {
    let (reachable, edges) = rail_graph::with(|graphs| {
        let graph = graphs.of(&world.key);
        let reachable: Vec<BlockPos> = graph
            .reachable_stations(rail, |pos| rail_graph::kind(world, pos))
            .into_iter()
            .map(|(station, _)| station)
            .collect();
        (reachable, graph.component(rail))
    });
    if reachable.is_empty() {
        player.send_overlay_message(&TextComponent::plain(
            "Схемы пока нет: линия появится, когда по ней проедет вагонетка".to_owned(),
        ));
        return None;
    }

    // At first the whole network fits on the panel (north up).
    let (across, down) = bounds(rail, &edges);
    let layout = Layout::new(player, MapView::new(across, down, WIDTH, HEIGHT));

    // No backing: lines and names float in the air in front of the player.
    let mut title = Element::text(TextComponent::plain(
        "Куда едем? Наведите взгляд на станцию и кликните".to_owned(),
    ));
    title.transform(
        layout.at(Vec3::new(0.0, HEIGHT / 2.0 + 0.28, 0.0)),
        layout.rotation,
        Vec3::splat(SHRINK * 0.4),
    );
    let mut hint = Element::text(colored(
        "Колёсико — масштаб, зажатая правая кнопка — двигать карту",
        GRAY,
    ));
    hint.transform(
        layout.at(Vec3::new(0.0, HEIGHT / 2.0 + 0.12, 0.0)),
        layout.rotation,
        Vec3::splat(SHRINK * 0.3),
    );

    let (strips, tracks) = draw_lines(world, &edges);

    // Forks where the line color changes: a white ring, as an interchange on a metro map.
    let station_rails: Vec<BlockPos> = edges
        .iter()
        .flat_map(|edge| [edge.from, edge.to])
        .filter(|node| *node == rail || reachable.contains(node))
        .collect();
    let mut marks = Vec::new();
    for transfer in transfers::find(&tracks, &station_rails) {
        marks.push(Mark::new(
            transfer.pos,
            colored("●", WHITE),
            -0.1 * 0.75 / 0.7,
            0.004,
            0.75,
        ));
        marks.push(Mark::new(
            transfer.pos,
            colored("●", 0x002A_2D33),
            -0.1 * 0.4 / 0.7,
            0.008,
            0.4,
        ));
    }
    let stations = draw_stations(world, rail, &reachable, &mut marks);

    // One click target over the whole map: a click sends the cart to the station the
    // player looks at, so nobody has to hit a tiny dot from a rocking cart.
    let mut panel = Panel {
        layout,
        fixed: vec![title, hint],
        button: next_entity_id(),
        strips,
        marks,
        stations,
        shown: None,
        held: None,
        grabbed: None,
        moved: 0.0,
    };
    panel.place();
    panel.spawn(player);
    Some(panel)
}

/// A click on the map: sends the cart to the station looked at.
fn click(
    player: &Player,
    cart: &dyn Entity,
    world: &Arc<World>,
    from: BlockPos,
    to: Option<BlockPos>,
) {
    let Some(to) = to else {
        player.send_overlay_message(&TextComponent::plain(
            "Наведите взгляд на станцию: её название появится, тогда нажимайте".to_owned(),
        ));
        return;
    };
    if ride::standing_at(cart, world) != Some(from) {
        return;
    }
    if !ride::send(cart, world, from, to) {
        player.send_overlay_message(&TextComponent::plain("Туда пока не проехать".to_owned()));
        return;
    }
    let name = stations::with(|stations| {
        stations
            .get(&world.key, to.below())
            .and_then(|station| station.name.clone())
    });
    let name = name.as_deref().unwrap_or("без названия");
    player.send_overlay_message(&TextComponent::plain(format!(
        "Едем до станции «{name}» без остановок"
    )));
    let open = MENUS
        .lock()
        .as_mut()
        .and_then(|menus| menus.remove(&player.uuid()));
    if let Some(mut open) = open {
        open.close(player);
    }
}

/// After a cart that drives itself moved: opens the map for whoever rides it (or its
/// train) at a station, keeps the open ones going, and closes the rest.
pub(super) fn tick(cart: &dyn Entity, world: &Arc<World>) {
    let rail = ride::standing_at(cart, world);
    let now = world.game_time();
    let mut clicks: Vec<(Uuid, Option<BlockPos>)> = Vec::new();
    {
        let mut menus = MENUS.lock();
        let menus = menus.get_or_insert_default();
        if rail.is_none() && menus.is_empty() {
            return;
        }
        ride::riders(cart, |player| {
            let id = player.uuid();
            if let Some(open) = menus.get_mut(&id)
                && Some(open.rail) != rail
            {
                open.close(player);
                menus.remove(&id);
            }
            let Some(rail) = rail else {
                return;
            };
            let open = menus.entry(id).or_insert_with(|| Open {
                cart: cart.uuid(),
                rail,
                slot: player.inventory.lock().get_selected_slot(),
                panel: open(player, world, rail),
                seen: now,
            });
            open.cart = cart.uuid();
            open.seen = now;
            if let Some(panel) = &mut open.panel
                && panel.look(player, now)
            {
                clicks.push((id, panel.target()));
            }
        });
        // Whoever got out of this cart.
        menus.retain(|id, open| {
            if open.cart != cart.uuid() || open.seen == now {
                return true;
            }
            if let Some(player) = world.players.get_by_uuid(id) {
                open.close(&player);
            }
            false
        });
    }
    let Some(rail) = rail else {
        return;
    };
    for (id, target) in clicks {
        if let Some(player) = world.players.get_by_uuid(&id) {
            click(&player, cart, world, rail, target);
        }
    }
}

/// A cart is gone: the maps open in it close.
pub(super) fn cart_removed(cart: &dyn Entity) {
    let mut menus = MENUS.lock();
    let Some(menus) = menus.as_mut() else {
        return;
    };
    let world = cart.level();
    menus.retain(|id, open| {
        if open.cart != cart.uuid() {
            return true;
        }
        if let Some(player) = world
            .as_ref()
            .and_then(|world| world.players.get_by_uuid(id))
        {
            open.close(&player);
        }
        false
    });
}

/// The cart whose map `player` has open, when entity `id` is the map's click target.
fn target_of(player: &Player, id: i32) -> Option<(BlockPos, Option<BlockPos>)> {
    let menus = MENUS.lock();
    let open = menus.as_ref()?.get(&player.uuid())?;
    let panel = open.panel.as_ref().filter(|panel| panel.button == id)?;
    Some((open.rail, panel.target()))
}

/// The clicks and the wheel of players with a map open.
pub struct Input;

/// The observer to install in `steel-core`.
pub static INPUT: Input = Input;

impl PlayerInputObserver for Input {
    fn interact(&self, player: &Player, entity_id: i32, _hand: InteractionHand) -> bool {
        let mut menus = MENUS.lock();
        let Some(panel) = menus
            .as_mut()
            .and_then(|menus| menus.get_mut(&player.uuid()))
            .and_then(|open| open.panel.as_mut())
            .filter(|panel| panel.button == entity_id)
        else {
            return false;
        };
        panel.hold(player, player.get_world().game_time());
        true
    }

    fn attack(&self, player: &Player, entity_id: i32) -> bool {
        let Some((rail, target)) = target_of(player, entity_id) else {
            return false;
        };
        if let Some(cart) = player.vehicle() {
            let cart = trains::driver_of(&*cart).unwrap_or(cart);
            click(player, &*cart, &player.get_world(), rail, target);
        }
        true
    }

    fn custom_click(&self, player: &Player, id: &Identifier) {
        nametags::clicked(player, id);
    }

    fn use_on_block(
        &self,
        player: &Arc<Player>,
        world: &Arc<World>,
        hand: InteractionHand,
        hit: &BlockHitResult,
    ) -> Option<InteractionResult> {
        anvil_repair::clicked(player, world, hand, hit)
    }

    fn carried_item(&self, player: &Player, from: u8, to: u8) {
        if from == to {
            return;
        }
        let mut menus = MENUS.lock();
        let Some(panel) = menus
            .as_mut()
            .and_then(|menus| menus.get_mut(&player.uuid()))
            .and_then(|open| open.panel.as_mut())
        else {
            return;
        };
        // Up the hotbar (the wheel away from you) is closer.
        let mut delta = (i32::from(to) - i32::from(from)).rem_euclid(9);
        if delta > 4 {
            delta -= 9;
        }
        panel.zoom(player, -delta);
    }
}
