//! The guide of the magnifier (the Fabric `MagnifierGuide`): the recipe, a hit on a block, a
//! use on a container, a use on a face for a block that is gone, and paging. The chat in the
//! scene is drawn by [`lines`], so it looks exactly like the real answer.
//!
//! The scene in cells: the floor at y 0, the viewer's head on the left, the inspected block at
//! x 4, z 1.

use glam::Vec3;
use steel_registry::{
    blocks::BlockRef, item_stack::ItemStack, items::ItemRef, sound_event::SoundEvent, sound_events,
    vanilla_blocks, vanilla_items,
};
use text_components::TextComponent;

use super::{
    magnifier::{Glance, PAGE_SIZE, lines},
    now_ms,
    store::{Actor, BREAK, Event, PLACE, PUT, Subject, TAKE},
};
use crate::{
    content::MAGNIFIER,
    guides::{
        Guide,
        stage::{Piece, Stage},
    },
    moderation::format::render,
};

const WIDTH: u8 = 7;
const DEPTH: u8 = 3;
const GLASS: u32 = 0x00FF_C857;
const RED: u32 = 0x00FF_5555;
const TARGET_X: f32 = 4.0;
const ROW: f32 = 1.0;
/// Where the chat label hangs, over the middle of the stage.
const CHAT_X: f32 = 3.0;
const CHAT_Y: f32 = 2.35;
/// Smaller than a label: it is a whole chat page.
const CHAT_SCALE: f32 = 0.55;
const RECIPE_ITEMS: u8 = 6;
const SHOWN_POS: (i32, i32, i32) = (128, 64, -42);
const HOUR: i64 = 3600;

/// The guide of the magnifier.
#[must_use]
pub fn guide() -> Guide {
    Guide::new("magnifier", "Лупа", WIDTH, DEPTH)
        .visible_when(super::on)
        .opens_with(&MAGNIFIER)
        .step(
            "Лупа крафтится на верстаке: 4 медных слитка, стеклянная панель и палка.",
            90,
            recipe,
        )
        .step(
            "ЛКМ лупой по блоку: в чате появится, кто его ставил и ломал. Сам блок при этом не ломается.",
            120,
            block,
        )
        .step(
            "ПКМ по сундуку, бочке или печке: кто что клал и забирал. Сундук при этом не открывается.",
            120,
            chest,
        )
        .step(
            "Блока уже нет? ПКМ по соседнему блоку с той стороны, где он стоял: лупа покажет, кто его сломал.",
            120,
            gone,
        )
        .step(
            "За раз видно 7 записей. Кликни по тому же блоку ещё раз в течение минуты, и покажутся следующие. У откаченных записей стоит пометка «(откачено)».",
            150,
            pages,
        )
}

// ---- the steps -------------------------------------------------------------------------------

fn recipe(stage: &mut Stage) {
    floor(stage, &vanilla_blocks::SPRUCE_PLANKS);
    // " C " / "CGC" / "SC ", standing up over the floor like a crafting grid.
    let copper = &vanilla_items::COPPER_INGOT;
    let grid: [[Option<ItemRef>; 3]; 3] = [
        [None, Some(copper), None],
        [Some(copper), Some(&vanilla_items::GLASS_PANE), Some(copper)],
        [Some(&vanilla_items::STICK), Some(copper), None],
    ];
    let mut n = 0_u8;
    for (row, items) in (0_u8..).zip(grid) {
        for (col, item) in (0_u8..).zip(items) {
            let Some(item) = item else {
                continue;
            };
            let id = format!("r{n}");
            let x = f32::from(col).mul_add(0.8, 0.2);
            let y = f32::from(row).mul_add(-0.8, 2.6);
            stage.later(15 + u32::from(n) * 5, move |stage| {
                stage.item(&id, ItemStack::new(item), x, y, ROW, 0.38);
                stage.sound(&sound_events::ENTITY_ITEM_PICKUP, 1.2);
            });
            n += 1;
        }
    }
    stage.later(50, |stage| {
        stage.label("eq", TextComponent::plain("="), 2.9, 1.5, ROW);
    });
    stage.later(62, |stage| {
        stage.item("result", MAGNIFIER.stack(1), 4.2, 1.7, ROW, 0.8);
        stage.glow("result", GLASS);
        stage.pulse("result");
        stage.sound(&sound_events::ENTITY_VILLAGER_WORK_TOOLSMITH, 1.0);
    });
}

