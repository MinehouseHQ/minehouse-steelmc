//! «Карта зон спавна» (the Fabric `ZoneMap`): a vanilla filled map whose picture the server
//! draws itself and sends only as packets, with no saved map data behind it.
//!
//! The terrain of the loaded chunks, the borders of the zones in the colors of their status, a
//! banner with the number and the tenant over every zone and the marker of the holder. Sent
//! twice a second to those who hold the map; the terrain is read again every two minutes.

use std::{collections::BTreeMap, sync::Arc};

use chrono::{DateTime, Utc};
use simdnbt::owned::NbtCompound;
use steel_core::{
    chunk::heightmap::HeightmapType, entity::Entity as _, player::Player, server::Server,
    world::World,
};
use steel_protocol::packets::game::{CMapItemData, MapDecorationData, MapPatch};
use steel_registry::{
    RegistryReference,
    blocks::block_state_ext::BlockStateExt as _,
    data_components::{
        components::{CustomData, ItemLore, MapId},
        vanilla_components::{CUSTOM_DATA, ITEM_NAME, LORE, MAP_ID},
    },
    item_stack::ItemStack,
    map_color::{MapColor, MapColorBrightness},
    map_decoration_type::MapDecorationType,
    vanilla_items, vanilla_map_decoration_types,
};
use steel_utils::{BlockPos, ChunkPos, SectionPos, locks::SyncMutex, types::InteractionHand};
use text_components::{TextComponent, format::Color};
use uuid::Uuid;

use super::{Zone, left};
use crate::bank::{Bank, gui::line};

const KEY: &str = "minehouse_zone_map";
/// Far above the ids vanilla hands out, so no real map is overwritten on the client.
const ID: i32 = 1_900_000_000;
const SIZE: usize = 128;
/// The blocks left around the zones.
const MARGIN: i32 = 24;
/// The biggest scale of a vanilla map.
const MAX_SCALE: u8 = 4;
/// How often the terrain is read again.
const TERRAIN_TICKS: u64 = 20 * 120;
/// How deep water is looked through.
const MAX_DEPTH: i32 = 12;
const SNOW: MapColor = MapColor::from_id(8);
const RED: MapColor = MapColor::from_id(28);
const GOLD: MapColor = MapColor::from_id(30);
const EMERALD: MapColor = MapColor::from_id(33);

/// What part of the world the picture shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Frame {
    x: i32,
    z: i32,
    scale: u8,
}

impl Frame {
    /// The middle and the scale at which every zone fits with a margin.
    fn of(zones: &[Zone]) -> Self {
        let points = || zones.iter().flat_map(|zone| zone.points.iter().copied());
        let low = |pick: fn((i32, i32)) -> i32| points().map(pick).min().unwrap_or(0);
        let high = |pick: fn((i32, i32)) -> i32| points().map(pick).max().unwrap_or(0);
        let (min_x, max_x) = (low(|point| point.0), high(|point| point.0));
        let (min_z, max_z) = (low(|point| point.1), high(|point| point.1));
        let span = (max_x - min_x).max(max_z - min_z) + MARGIN;
        let mut scale = 0;
        while (128 << scale) < span && scale < MAX_SCALE {
            scale += 1;
        }
        Self {
            x: (min_x + max_x).div_euclid(2),
            z: (min_z + max_z).div_euclid(2),
            scale,
        }
    }

