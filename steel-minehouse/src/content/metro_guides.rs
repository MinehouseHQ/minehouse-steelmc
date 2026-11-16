//! The guides of the metro (the Fabric `MetroGuides`): the station, the junction, trains and
//! riding by the map. Sneak + right click in the air with a station or a junction opens its
//! guide; `/guide` lists all of them.
//!
//! The models are in cells: the floor at y 0, rails and carts at y 1, the track along x.

use std::f32::consts::FRAC_PI_2;

use glam::{Mat4, Vec3};
use steel_registry::{
    REGISTRY, blocks::BlockRef, dye_color::DyeColor, item_stack::ItemStack, sound_events,
    vanilla_blocks, vanilla_items,
};
use steel_utils::BlockStateId;
use text_components::{Modifier as _, TextComponent, format::Color};

use super::{
    JUNCTION, STATION,
    metro::{line_name, line_rgb},
};
use crate::guides::{
    Guide,
    stage::{Piece, Stage},
};

/// Ticks a cart takes for one cell in the models.
const CELL_TICKS: u32 = 5;
const HERE: u32 = 0x00FF_E066;
const BRASS: u32 = 0x00FF_C857;
const GREEN: u32 = 0x0055_FF55;
const WHITE: u32 = 0x00FF_FFFF;
const JUNCTION_MODEL: &str = "minehouse-tweaks:metro_junction";
/// The vanilla minecart as an item model of the same boxes and texture.
const MINECART_MODEL: &str = "minehouse-tweaks:guide/minecart";

/// The guides of the metro.
#[must_use]
pub fn all() -> Vec<Guide> {
    vec![
        station_guide(),
        junction_guide(),
        train_guide(),
        map_guide(),
    ]
}

// ---- the building blocks of the models -------------------------------------------------------

fn state(block: BlockRef, properties: &[(&str, &str)]) -> Option<BlockStateId> {
    REGISTRY
        .blocks
        .state_id_from_block_properties(block, properties)
}

fn rail(shape: &str) -> BlockStateId {
    let rail = &vanilla_blocks::RAIL;
    state(rail, &[("shape", shape)]).unwrap_or_else(|| rail.default_state())
}

fn powered(on: bool) -> BlockStateId {
    let rail = &vanilla_blocks::POWERED_RAIL;
    let on = if on { "true" } else { "false" };
    state(rail, &[("shape", "east_west"), ("powered", on)]).unwrap_or_else(|| rail.default_state())
}

fn lever(on: bool) -> BlockStateId {
    let lever = &vanilla_blocks::LEVER;
    let on = if on { "true" } else { "false" };
    state(
        lever,
        &[("face", "floor"), ("facing", "east"), ("powered", on)],
    )
    .unwrap_or_else(|| lever.default_state())
}

fn chain_state() -> BlockStateId {
    let chain = &vanilla_blocks::IRON_CHAIN;
    state(chain, &[("axis", "x")]).unwrap_or_else(|| chain.default_state())
}

fn floor_block(x: u8, z: u8) -> BlockStateId {
    if (x + z).is_multiple_of(2) {
        vanilla_blocks::SMOOTH_STONE.default_state()
    } else {
        vanilla_blocks::POLISHED_ANDESITE.default_state()
    }
}

fn at(cell: u8) -> f32 {
    f32::from(cell)
}

/// `cells` of the way of a cart, in ticks.
fn way(cells: u8) -> u32 {
    u32::from(cells) * CELL_TICKS
}

/// Ticks as a move takes them.
fn glide(ticks: u32) -> i32 {
    i32::try_from(ticks).unwrap_or(i32::MAX)
}

/// The checkered floor, waving in from the back left; the cells of `skip` stay empty.
fn floor(stage: &mut Stage, width: u8, depth: u8, skip: &'static [(u8, u8)]) {
    for x in 0..width {
        for z in 0..depth {
            if skip.contains(&(x, z)) {
                continue;
            }
            stage.later(u32::from(x + z), move |stage| {
                stage.block(
                    &format!("floor{x}_{z}"),
                    floor_block(x, z),
                    at(x),
                    0.0,
                    at(z),
                );
            });
        }
    }
}