fn block(stage: &mut Stage) {
    clear(stage, &["eq", "result"]);
    for n in 0..RECIPE_ITEMS {
        stage.remove(&format!("r{n}"));
    }
    stage.later(5, |stage| {
        let head = stage.player_head();
        stage.item("head", head, 1.0, 1.0, ROW, 0.6);
    });
    stage.later(10, |stage| {
        let planks = vanilla_blocks::OAK_PLANKS.default_state();
        stage.block("target", planks, TARGET_X, 1.0, ROW);
        stage.block("target2", planks, TARGET_X + 1.0, 1.0, ROW);
    });
    click(
        stage,
        30,
        TARGET_X - 0.7,
        &sound_events::BLOCK_WOOD_HIT,
        |stage| {
            stage.glow("target", GLASS);
            stage.pulse("target");
            let now = now_ms();
            let me = viewer(stage);
            let planks = "minecraft:oak_planks";
            let rows = [
                placed(now, 3 * HOUR, &me, PLACE, planks),
                placed(now, 5 * HOUR, "Steve", BREAK, planks),
                placed(now, 48 * HOUR, "Steve", PLACE, planks),
            ];
            chat(stage, lines(&glance(false, 0), planks, &rows, now));
        },
    );
}

fn chest(stage: &mut Stage) {
    clear(stage, &["chat", "target", "target2", "hand"]);
    stage.later(10, |stage| {
        let chest = vanilla_blocks::CHEST.default_state();
        stage.block("chest", chest, TARGET_X, 1.0, ROW);
        stage.sound(&sound_events::BLOCK_WOOD_PLACE, 1.0);
    });
    click(
        stage,
        30,
        TARGET_X - 0.7,
        &sound_events::BLOCK_CHEST_LOCKED,
        |stage| {
            stage.glow("chest", GLASS);
            stage.pulse("chest");
            let now = now_ms();
            let me = viewer(stage);
            let rows = [
                moved(now, 600, "Steve", TAKE, "minecraft:diamond", 16),
                moved(now, 26 * HOUR, &me, PUT, "minecraft:diamond", 16),
                moved(now, 26 * HOUR, &me, PUT, "minecraft:emerald", 32),
            ];
            chat(
                stage,
                lines(&glance(true, 0), "minecraft:chest", &rows, now),
            );
        },
    );
}

fn gone(stage: &mut Stage) {
    clear(stage, &["chat", "chest", "hand"]);
    stage.later(10, |stage| {
        let stone = vanilla_blocks::STONE.default_state();
        stage.block("stone", stone, TARGET_X + 1.0, 1.0, ROW);
        stage.block("stone2", stone, TARGET_X + 1.0, 2.0, ROW);
        let ore = vanilla_blocks::DIAMOND_ORE.default_state();
        stage.block("ore", ore, TARGET_X, 1.0, ROW);
    });
    // The ore is mined: an empty spot is left next to the stone.
    stage.later(30, |stage| {
        stage.remove("ore");
        stage.sound(&sound_events::BLOCK_STONE_BREAK, 1.0);
    });
    stage.later(42, |stage| {
        let glass = vanilla_blocks::GLASS.default_state();
        stage.block("spot", glass, TARGET_X, 1.0, ROW);
        stage.glow("spot", RED);
    });
    click(
        stage,
        55,
        TARGET_X + 0.3,
        &sound_events::BLOCK_STONE_HIT,
        |stage| {
            stage.glow("stone", GLASS);
            stage.pulse("stone");
            let now = now_ms();
            let rows = [placed(now, 1200, "Steve", BREAK, "minecraft:diamond_ore")];
            chat(stage, lines(&glance(false, 0), "minecraft:air", &rows, now));
        },
    );
}

/// The history of the last step, the newest first. The TNT blast was rolled back by an admin;
/// it is the second row, so the short page of the scene shows how such a row looks.
fn history(now: i64) -> Vec<Event> {
    let who = [
        "Steve", "#tnt", "Alex", "#creeper", "Steve", "Alex", "Steve", "Alex", "Steve",
    ];
    (1_i64..)
        .zip(who)
        .map(|(n, who)| {
            let action = if n % 2 == 1 { PLACE } else { BREAK };
            let mut event = placed(now, n * 3 * HOUR, who, action, "minecraft:cobblestone");
            event.rolled_back = n == 2;
            event
        })
        .collect()
}

fn pages(stage: &mut Stage) {
    clear(stage, &["chat", "hand", "spot", "stone", "stone2"]);
    stage.later(10, |stage| {
        let cobblestone = vanilla_blocks::COBBLESTONE.default_state();
        stage.block("target", cobblestone, TARGET_X, 1.0, ROW);
    });
    let page = PAGE_SIZE as usize;
    click(
        stage,
        25,
        TARGET_X - 0.7,
        &sound_events::BLOCK_STONE_HIT,
        move |stage| {
            stage.glow("target", GLASS);
            stage.pulse("target");
            let now = now_ms();
            let all = history(now);
            // One row more than a page: the answer ends with «ещё раз — дальше».
            let rows = all.get(..=page).unwrap_or(&all);
            let full = lines(&glance(false, 0), "minecraft:cobblestone", rows, now);
            chat(stage, shortened(full));
        },
    );
    click(
        stage,
        85,
        TARGET_X - 0.7,
        &sound_events::BLOCK_STONE_HIT,
        move |stage| {
            stage.pulse("target");
            let now = now_ms();
            let all = history(now);
            let rows = all.get(page..).unwrap_or_default();
            chat(
                stage,
                lines(&glance(false, 1), "minecraft:cobblestone", rows, now),
            );
        },
    );
}