    /// The block of a pixel, by one axis.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "a pixel is below 128"
    )]
    const fn block(self, middle: i32, pixel: usize) -> i32 {
        middle + ((pixel as i32 - 64) << self.scale)
    }

    /// Where a position is on the map, by one axis: half pixels from the middle.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "clamped to the range of a byte"
    )]
    fn half_pixels(self, middle: i32, at: f64) -> i8 {
        let size = f64::from(1 << self.scale);
        let halves = ((at - f64::from(middle)) / size * 2.0 + 0.5).floor();
        halves.clamp(-128.0, 127.0) as i8
    }

    fn marker(
        self,
        kind: &'static MapDecorationType,
        x: f64,
        z: f64,
        yaw: f32,
        name: Option<String>,
    ) -> MapDecorationData {
        let turned = f64::from(yaw) + if yaw < 0.0 { -8.0 } else { 8.0 };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "vanilla keeps the low four bits of the rotation"
        )]
        let rot = ((turned * 16.0 / 360.0) as i32 & 15) as i8;
        MapDecorationData {
            decoration_type: RegistryReference::new(kind),
            x: self.half_pixels(self.x, x),
            y: self.half_pixels(self.z, z),
            rot,
            name: name.map(TextComponent::plain),
        }
    }
}

#[derive(Default)]
struct State {
    /// The zones the picture was drawn for.
    zones: Option<Arc<Vec<Zone>>>,
    frame: Frame,
    terrain: Vec<u8>,
    picture: Vec<u8>,
    version: u32,
    terrain_tick: u64,
    /// The version of the picture each holder has.
    sent: BTreeMap<Uuid, u32>,
}

/// The picture of the map and who has which version of it.
#[derive(Default)]
pub struct ZoneMap {
    state: SyncMutex<State>,
}

impl ZoneMap {
    /// The player gets the whole picture again.
    pub(super) fn forget(&self, player: Uuid) {
        self.state.lock().sent.remove(&player);
    }
}

/// The item «Карта зон спавна».
#[must_use]
pub fn item() -> ItemStack {
    let mut stack = ItemStack::new(&vanilla_items::FILLED_MAP);
    let mut tag = NbtCompound::new();
    tag.insert(KEY, 1_i8);
    if let Some(data) = CustomData::try_from_compound(tag) {
        stack.set(CUSTOM_DATA, data);
    }
    stack.set(MAP_ID, MapId::new(ID));
    stack.set(ITEM_NAME, TextComponent::plain("Карта зон спавна"));
    if let Ok(lore) = ItemLore::new(vec![
        line("Зоны острова спавна, хозяева и аукционы", Color::Gray),
        line("Обновляется, пока держишь в руке", Color::DarkGray),
    ]) {
        stack.set(LORE, lore);
    }
    stack
}

fn is_map(stack: &ItemStack) -> bool {
    stack.is(&vanilla_items::FILLED_MAP)
        && stack
            .get(CUSTOM_DATA)
            .is_some_and(|data| data.as_compound().byte(KEY) == Some(1))
}

fn holds_map(player: &Player) -> bool {
    let inventory = player.inventory.lock();
    is_map(inventory.get_item_in_hand(InteractionHand::MainHand))
        || is_map(inventory.get_item_in_hand(InteractionHand::OffHand))
}

/// Vanilla-like colors from the top block of every loaded column; unloaded ones stay blank.
fn terrain(world: &World, frame: Frame) -> Vec<u8> {
    let min_y = world.get_min_y();
    let mut colors = vec![0; SIZE * SIZE];
    // The height of the land in each pixel; none over water and unloaded chunks.
    let mut heights: Vec<Option<i32>> = vec![None; SIZE * SIZE];
    for pixel_z in 0..SIZE {
        for pixel_x in 0..SIZE {
            let (x, z) = (frame.block(frame.x, pixel_x), frame.block(frame.z, pixel_z));
            let chunk = ChunkPos::new(
                SectionPos::block_to_section_coord(x),
                SectionPos::block_to_section_coord(z),
            );
            let north = pixel_z
                .checked_sub(1)
                .and_then(|row| heights.get(row * SIZE + pixel_x).copied().flatten());
            let column = world.chunk_map.with_full_chunk(chunk, |chunk| {
                let local = |block: i32| usize::try_from(block & 15).unwrap_or(0);
                let mut y = chunk.get_height(HeightmapType::WorldSurface, local(x), local(z)) - 1;
                let mut state = chunk.get_block_state(BlockPos::new(x, y, z));
                let mut depth = 0;
                while !state.get_fluid_state().is_empty() && y > min_y && depth < MAX_DEPTH {
                    y -= 1;
                    depth += 1;
                    state = chunk.get_block_state(BlockPos::new(x, y, z));
                }
                if depth > 0 {
                    (MapColor::WATER, water_brightness(depth), None)
                } else {
                    (state.get_map_color(), land_brightness(y, north), Some(y))
                }
            });
            let Some((color, brightness, height)) = column else {
                continue;
            };
            let index = pixel_z * SIZE + pixel_x;
            if let Some(slot) = heights.get_mut(index) {
                *slot = height;
            }
            if color != MapColor::NONE
                && let Some(slot) = colors.get_mut(index)
            {
                *slot = color.packed_id(brightness);
            }
        }
    }
    colors
}