/// Rails along x on a row, laid one by one; a powered one at `powered_at`.
fn rails(stage: &mut Stage, row: u8, width: u8, powered_at: Option<u8>) {
    for x in 0..width {
        let laid = if powered_at == Some(x) {
            powered(true)
        } else {
            rail("east_west")
        };
        stage.later(12 + u32::from(x) * 2, move |stage| {
            stage.block(&format!("rail{x}"), laid, at(x), 1.0, at(row));
            if x.is_multiple_of(2) {
                stage.sound(&sound_events::BLOCK_STONE_PLACE, 1.2);
            }
        });
    }
}

const fn dye_name(line: DyeColor) -> &'static str {
    match line {
        DyeColor::Red => "red",
        DyeColor::Blue => "blue",
        DyeColor::Green => "green",
        _ => "none",
    }
}

fn station_model(line: Option<DyeColor>) -> String {
    let line = line.map_or("none", dye_name);
    format!("minehouse-tweaks:guide/metro_station_{line}")
}

/// The station block at (x, 0, z), as the pack draws it for the line.
fn station(stage: &mut Stage, id: &str, line: Option<DyeColor>, x: u8, z: u8) {
    stage.model(id, &station_model(line), at(x), 0.0, at(z));
}

fn colored(text: &str, rgb: u32) -> TextComponent {
    let [_, red, green, blue] = rgb.to_be_bytes();
    TextComponent::plain(text.to_owned()).color(Color::Rgb(red, green, blue))
}

/// A name in the color of its line.
fn lined(name: &str, line: DyeColor) -> TextComponent {
    colored(name, line_rgb(line))
}

/// A minecart on the rail at (x, z), along x; `head` seats the viewer in it.
fn cart(stage: &mut Stage, id: &str, x: u8, z: u8, head: bool) {
    let mut pieces = vec![Piece::model(MINECART_MODEL)];
    if head {
        // Looking the way the cart goes (+x).
        let local = Mat4::from_translation(Vec3::new(0.5, 0.72, 0.5))
            * Mat4::from_rotation_y(-FRAC_PI_2)
            * Mat4::from_scale(Vec3::splat(0.6));
        pieces.push(Piece::fixed(stage.player_head(), local));
    }
    stage.group(id, at(x), 1.0, at(z), pieces);
}

/// A chain between the cart at x and the one at x + 1.
fn chain(stage: &mut Stage, id: &str, x: u8, z: u8) {
    stage.stretched(
        id,
        chain_state(),
        Vec3::new(at(x), 1.0, at(z)),
        Vec3::new(0.7, -0.12, 0.0),
        Vec3::new(0.6, 1.0, 1.0),
    );
}

/// A train of `count` carts `<prefix>0` (the front, at x + count - 1) to `<prefix>N`, coupled.
fn train(stage: &mut Stage, prefix: &str, x: u8, row: u8, count: u8) {
    for n in 0..count {
        let cell = x + count - 1 - n;
        cart(stage, &format!("{prefix}{n}"), cell, row, false);
        if n > 0 {
            chain(stage, &format!("{prefix}k{n}"), cell, row);
        }
    }
}

fn move_train(stage: &mut Stage, prefix: &str, count: u8, dx: f32, ticks: u32) {
    for n in 0..count {
        stage.move_by(&format!("{prefix}{n}"), dx, 0.0, glide(ticks));
        if n > 0 {
            stage.move_by(&format!("{prefix}k{n}"), dx, 0.0, glide(ticks));
        }
    }
}

fn remove_train(stage: &mut Stage, prefix: &str, count: u8) {
    for n in 0..count {
        stage.remove(&format!("{prefix}{n}"));
        stage.remove(&format!("{prefix}k{n}"));
    }
}

/// An item comes down onto the block at (x, 0, z) like a right click, then `effect`.
fn use_on(
    stage: &mut Stage,
    stack: ItemStack,
    x: u8,
    z: u8,
    effect: impl FnOnce(&mut Stage) + Send + 'static,
) {
    stage.item("hand", stack, at(x), 3.0, at(z), 0.5);
    stage.later(15, move |stage| {
        stage.move_to("hand", at(x), 1.4, at(z), 0.0, 10);
    });
    stage.later(27, move |stage| {
        stage.remove("hand");
        effect(stage);
    });
}