// ---- helpers ---------------------------------------------------------------------------------

fn viewer(stage: &Stage) -> String {
    stage.player().gameprofile.name().to_owned()
}

const fn glance(container: bool, page: u32) -> Glance {
    Glance {
        world: String::new(),
        pos: SHOWN_POS,
        container,
        page,
        at: 0,
    }
}

fn event(now: i64, ago: i64, who: &str, action: u8, kind: &str, subject: Subject) -> Event {
    Event {
        time: now - ago * 1000,
        actor: Actor {
            name: who.to_owned(),
            uuid: None,
        },
        world: String::new(),
        pos: SHOWN_POS,
        kind: kind.to_owned(),
        action,
        rolled_back: false,
        subject,
    }
}

/// A block placed or broken `ago` seconds before `now`.
fn placed(now: i64, ago: i64, who: &str, action: u8, block: &str) -> Event {
    let subject = Subject::Block {
        state: String::new(),
    };
    event(now, ago, who, action, block, subject)
}

/// Items put into a container or taken out of it `ago` seconds before `now`.
fn moved(now: i64, ago: i64, who: &str, action: u8, item: &str, amount: i32) -> Event {
    let subject = Subject::Items { amount, data: None };
    event(now, ago, who, action, item, subject)
}

/// The magnifier flies from the head to the block, taps it `at` ticks in, then `effect`.
fn click(
    stage: &mut Stage,
    at: u32,
    x: f32,
    sound: &'static SoundEvent,
    effect: impl FnOnce(&mut Stage) + Send + 'static,
) {
    stage.later(at, |stage| {
        if !stage.has("hand") {
            stage.item("hand", MAGNIFIER.stack(1), 2.0, 1.2, ROW, 0.6);
        }
    });
    stage.later(at + 8, move |stage| {
        stage.move_to("hand", x, 1.1, ROW, 0.0, 8);
    });
    stage.later(at + 17, move |stage| {
        stage.sound(sound, 1.0);
        effect(stage);
    });
    stage.later(at + 24, |stage| {
        stage.move_to("hand", 2.0, 1.2, ROW, 0.0, 8);
    });
}

/// A full page does not fit over the stage: the header, the first two lines, "…" and the
/// footer.
fn shortened(lines: Vec<TextComponent>) -> Vec<TextComponent> {
    let last = lines.len().saturating_sub(1);
    lines
        .into_iter()
        .enumerate()
        .filter_map(|(n, line)| match n {
            0..3 => Some(line),
            3 => Some(render("&8…", &[])),
            _ => (n == last).then_some(line),
        })
        .collect()
}

/// Chat lines as one label, the newest page in place of the old one.
fn chat(stage: &mut Stage, lines: Vec<TextComponent>) {
    let mut text = TextComponent::plain("");
    for (n, line) in lines.into_iter().enumerate() {
        if n > 0 {
            text.children.push(TextComponent::plain("\n"));
        }
        text.children.push(line);
    }
    if stage.has("chat") {
        stage.set_text("chat", &text);
        stage.pulse("chat");
    } else {
        let piece = Piece::text(text, CHAT_SCALE, Vec3::new(0.5, 0.2, 0.5));
        stage.group("chat", CHAT_X, CHAT_Y, ROW + 0.3, vec![piece]);
    }
    stage.sound(&sound_events::UI_BUTTON_CLICK, 1.6);
}

fn floor(stage: &mut Stage, block: BlockRef) {
    for x in 0..WIDTH {
        for z in 0..DEPTH {
            stage.later(u32::from(x + z) * 2, move |stage| {
                stage.block(
                    &format!("ground{x}_{z}"),
                    block.default_state(),
                    f32::from(x),
                    0.0,
                    f32::from(z),
                );
            });
        }
    }
}

fn clear(stage: &mut Stage, ids: &[&str]) {
    for id in ids {
        stage.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_page_of_the_guide_is_full_and_has_a_next() {
        let all = history(0);
        let rows = &all[..=PAGE_SIZE as usize];
        let full = lines(&glance(false, 0), "minecraft:cobblestone", rows, 0);
        // The header, seven rows and «ещё раз — дальше».
        assert_eq!(full.len(), 9);
        let footer = format!("{:?}", full[8]);
        let short = shortened(full);
        assert_eq!(short.len(), 5);
        assert!(format!("{:?}", short[3]).contains('…'));
        assert!(format!("{:?}", short[2]).contains("откачено"));
        assert_eq!(format!("{:?}", short[4]), footer);
    }

    #[test]
    fn the_second_page_shows_what_is_left() {
        let all = history(0);
        let rows = &all[PAGE_SIZE as usize..];
        let page = lines(&glance(false, 1), "minecraft:cobblestone", rows, 0);
        assert_eq!(page.len(), 3);
        assert!(all[1].rolled_back && all[1].actor.name == "#tnt");
    }
}