const fn water_brightness(depth: i32) -> MapColorBrightness {
    if depth < 2 {
        MapColorBrightness::High
    } else if depth < 5 {
        MapColorBrightness::Normal
    } else {
        MapColorBrightness::Low
    }
}

/// Land is lit from the north: a column above its northern neighbor is bright.
const fn land_brightness(y: i32, north: Option<i32>) -> MapColorBrightness {
    match north {
        Some(north) if y > north => MapColorBrightness::High,
        Some(north) if y < north => MapColorBrightness::Low,
        _ => MapColorBrightness::Normal,
    }
}

const fn status_color(zone: &Zone) -> MapColor {
    if zone.common {
        return SNOW;
    }
    match zone.status.as_bytes() {
        b"auction" => GOLD,
        b"leased" => RED,
        _ => EMERALD,
    }
}

/// Draws the borders of the zones: a pixel in a zone whose neighbor is not in it.
fn overlay(colors: &mut [u8], zones: &[Zone], frame: Frame) {
    let half = f64::from(1 << frame.scale) / 2.0;
    for zone in zones {
        let border = status_color(zone).packed_id(MapColorBrightness::High);
        let inside: Vec<bool> = (0..SIZE * SIZE)
            .map(|index| {
                let (pixel_x, pixel_z) = (index % SIZE, index / SIZE);
                zone.contains_xz(
                    f64::from(frame.block(frame.x, pixel_x)) + half,
                    f64::from(frame.block(frame.z, pixel_z)) + half,
                )
            })
            .collect();
        let is_in = |x: usize, z: usize| inside.get(z * SIZE + x).copied().unwrap_or(false);
        for pixel_z in 0..SIZE {
            for pixel_x in 0..SIZE {
                if !is_in(pixel_x, pixel_z) {
                    continue;
                }
                let edge = pixel_x == 0
                    || pixel_z == 0
                    || pixel_x == SIZE - 1
                    || pixel_z == SIZE - 1
                    || !is_in(pixel_x - 1, pixel_z)
                    || !is_in(pixel_x + 1, pixel_z)
                    || !is_in(pixel_x, pixel_z - 1)
                    || !is_in(pixel_x, pixel_z + 1);
                if edge && let Some(slot) = colors.get_mut(pixel_z * SIZE + pixel_x) {
                    *slot = border;
                }
            }
        }
    }
}

/// What the banner of a zone says.
fn label(zone: &Zone, now: DateTime<Utc>) -> String {
    let num = zone.num;
    if zone.common {
        return zone.name();
    }
    match &zone.auction {
        Some(auction) if zone.auction_soon(now) => {
            format!("{num} · аукцион через {}", left(auction.starts_at, now))
        }
        Some(auction) if zone.auction_open(now) => {
            let price = auction.top.unwrap_or(auction.start_price);
            format!("{num} · аукцион {price} АР")
        }
        _ if zone.leased() => {
            format!("{num} · {}", zone.owner.as_deref().unwrap_or("занята"))
        }
        _ => format!("{num} · свободна"),
    }
}