fn bell(stage: &Stage, pitch: f32) {
    stage.sound(&sound_events::BLOCK_NOTE_BLOCK_BELL, pitch);
}

// ---- the station -----------------------------------------------------------------------------

/// The station guide: 11 by 5, the track on row 2, the station at x 5.
mod st {
    pub const WIDTH: u8 = 11;
    pub const DEPTH: u8 = 5;
    pub const ROW: u8 = 2;
    pub const AT: u8 = 5;
    /// The last cell of the track.
    pub const END: u8 = WIDTH - 1;
}

fn station_guide() -> Guide {
    Guide::new("metro-station", "Станция метро", st::WIDTH, st::DEPTH)
        .opens_with(&STATION)
        .step(
            "Станция — это блок под рельсом там, где поезд должен остановиться. Под энергорельс с питанием или под обычный.",
            60,
            station_block,
        )
        .step(
            "ПКМ биркой с именем называет станцию. Бирка тратится, как на мобе.",
            50,
            station_named,
        )
        .step(
            "ПКМ красителем ставит станцию на линию этого цвета. Линии видно на сайте в разделах «Карта» и «Метро».",
            50,
            station_dyed,
        )
        .step(
            "Поезд останавливается на станции. С игроком он ждёт, пока тот выберет на схеме, куда ехать; пустой просто стоит.",
            130,
            station_stop,
        )
        .step(
            "Сами вагонетки никуда не едут: толкни любой вагон, и поедет весь состав. С конечной толкай назад, поедет обратно.",
            170,
            station_push,
        )
        .step(
            "Энергорельс станции без питания — это депо: поезд ждёт, пока рельсу не дадут ток рычагом или редстоуном.",
            200,
            station_depot,
        )
}

fn station_block(stage: &mut Stage) {
    floor(
        stage,
        st::WIDTH,
        st::DEPTH,
        &[(st::AT, st::ROW), (st::AT, st::ROW + 1)],
    );
    rails(stage, st::ROW, st::WIDTH, Some(st::AT));
    stage.later(30, |stage| {
        station(stage, "station", None, st::AT, st::ROW);
        stage.glow("station", BRASS);
        stage.pulse("station");
    });
    stage.later(32, |stage| {
        stage.sound(&sound_events::BLOCK_COPPER_PLACE, 1.0);
    });
}

fn station_named(stage: &mut Stage) {
    stage.glow("station", 0);
    let tag = ItemStack::new(&vanilla_items::NAME_TAG);
    use_on(stage, tag, st::AT, st::ROW, |stage| {
        bell(stage, 1.2);
        let name = TextComponent::plain("Пушкинская");
        stage.label("name", name, at(st::AT), 2.0, at(st::ROW));
    });
}

fn station_dyed(stage: &mut Stage) {
    let dye = ItemStack::new(&vanilla_items::RED_DYE);
    use_on(stage, dye, st::AT, st::ROW, |stage| {
        stage.sound(&sound_events::ITEM_DYE_USE, 1.0);
        stage.set_model("station", &station_model(Some(DyeColor::Red)));
        stage.pulse("station");
        let mut name = TextComponent::plain("Пушкинская · ");
        let line = format!("{} линия", line_name(DyeColor::Red));
        name.children.push(lined(&line, DyeColor::Red));
        stage.set_text("name", &name);
    });
}

/// The train of two comes from the left end to the station.
fn arrive(stage: &mut Stage) {
    move_train(stage, "t", 2, at(st::AT - 1), way(st::AT) - 5);
    stage.later(way(st::AT), |stage| bell(stage, 1.0));
}

fn station_stop(stage: &mut Stage) {
    train(stage, "t", 0, st::ROW, 2);
    stage.later(8, |stage| {
        arrive(stage);
        stage.later(way(st::AT), |stage| stage.pulse("name"));
        let leave = way(st::AT) + 50 + way(st::END - st::AT);
        stage.later(leave, |stage| remove_train(stage, "t", 2));
    });
}

