//! The guides about the spawn zones (the Fabric `ZoneGuides`): what the zones are and where to
//! look at them, and the auction and the lease. No item opens them (the zone map is an ordinary
//! filled map), so they open from `/guide`, the guide book and the buttons of `/zones`; they
//! are listed while the zones are open.
//!
//! The island: 7 by 7 cells of grass, the floor at y 0. The shared zone is the middle 3 by 3,
//! eight lots lie around it; each zone is a stretched carpet colored like on the zone map
//! (white shared, lime free, yellow auction, red leased). The auction guide shows one lot,
//! 7 by 3, the lot in the middle.

use std::sync::Arc;

use glam::Vec3;
use steel_core::player::Player;
use steel_registry::{
    blocks::BlockRef, item_stack::ItemStack, sound_events, vanilla_blocks, vanilla_items,
};
use text_components::{Modifier as _, TextComponent, format::Color};

use crate::{
    bank::bank,
    guides::{self, Guide, stage::Stage},
};

/// The id of the guide about the zones.
pub const ZONES: &str = "spawn-zones";
/// The id of the guide about the auction and the lease.
pub const AUCTION: &str = "spawn-zones-auction";

const AQUA: u32 = 0x004F_E0D4;
const GOLD: u32 = 0x00F5_D33D;
const GREEN: u32 = 0x0055_FF55;
const RED: u32 = 0x00FF_5555;
const WHITE: u32 = 0x00FF_FFFF;

/// A zone of the model: x, z, width and depth in cells.
type Rect = [u8; 4];

/// The lots 1 to 8. Lot 6 is the front middle one, nearest the viewer.
const LOTS: [Rect; 8] = [
    [0, 0, 2, 2],
    [2, 0, 3, 2],
    [5, 0, 2, 2],
    [5, 2, 2, 3],
    [5, 5, 2, 2],
    [2, 5, 3, 2],
    [0, 5, 2, 2],
    [0, 2, 2, 3],
];
const COMMON: Rect = [2, 2, 3, 3];
const FRONT_LOT: usize = 6;
/// The lot of the auction guide.
const LOT: Rect = [1, 0, 5, 3];

/// The guides of the spawn zones.
#[must_use]
pub fn all() -> Vec<Guide> {
    vec![zones(), auction()]
}

fn open() -> bool {
    bank().is_some_and(|bank| bank.zones_open())
}

/// Opens a guide from the `/zones` window.
pub fn show(player: &Arc<Player>, id: &str) {
    if let Some(guide) = guides::by_id(id) {
        guides::open(player, guide);
    }
}

// ---- helpers ---------------------------------------------------------------------------------

fn colored(text: &str, rgb: u32) -> TextComponent {
    let [_, red, green, blue] = rgb.to_be_bytes();
    TextComponent::plain(text.to_owned()).color(Color::Rgb(red, green, blue))
}

fn plain(text: &str) -> TextComponent {
    TextComponent::plain(text.to_owned())
}

fn lot_id(num: usize) -> String {
    format!("lot{num}")
}

fn lot(num: usize) -> Rect {
    LOTS.get(num.wrapping_sub(1)).copied().unwrap_or(COMMON)
}

fn grass(stage: &mut Stage, width: u8, depth: u8) {
    for x in 0..width {
        for z in 0..depth {
            stage.later(u32::from(x + z) * 2, move |stage| {
                stage.block(
                    &format!("ground{x}_{z}"),
                    vanilla_blocks::GRASS_BLOCK.default_state(),
                    f32::from(x),
                    0.0,
                    f32::from(z),
                );
            });
        }
    }
}

/// A zone: one carpet stretched over its cells, a little inset so neighbors show a seam.
fn zone(stage: &mut Stage, id: &str, rect: Rect, carpet: BlockRef) {
    let [x, z, width, depth] = rect.map(f32::from);
    stage.stretched(
        id,
        carpet.default_state(),
        Vec3::new(x, 1.0, z),
        Vec3::new(0.06, 0.0, 0.06),
        Vec3::new(width - 0.12, 1.0, depth - 0.12),
    );
}

/// Recolors a zone and swells it.
fn recolor(stage: &mut Stage, id: &str, carpet: BlockRef) {
    stage.set_state(id, carpet.default_state());
    stage.pulse(id);
}

fn centre_x(rect: Rect) -> f32 {
    f32::from(rect[0]) + (f32::from(rect[2]) - 1.0) / 2.0
}

fn centre_z(rect: Rect) -> f32 {
    f32::from(rect[1]) + (f32::from(rect[3]) - 1.0) / 2.0
}