/// A banner over the middle of every zone.
fn banners(zones: &[Zone], frame: Frame, now: DateTime<Utc>) -> Vec<MapDecorationData> {
    zones
        .iter()
        .map(|zone| {
            let kind = if zone.common {
                &vanilla_map_decoration_types::BANNER_WHITE
            } else {
                match zone.status.as_str() {
                    "auction" => &vanilla_map_decoration_types::BANNER_YELLOW,
                    "leased" => &vanilla_map_decoration_types::BANNER_RED,
                    _ => &vanilla_map_decoration_types::BANNER_LIME,
                }
            };
            #[expect(
                clippy::cast_precision_loss,
                reason = "a zone has a handful of corners"
            )]
            let count = zone.points.len().max(1) as f64;
            let sum = |pick: fn(&(i32, i32)) -> i32| -> f64 {
                zone.points
                    .iter()
                    .map(|point| f64::from(pick(point)) + 0.5)
                    .sum()
            };
            let (x, z) = (sum(|point| point.0) / count, sum(|point| point.1) / count);
            frame.marker(kind, x, z, 180.0, Some(label(zone, now)))
        })
        .collect()
}

impl Bank {
    /// Sends the map to those who hold it; twice a second.
    pub(super) fn tick_zone_map(&self, server: &Server, tick: u64) {
        let holders: Vec<Arc<Player>> = server
            .get_players()
            .into_iter()
            .filter(|player| holds_map(player))
            .collect();
        let zones = self.zones.all();
        let Some(first) = zones.first() else {
            return;
        };
        if holders.is_empty() {
            return;
        }
        let Some(world) = server
            .worlds
            .values()
            .find(|world| world.key.to_string() == first.world)
        else {
            return;
        };
        let mut state = self.zones.map.state.lock();
        let state = &mut *state;
        let changed = state.zones.as_ref().is_none_or(|drawn| **drawn != *zones);
        if changed || tick.saturating_sub(state.terrain_tick) > TERRAIN_TICKS {
            state.frame = Frame::of(&zones);
            state.terrain = terrain(world, state.frame);
            state.terrain_tick = tick;
            state.picture.clone_from(&state.terrain);
            overlay(&mut state.picture, &zones, state.frame);
            state.zones = Some(Arc::clone(&zones));
            state.version = state.version.wrapping_add(1);
        }
        let banners = banners(&zones, state.frame, Utc::now());
        #[expect(clippy::cast_possible_wrap, reason = "a scale is at most 4")]
        let scale = state.frame.scale as i8;
        for player in holders {
            let mut decorations = banners.clone();
            if Arc::ptr_eq(&player.get_world(), world) {
                let position = player.position();
                decorations.push(state.frame.marker(
                    &vanilla_map_decoration_types::PLAYER,
                    position.x,
                    position.z,
                    player.rotation().0,
                    None,
                ));
            }
            let uuid = player.gameprofile.id();
            let color_patch = (state.sent.get(&uuid) != Some(&state.version)).then(|| MapPatch {
                start_x: 0,
                start_y: 0,
                width: 128,
                height: 128,
                colors: state.picture.clone(),
            });
            player.send_packet(CMapItemData {
                map_id: ID,
                scale,
                locked: true,
                decorations: Some(decorations),
                color_patch,
            });
            state.sent.insert(uuid, state.version);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-15T12:00:00Z")
            .map_or_default(|time| time.with_timezone(&Utc))
    }

    fn zone(num: i32, status: &str, from: i32, to: i32) -> Zone {
        let listed = json!({
            "id": format!("z-{num}"), "num": num, "kind": if num == 0 { "common" } else { "lot" },
            "world": "minecraft:overworld",
            "points": [[from, from], [to, from], [to, to], [from, to]],
            "y_min": 60, "y_max": 100, "start_price": 5, "status": status,
            "owner": (status == "leased").then_some("tenant"),
        });
        Zone::parse(&listed).unwrap_or_else(|| unreachable!())
    }

    #[test]
    fn the_map_is_known_by_its_tag() {
        assert!(is_map(&item()));
        assert!(!is_map(&ItemStack::new(&vanilla_items::FILLED_MAP)));
        assert_eq!(item().get(MAP_ID), Some(&MapId::new(ID)));
    }

    #[test]
    fn the_frame_holds_every_zone_with_a_margin() {
        let small = Frame::of(&[zone(1, "free", 0, 100)]);
        assert_eq!(
            small,
            Frame {
                x: 50,
                z: 50,
                scale: 0
            }
        );
        // 104 blocks and the margin no longer fit 128 pixels of one block.
        let wide = Frame::of(&[zone(1, "free", -10, 10), zone(2, "free", 60, 95)]);
        assert_eq!(
            wide,
            Frame {
                x: 42,
                z: 42,
                scale: 1
            }
        );
        assert_eq!(wide.block(wide.x, 64), 42);
        assert_eq!(wide.block(wide.x, 0), 42 - 128);
    }

    #[test]
    fn a_border_is_drawn_in_the_color_of_the_status() {
        let zones = [zone(1, "leased", 0, 10)];
        let frame = Frame {
            x: 5,
            z: 5,
            scale: 0,
        };
        let mut colors = vec![0; SIZE * SIZE];
        overlay(&mut colors, &zones, frame);
        let at = |x: i32, z: i32| {
            let pixel = |block: i32| usize::try_from(block - 5 + 64).unwrap_or(0);
            colors.get(pixel(z) * SIZE + pixel(x)).copied()
        };
        let red = RED.packed_id(MapColorBrightness::High);
        assert_eq!(at(0, 0), Some(red));
        assert_eq!(at(9, 4), Some(red));
        assert_eq!(at(4, 4), Some(0));
        assert_eq!(at(-1, 4), Some(0));
        assert_eq!(at(10, 4), Some(0));
    }

    #[test]
    fn a_banner_says_whose_the_zone_is() {
        let zones = [
            zone(0, "free", -20, 20),
            zone(1, "free", 30, 40),
            zone(2, "leased", 50, 60),
        ];
        let frame = Frame {
            x: 0,
            z: 0,
            scale: 1,
        };
        let banners = banners(&zones, frame, now());
        let names: Vec<Option<TextComponent>> =
            banners.iter().map(|banner| banner.name.clone()).collect();
        let plain = |text: &str| Some(TextComponent::plain(text.to_owned()));
        assert_eq!(
            names,
            [plain("Спавн"), plain("1 · свободна"), plain("2 · tenant")]
        );
        // The middle of zone 1 is 35.5 blocks away: 35.5 half pixels at two blocks a pixel.
        assert_eq!(
            banners
                .get(1)
                .map(|banner| (banner.x, banner.y, banner.rot)),
            Some((36, 36, 8))
        );
    }

    #[test]
    fn a_marker_stays_on_the_edge_of_the_map() {
        let frame = Frame {
            x: 0,
            z: 0,
            scale: 0,
        };
        let far = frame.marker(
            &vanilla_map_decoration_types::PLAYER,
            500.0,
            -500.0,
            -90.0,
            None,
        );
        assert_eq!((far.x, far.y, far.rot), (127, -128, 12));
    }

    #[test]
    fn water_gets_darker_with_depth_and_land_is_lit_from_the_north() {
        assert_eq!(water_brightness(1), MapColorBrightness::High);
        assert_eq!(water_brightness(4), MapColorBrightness::Normal);
        assert_eq!(water_brightness(9), MapColorBrightness::Low);
        assert_eq!(land_brightness(70, Some(69)), MapColorBrightness::High);
        assert_eq!(land_brightness(70, Some(71)), MapColorBrightness::Low);
        assert_eq!(land_brightness(70, None), MapColorBrightness::Normal);
    }
}