fn station_push(stage: &mut Stage) {
    for x in st::AT + 2..st::WIDTH {
        stage.later(u32::from(x - st::AT - 2) * 2, move |stage| {
            stage.remove(&format!("rail{x}"));
        });
    }
    stage.later(20, |stage| train(stage, "t", 0, st::ROW, 2));
    stage.later(28, |stage| {
        arrive(stage);
        stage.later(way(st::AT) + 45, |stage| {
            move_train(stage, "t", 2, -at(st::AT - 1), way(st::AT));
        });
        stage.later(way(st::AT) * 2 + 45, |stage| {
            remove_train(stage, "t", 2);
        });
    });
}

fn station_depot(stage: &mut Stage) {
    for x in st::AT + 2..st::WIDTH {
        stage.later(u32::from(x - st::AT - 2) * 2, move |stage| {
            stage.block(
                &format!("rail{x}"),
                rail("east_west"),
                at(x),
                1.0,
                at(st::ROW),
            );
        });
    }
    let station_rail = format!("rail{}", st::AT);
    stage.set_state(&station_rail, powered(false));
    stage.pulse(&station_rail);
    stage.later(15, |stage| train(stage, "t", 0, st::ROW, 2));
    stage.later(23, move |stage| {
        arrive(stage);
        let stop = way(st::AT);
        stage.later(stop + 30, |stage| {
            stage.block("lever", lever(false), at(st::AT), 1.0, at(st::ROW + 1));
        });
        stage.later(stop + 55, move |stage| {
            stage.sound(&sound_events::BLOCK_LEVER_CLICK, 0.6);
            stage.set_state("lever", lever(true));
            stage.set_state(&station_rail, powered(true));
            stage.pulse(&station_rail);
        });
        let rest = st::END - st::AT;
        stage.later(stop + 70, move |stage| {
            move_train(stage, "t", 2, at(rest), way(rest));
        });
        stage.later(stop + 70 + way(rest), |stage| {
            remove_train(stage, "t", 2);
        });
    });
}

// ---- the junction ----------------------------------------------------------------------------

/// The junction guide: 11 by 6, the line on row 1, the branch goes south from x 5.
mod jn {
    pub const WIDTH: u8 = 11;
    pub const DEPTH: u8 = 6;
    pub const ROW: u8 = 1;
    pub const FORK: u8 = 5;
    pub const EAST: u8 = WIDTH - 1;
    pub const SOUTH: u8 = DEPTH - 1;
}

fn junction_guide() -> Guide {
    Guide::new("metro-junction", "Стрелка метро", jn::WIDTH, jn::DEPTH)
        .opens_with(&JUNCTION)
        .step(
            "Стрелка ставится под обычный рельс там, где от линии отходит ветка.",
            70,
            junction_block,
        )
        .step(
            "Едешь по схеме до станции — поезд сам переводит стрелку за 4 блока до неё и сворачивает куда нужно.",
            110,
            junction_turn,
        )
        .step(
            "Поезда без маршрута едут так, как лежит рельс. Его, как обычную развилку, можно переключить редстоуном.",
            120,
            junction_redstone,
        )
        .step(
            "Новую ветку надо один раз проехать: так мод узнаёт путь, и он появляется на схеме выбора станции.",
            90,
            junction_learned,
        )
}

fn junction_block(stage: &mut Stage) {
    floor(
        stage,
        jn::WIDTH,
        jn::DEPTH,
        &[
            (jn::EAST, jn::ROW),
            (jn::FORK, jn::SOUTH),
            (jn::FORK, jn::ROW),
        ],
    );
    rails(stage, jn::ROW, jn::WIDTH, None);
    for z in jn::ROW + 1..jn::DEPTH {
        stage.later(22 + u32::from(z) * 2, move |stage| {
            let rail = rail("north_south");
            stage.block(&format!("branch{z}"), rail, at(jn::FORK), 1.0, at(z));
        });
    }
    stage.later(30, |stage| {
        station(stage, "east", Some(DyeColor::Red), jn::EAST, jn::ROW);
        station(stage, "south", Some(DyeColor::Blue), jn::FORK, jn::SOUTH);
        let east = lined("Восточная", DyeColor::Red);
        stage.label("east-name", east, at(jn::EAST), 1.8, at(jn::ROW));
        let south = lined("Южная", DyeColor::Blue);
        stage.label("south-name", south, at(jn::FORK), 1.8, at(jn::SOUTH));
    });
    stage.later(45, |stage| {
        stage.model("junction", JUNCTION_MODEL, at(jn::FORK), 0.0, at(jn::ROW));
        stage.glow("junction", BRASS);
        stage.pulse("junction");
        stage.sound(&sound_events::BLOCK_COPPER_PLACE, 1.0);
    });
}