/// A label over the middle of a zone.
fn tag(stage: &mut Stage, id: &str, text: TextComponent, rect: Rect, y: f32) {
    stage.label(id, text, centre_x(rect), y, centre_z(rect));
}

// ---- the zones -------------------------------------------------------------------------------

fn zones() -> Guide {
    Guide::new(ZONES, "Зоны спавна", 7, 7)
        .visible_when(open)
        .step(
            "Остров спавна поделён на зоны. В центре общая зона: она принадлежит всем и не продаётся.",
            90,
            island,
        )
        .step(
            "Вокруг лежат лоты. Их разыгрывают на аукционе и сдают в аренду на 7 дней за АР с личного счёта в банке.",
            110,
            lots,
        )
        .step(
            "Цвет показывает, что с зоной: зелёная свободна, жёлтая на аукционе, красная арендована. Так же зоны раскрашены на карте и на сайте.",
            120,
            colors,
        )
        .step(
            "Зоны смотри в /zones или «Меню сервера» → «Зоны спавна»: список, продление, граница зоны. То же на сайте minehouse.pro/zones и на карте сайта.",
            120,
            where_to_look,
        )
        .step(
            "В /zones кнопка «Карта зон» даёт карту острова с хозяевами и аукционами, а «Показать границы» подсвечивает границы на 30 секунд.",
            130,
            map_and_borders,
        )
        .step(
            "Входишь в зону, и над хотбаром видно её название и кто её арендует.",
            100,
            walking_in,
        )
        .step(
            "Зона — не приват: блоки в ней ничем не защищены. Но ломать чужие постройки — гриф, а строить на чужой зоне можно только с разрешения хозяина.",
            150,
            not_a_claim,
        )
}

fn island(stage: &mut Stage) {
    grass(stage, 7, 7);
    stage.later(35, |stage| {
        zone(stage, "common", COMMON, &vanilla_blocks::WHITE_CARPET);
        stage.pulse("common");
        stage.sound(&sound_events::BLOCK_WOOL_PLACE, 1.0);
    });
    stage.later(50, |stage| {
        stage.label("common_label", plain("Общая зона"), 3.0, 1.6, 3.0);
        stage.pulse("common_label");
    });
}

fn lots(stage: &mut Stage) {
    stage.remove("common_label");
    for (index, rect) in (0_u8..).zip(LOTS) {
        let num = usize::from(index) + 1;
        stage.later(5 + u32::from(index) * 6, move |stage| {
            zone(stage, &lot_id(num), rect, &vanilla_blocks::LIME_CARPET);
            tag(
                stage,
                &format!("num{num}"),
                plain(&num.to_string()),
                rect,
                1.3,
            );
            let pitch = f32::from(index + 1).mul_add(0.08, 0.8);
            stage.sound(&sound_events::BLOCK_WOOL_PLACE, pitch);
        });
    }
}

fn colors(stage: &mut Stage) {
    stage.later(10, |stage| {
        recolor(stage, "lot2", &vanilla_blocks::YELLOW_CARPET);
        recolor(stage, "lot5", &vanilla_blocks::YELLOW_CARPET);
        tag(stage, "tag_auction", colored("Аукцион", GOLD), lot(2), 2.1);
        stage.sound(&sound_events::BLOCK_NOTE_BLOCK_BELL, 1.2);
    });
    stage.later(45, |stage| {
        for num in [1, 4, 8] {
            recolor(stage, &lot_id(num), &vanilla_blocks::RED_CARPET);
        }
        tag(stage, "tag_leased", colored("Steve", RED), lot(4), 2.1);
        stage.sound(&sound_events::BLOCK_NOTE_BLOCK_BELL, 0.9);
    });
}

fn where_to_look(stage: &mut Stage) {
    stage.remove("tag_auction");
    stage.remove("tag_leased");
    for num in 1..=LOTS.len() {
        stage.remove(&format!("num{num}"));
    }
    stage.later(10, |stage| {
        stage.label("cmd", colored("/zones", AQUA), 3.0, 2.6, 3.0);
        stage.pulse("cmd");
        stage.sound(&sound_events::UI_BUTTON_CLICK, 1.0);
    });
    stage.later(50, |stage| {
        stage.label("site", plain("minehouse.pro/zones"), 3.0, 1.9, 3.0);
    });
}