fn fork_rail() -> String {
    format!("rail{}", jn::FORK)
}

fn junction_turn(stage: &mut Stage) {
    stage.glow("junction", 0);
    cart(stage, "c", 0, jn::ROW, true);
    stage.later(8, |stage| {
        stage.move_by("c", 1.0, 0.0, glide(CELL_TICKS));
        stage.later(CELL_TICKS, |stage| {
            stage.sound(&sound_events::BLOCK_LEVER_CLICK, 1.2);
            stage.set_state(&fork_rail(), rail("south_west"));
            stage.pulse(&fork_rail());
            stage.pulse("junction");
        });
        stage.later(CELL_TICKS, |stage| {
            stage.move_by("c", at(jn::FORK - 1), 0.0, glide(way(jn::FORK - 1)));
        });
        stage.later(way(jn::FORK), |stage| {
            let (x, z) = (at(jn::FORK), at(jn::ROW + 1));
            stage.move_to("c", x, 1.0, z, FRAC_PI_2, glide(CELL_TICKS));
        });
        let down = jn::SOUTH - 1 - jn::ROW;
        stage.later(way(jn::FORK + 1), move |stage| {
            stage.move_by("c", 0.0, at(down), glide(way(down)));
        });
        let arrive = way(jn::FORK + jn::SOUTH - jn::ROW);
        stage.later(arrive, |stage| {
            bell(stage, 1.0);
            stage.pulse("south-name");
        });
        stage.later(arrive + 30, |stage| stage.remove("c"));
    });
}

fn junction_redstone(stage: &mut Stage) {
    let torch = vanilla_blocks::REDSTONE_TORCH.default_state();
    stage.block("torch", torch, at(jn::FORK + 1), 1.0, at(jn::ROW - 1));
    stage.later(10, |stage| {
        stage.sound(&sound_events::BLOCK_LEVER_CLICK, 1.0);
        stage.set_state(&fork_rail(), rail("east_west"));
        stage.pulse(&fork_rail());
    });
    stage.later(20, |stage| cart(stage, "c", 0, jn::ROW, false));
    stage.later(28, |stage| {
        stage.move_by("c", at(jn::EAST), 0.0, glide(way(jn::EAST)));
        stage.later(way(jn::EAST), |stage| {
            bell(stage, 1.0);
            stage.pulse("east-name");
        });
        stage.later(way(jn::EAST) + 30, |stage| stage.remove("c"));
    });
}

fn junction_learned(stage: &mut Stage) {
    stage.remove("torch");
    for z in jn::ROW + 1..jn::DEPTH {
        stage.later(u32::from(z - jn::ROW) * 8, move |stage| {
            let id = format!("branch{z}");
            stage.glow(&id, GREEN);
            stage.pulse(&id);
        });
    }
    stage.later(u32::from(jn::DEPTH - jn::ROW) * 8, |stage| {
        bell(stage, 1.4);
        let learned = colored("✓ ветка на схеме", GREEN);
        stage.label("learned", learned, at(jn::FORK + 2), 1.2, at(jn::DEPTH - 2));
    });
}

// ---- the train -------------------------------------------------------------------------------

/// The train guide: 9 by 4, the track on row 2.
mod tr {
    pub const WIDTH: u8 = 9;
    pub const DEPTH: u8 = 4;
    pub const ROW: u8 = 2;
}

fn train_guide() -> Guide {
    Guide::new("metro-train", "Поезд из вагонеток", tr::WIDTH, tr::DEPTH)
        .step(
            "Поезд — это вагонетки, сцепленные цепью. Поставь их на рельсы друг за другом.",
            50,
            train_carts,
        )
        .step(
            "Возьми цепь, присядь (Shift) и нажми ПКМ по вагонетке, потом по соседней.",
            90,
            train_coupling,
        )
        .step(
            "Сцепленные вагонетки едут вместе, как один поезд. В составе может быть до 12 вагонеток.",
            80,
            train_ride,
        )
        .step(
            "Shift + ПКМ ножницами по вагонетке расцепляет её, цепь выпадает.",
            70,
            train_shears,
        )
}

fn train_carts(stage: &mut Stage) {
    floor(stage, tr::WIDTH, tr::DEPTH, &[]);
    rails(stage, tr::ROW, tr::WIDTH, None);
    // The front cart `t0` is the rightmost one.
    for n in 0..3_u8 {
        stage.later(25 + u32::from(n) * 6, move |stage| {
            cart(stage, &format!("t{}", 2 - n), 2 + n, tr::ROW, false);
        });
    }
}

fn train_coupling(stage: &mut Stage) {
    let row = at(tr::ROW);
    let hand = ItemStack::new(&vanilla_items::IRON_CHAIN);
    stage.item("hand", hand, 2.0, 2.4, row, 0.5);
    stage.later(15, move |stage| {
        stage.move_to("hand", 3.0, 2.4, row, 0.0, 8);
    });
    stage.later(25, |stage| {
        chain(stage, "tk2", 2, tr::ROW);
        stage.sound(&sound_events::BLOCK_CHAIN_PLACE, 1.0);
    });
    stage.later(45, move |stage| {
        stage.move_to("hand", 4.0, 2.4, row, 0.0, 8);
    });
    stage.later(55, |stage| {
        chain(stage, "tk1", 3, tr::ROW);
        stage.sound(&sound_events::BLOCK_CHAIN_PLACE, 1.0);
    });
    stage.later(70, |stage| stage.remove("hand"));
}

fn train_ride(stage: &mut Stage) {
    move_train(stage, "t", 3, 3.0, way(3));
    stage.later(way(3) + 10, |stage| {
        move_train(stage, "t", 3, -3.0, way(3));
    });
}

fn train_shears(stage: &mut Stage) {
    let side = at(tr::ROW + 1);
    let shears = ItemStack::new(&vanilla_items::SHEARS);
    stage.item("hand", shears, 3.5, 2.4, at(tr::ROW), 0.5);
    stage.later(20, move |stage| {
        stage.sound(&sound_events::ENTITY_SHEEP_SHEAR, 1.0);
        stage.remove("tk1");
        let drop = ItemStack::new(&vanilla_items::IRON_CHAIN);
        stage.item("drop", drop, 3.5, 1.6, side, 0.35);
    });
    stage.later(30, |stage| stage.remove("hand"));
    stage.later(30, move |stage| {
        stage.move_to("drop", 3.5, 1.1, side, 0.0, 6);
    });
    stage.later(50, |stage| {
        stage.sound(&sound_events::ENTITY_ITEM_PICKUP, 1.0);
        stage.remove("drop");
    });
}

// ---- riding by the map -----------------------------------------------------------------------

/// The map guide: 11 by 5, the track on row 2, three stations.
mod mp {
    pub const WIDTH: u8 = 11;
    pub const DEPTH: u8 = 5;
    pub const ROW: u8 = 2;
    pub const AT: [u8; 3] = [1, 5, 9];
    pub const NAMES: [&str; 3] = ["Северная", "Центральная", "Парковая"];
}

fn map_guide() -> Guide {
    Guide::new("metro-map", "Поездка по схеме", mp::WIDTH, mp::DEPTH)
        .step(
            "Поставь вагонетку на станцию и сядь в неё: перед тобой откроется схема линий.",
            80,
            map_opens,
        )
        .step(
            "Нажми на станцию на схеме (ПКМ или ЛКМ), и вагонетка поедет туда без остановок.",
            120,
            map_ride,
        )
        .step(
            "На схеме только пути, по которым уже проехала вагонетка. Сломанная станция пропадает со схемы.",
            60,
            map_broken,
        )
}