fn map_and_borders(stage: &mut Stage) {
    stage.remove("cmd");
    stage.remove("site");
    stage.later(10, |stage| {
        let map = ItemStack::new(&vanilla_items::FILLED_MAP);
        stage.item("map", map, 3.0, 2.2, 3.0, 1.1);
        stage.pulse("map");
        stage.sound(&sound_events::ITEM_BOOK_PAGE_TURN, 1.0);
    });
    stage.later(60, |stage| {
        stage.remove("map");
        stage.glow("common", WHITE);
        for num in 1..=LOTS.len() {
            let color = match num {
                2 | 5 => GOLD,
                1 | 4 | 8 => RED,
                _ => GREEN,
            };
            stage.glow(&lot_id(num), color);
        }
        stage.sound(&sound_events::BLOCK_AMETHYST_BLOCK_CHIME, 1.2);
    });
}

fn walking_in(stage: &mut Stage) {
    let front = lot(FRONT_LOT);
    stage.later(5, |stage| {
        stage.glow("common", 0);
        for num in 1..=LOTS.len() {
            stage.glow(&lot_id(num), 0);
        }
    });
    stage.later(10, |stage| {
        let head = stage.player_head();
        stage.item("head", head, 3.0, 1.4, 3.0, 0.6);
    });
    stage.later(25, move |stage| {
        stage.move_to("head", centre_x(front), 1.4, centre_z(front), 0.0, 25);
    });
    stage.later(55, move |stage| {
        let text = colored(&format!("Зона {FRONT_LOT} · Steve"), RED);
        tag(stage, "bar", text, front, 2.4);
        stage.pulse("bar");
        recolor(stage, &lot_id(FRONT_LOT), &vanilla_blocks::RED_CARPET);
        stage.sound(&sound_events::BLOCK_NOTE_BLOCK_CHIME, 1.2);
    });
}

fn not_a_claim(stage: &mut Stage) {
    stage.remove("bar");
    let [x, z, ..] = lot(FRONT_LOT).map(f32::from);
    stage.later(10, move |stage| {
        for n in 0..3_u8 {
            let planks = vanilla_blocks::OAK_PLANKS.default_state();
            stage.block(&format!("house{n}"), planks, x + f32::from(n), 1.0, z);
        }
        stage.sound(&sound_events::BLOCK_WOOD_PLACE, 1.0);
    });
    stage.later(55, move |stage| {
        let barrier = ItemStack::new(&vanilla_items::BARRIER);
        stage.item("no", barrier, x + 1.0, 2.2, z, 0.7);
        stage.glow("no", RED);
        stage.pulse("no");
        let pick = ItemStack::new(&vanilla_items::IRON_PICKAXE);
        stage.item("pick", pick, x + 1.0, 2.2, z + 1.0, 0.5);
        stage.sound(&sound_events::ENTITY_VILLAGER_NO, 1.0);
    });
}

// ---- the auction and the lease ---------------------------------------------------------------

fn auction() -> Guide {
    Guide::new(AUCTION, "Зоны спавна: аукцион и аренда", 7, 3)
        .visible_when(open)
        .step(
            "Аукцион по зоне объявляет администрация, о старте пишут в чат. Обычно аукцион идёт 48 часов.",
            100,
            announced,
        )
        .step(
            "Ставку делают, стоя в зоне: зайди в неё и нажми /zones bid. С сайта тоже можно, пока персонаж стоит в зоне. Первая не меньше стартовой цены, каждая следующая хотя бы на 5% больше.",
            160,
            bid,
        )
        .step(
            "АР за ставку сразу списываются со счёта. Если её перебьют, они вернутся на счёт, а в игре придёт сообщение.",
            120,
            outbid,
        )
        .step(
            "Антиснайп: ставка в последние 10 минут продлевает аукцион, чтобы до конца снова оставалось 10 минут.",
            120,
            antisnipe,
        )
        .step(
            "Кто поставил больше всех к концу, получает зону на 7 дней: ставка — это плата за первую неделю.",
            110,
            won,
        )
        .step(
            "Одна зона на игрока. Пока у тебя есть зона или твоя ставка лидирует на другом аукционе, на новую зону не поставить.",
            110,
            one_zone,
        )
        .step(
            "Продлить аренду можно за 2 дня до конца: «Продлить на 7 дней» в /zones или на сайте. Цена у зоны своя и не растёт, дни прибавляются: 2 + 7 = 9.",
            140,
            renewal,
        )
        .step(
            "Сама аренда не продлевается, в последние 2 дня об этом напомнят при входе. Не продлил — зона уходит на новый аукцион, и её может забрать любой.",
            150,
            expired,
        )
}

/// Changes a label and swells it.
fn retext(stage: &mut Stage, id: &str, text: &TextComponent) {
    stage.set_text(id, text);
    stage.pulse(id);
}