/// The map as the rider sees it, shortened: one station a line, the chosen one highlighted.
fn scheme(chosen: Option<usize>) -> TextComponent {
    let mut text = TextComponent::plain("");
    for (n, name) in mp::NAMES.into_iter().enumerate() {
        let color = if n == 0 {
            WHITE
        } else if chosen == Some(n) {
            HERE
        } else {
            line_rgb(DyeColor::Green)
        };
        let line = format!(
            "{}● {name}{}",
            if n > 0 { "\n" } else { "" },
            if n == 0 { " · вы здесь" } else { "" }
        );
        text.children
            .push(colored(&line, color).bold(chosen == Some(n)));
    }
    text
}

fn map_opens(stage: &mut Stage) {
    floor(
        stage,
        mp::WIDTH,
        mp::DEPTH,
        &[
            (mp::AT[0], mp::ROW),
            (mp::AT[1], mp::ROW),
            (mp::AT[2], mp::ROW),
        ],
    );
    rails(stage, mp::ROW, mp::WIDTH, None);
    stage.later(25, |stage| {
        for (n, (x, name)) in mp::AT.into_iter().zip(mp::NAMES).enumerate() {
            station(stage, &format!("st{n}"), Some(DyeColor::Green), x, mp::ROW);
            let name = lined(name, DyeColor::Green);
            stage.label(&format!("name{n}"), name, at(x), 1.9, at(mp::ROW));
        }
    });
    stage.later(40, |stage| cart(stage, "c", mp::AT[0], mp::ROW, true));
    stage.later(60, |stage| {
        let x = at(mp::AT[0]) + 1.5;
        stage.label("scheme", scheme(None), x, 2.6, at(mp::ROW + 1));
        stage.pulse("scheme");
    });
}

fn map_ride(stage: &mut Stage) {
    stage.set_text("scheme", &scheme(Some(2)));
    stage.pulse("scheme");
    stage.sound(&sound_events::UI_BUTTON_CLICK, 1.0);
    stage.later(20, |stage| stage.remove("scheme"));
    let cells = mp::AT[2] - mp::AT[0];
    stage.later(25, move |stage| {
        stage.move_by("c", at(cells), 0.0, glide(way(cells)));
    });
    stage.later(25 + way(cells), |stage| {
        bell(stage, 1.0);
        let arrived = format!("{} · вы приехали", mp::NAMES[2]);
        stage.set_text("name2", &lined(&arrived, DyeColor::Green));
        stage.pulse("name2");
    });
}

fn map_broken(stage: &mut Stage) {
    stage.remove("st1");
    stage.remove("name1");
    stage.later(8, |stage| {
        let (x, z) = (mp::AT[1], mp::ROW);
        stage.block(
            &format!("floor{x}_{z}"),
            floor_block(x, z),
            at(x),
            0.0,
            at(z),
        );
    });
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use super::*;
    use crate::content::trains::MAX_CARTS;

    #[test]
    fn the_blocks_of_the_models_are_states_of_the_game() {
        init_vanilla_registry();
        for shape in ["east_west", "north_south", "south_west"] {
            assert!(state(&vanilla_blocks::RAIL, &[("shape", shape)]).is_some());
        }
        assert_ne!(powered(true), powered(false));
        assert_ne!(lever(true), lever(false));
        assert_ne!(lever(false), vanilla_blocks::LEVER.default_state());
        assert!(state(&vanilla_blocks::IRON_CHAIN, &[("axis", "x")]).is_some());
    }

    #[test]
    fn the_guide_of_the_train_tells_how_long_a_train_may_be() {
        let guide = train_guide();
        let told = format!("до {MAX_CARTS} вагонеток");
        assert!(guide.steps.iter().any(|step| step.caption.contains(&told)));
    }

    #[test]
    fn the_scheme_marks_the_chosen_station() {
        assert_ne!(
            format!("{:?}", scheme(None)),
            format!("{:?}", scheme(Some(2)))
        );
        assert_eq!(scheme(None).children.len(), 3);
    }
}