fn announced(stage: &mut Stage) {
    grass(stage, 7, 3);
    stage.later(25, |stage| {
        zone(stage, "lot", LOT, &vanilla_blocks::YELLOW_CARPET);
        stage.pulse("lot");
        stage.sound(&sound_events::BLOCK_WOOL_PLACE, 1.0);
    });
    stage.later(45, |stage| {
        stage.label("state", colored("Аукцион · 4 ч", GOLD), 3.0, 2.9, 1.0);
        stage.pulse("state");
        stage.sound(&sound_events::BLOCK_NOTE_BLOCK_BELL, 1.2);
    });
}

fn bid(stage: &mut Stage) {
    stage.label("price", plain("20 АР"), 3.0, 2.3, 1.0);
    stage.later(20, |stage| {
        let head = stage.player_head();
        stage.item("me", head, 2.0, 1.4, 2.0, 0.6);
    });
    stage.later(45, |stage| {
        retext(stage, "price", &colored("21 АР · ты", AQUA));
        stage.sound(&sound_events::BLOCK_NOTE_BLOCK_CHIME, 1.4);
    });
}

fn outbid(stage: &mut Stage) {
    stage.later(10, |stage| {
        let head = ItemStack::new(&vanilla_items::PLAYER_HEAD);
        stage.item("rival", head, 4.0, 1.4, 2.0, 0.6);
    });
    stage.later(35, |stage| {
        retext(stage, "price", &plain("23 АР · Steve"));
        stage.sound(&sound_events::BLOCK_NOTE_BLOCK_BASS, 1.0);
    });
    stage.later(60, |stage| {
        stage.label("back", colored("+21 АР вернулись", GREEN), 2.0, 2.0, 2.0);
        stage.pulse("back");
        stage.sound(&sound_events::ENTITY_EXPERIENCE_ORB_PICKUP, 1.0);
    });
}

fn antisnipe(stage: &mut Stage) {
    stage.remove("back");
    stage.set_text("state", &colored("Аукцион · 0:03", GOLD));
    stage.later(15, |stage| {
        let clock = ItemStack::new(&vanilla_items::CLOCK);
        stage.item("clock", clock, 5.0, 1.6, 1.0, 0.6);
        stage.pulse("clock");
    });
    stage.later(45, |stage| {
        retext(stage, "price", &colored("25 АР · ты", AQUA));
        stage.sound(&sound_events::BLOCK_NOTE_BLOCK_CHIME, 1.4);
    });
    stage.later(60, |stage| {
        retext(stage, "state", &colored("Аукцион · 10:00", GOLD));
        stage.sound(&sound_events::BLOCK_NOTE_BLOCK_BELL, 1.2);
    });
}

fn won(stage: &mut Stage) {
    stage.remove("clock");
    stage.remove("rival");
    stage.remove("price");
    stage.later(15, |stage| {
        recolor(stage, "lot", &vanilla_blocks::RED_CARPET);
        retext(stage, "state", &colored("Твоя зона · 7 дн", AQUA));
        stage.move_to("me", 3.0, 1.4, 1.0, 0.0, 20);
        stage.sound(&sound_events::ENTITY_PLAYER_LEVELUP, 1.2);
    });
}

fn one_zone(stage: &mut Stage) {
    stage.later(10, |stage| {
        let carpet = ItemStack::new(&vanilla_items::YELLOW_CARPET);
        stage.item("other", carpet, 6.0, 1.6, 1.0, 0.6);
        let barrier = ItemStack::new(&vanilla_items::BARRIER);
        stage.item("no", barrier, 6.0, 2.4, 1.0, 0.5);
        stage.glow("no", RED);
        stage.pulse("no");
        stage.sound(&sound_events::ENTITY_VILLAGER_NO, 1.0);
    });
}

fn renewal(stage: &mut Stage) {
    stage.remove("other");
    stage.remove("no");
    stage.set_text("state", &plain("Твоя зона · 2 дн"));
    stage.later(40, |stage| {
        retext(stage, "state", &colored("Твоя зона · 9 дн", AQUA));
        stage.pulse("lot");
        stage.sound(&sound_events::BLOCK_NOTE_BLOCK_CHIME, 1.4);
    });
}

fn expired(stage: &mut Stage) {
    stage.set_text("state", &colored("Твоя зона · 0 дн", RED));
    stage.later(40, |stage| stage.move_to("me", 0.0, 1.4, 2.0, 0.0, 20));
    stage.later(60, |stage| {
        recolor(stage, "lot", &vanilla_blocks::YELLOW_CARPET);
        retext(stage, "state", &colored("Аукцион · 4 ч", GOLD));
        stage.sound(&sound_events::BLOCK_NOTE_BLOCK_BELL, 1.2);
    });
}
