//! The guides of the magic herb (the Fabric `HerbGuides`): finding and growing it (sneak +
//! right click in the air with seeds), the broth and the papirosa (the same with a leaf).
//!
//! The scenes in cells: the floor at y 0, the middle cell is x 3, z 1.

use steel_registry::{
    REGISTRY, blocks::BlockRef, item_stack::ItemStack, sound_events, vanilla_blocks, vanilla_items,
};
use steel_utils::BlockStateId;
use text_components::{Modifier as _, TextComponent, format::Color};

use super::{BROTH, LEAF, PAPIROSA, SEEDS};
use crate::guides::{Guide, stage::Stage};

const WIDTH: u8 = 7;
const DEPTH: u8 = 3;
const MAGIC: u32 = 0x00B0_4CFF;
const EMBER: u32 = 0x00FF_8A3D;

/// The guides of the magic herb.
#[must_use]
pub fn all() -> Vec<Guide> {
    vec![herb(), broth()]
}

// ---- helpers ---------------------------------------------------------------------------------

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

/// A vanilla cauldron filled to `level`.
fn cauldron(block: BlockRef, level: u8) -> BlockStateId {
    REGISTRY
        .blocks
        .state_id_from_block_properties(block, &[("level", &level.to_string())])
        .unwrap_or_else(|| block.default_state())
}

/// A block of the herbs as the pack draws it for the guides.
fn model(block: &str) -> String {
    format!("minehouse-herbs:guide_{block}")
}

fn plain(text: &str) -> TextComponent {
    TextComponent::plain(text.to_owned())
}

/// An item of ours lying in the row of the scene.
fn lay(stage: &mut Stage, id: &str, stack: ItemStack, x: f32, z: f32, size: f32) {
    stage.item(id, stack, x, 1.0, z, size);
}

// ---- finding and growing ---------------------------------------------------------------------

fn herb() -> Guide {
    Guide::new("magic-herb", "Магическая трава", WIDTH, DEPTH)
        .opens_with(&SEEDS)
        .step(
            "Дикий куст с шишками растёт только в Раю: на травяных островах и в берёзовых рощах, среди цветов.",
            80,
            wild,
        )
        .step(
            "Куст ломается один раз: он исчезает и даёт 2–3 магических семени.",
            60,
            broken,
        )
        .step(
            "Семена сажаются на вспаханную землю, как свёкла, но растут вдвое медленнее.",
            130,
            growing,
        )
        .step(
            "Выросшее растение даёт 1–2 магических листа и 1–2 семени. Семена сажай снова, листья — в отвар и папиросы.",
            70,
            harvest,
        )
}

fn wild(stage: &mut Stage) {
    floor(stage, &vanilla_blocks::GRASS_BLOCK);
    let flowers: [BlockRef; 5] = [
        &vanilla_blocks::POPPY,
        &vanilla_blocks::ALLIUM,
        &vanilla_blocks::OXEYE_DAISY,
        &vanilla_blocks::CORNFLOWER,
        &vanilla_blocks::AZURE_BLUET,
    ];
    let cells: [(u8, u8); 8] = [
        (0, 0),
        (1, 2),
        (2, 0),
        (4, 2),
        (5, 0),
        (6, 1),
        (1, 0),
        (5, 2),
    ];
    for (n, ((x, z), flower)) in (0_u32..).zip(cells.into_iter().zip(flowers.into_iter().cycle())) {
        stage.later(10 + n * 2, move |stage| {
            stage.block(
                &format!("flower{x}_{z}"),
                flower.default_state(),
                f32::from(x),
                1.0,
                f32::from(z),
            );
        });
    }
    stage.later(35, |stage| {
        stage.model("wild", &model("wild_magic_herb"), 3.0, 1.0, 1.0);
        stage.glow("wild", MAGIC);
        stage.pulse("wild");
        stage.sound(&sound_events::BLOCK_AMETHYST_BLOCK_CHIME, 1.2);
    });
}

fn broken(stage: &mut Stage) {
    stage.later(10, |stage| {
        stage.remove("wild");
        stage.sound(&sound_events::BLOCK_GRASS_BREAK, 1.0);
        for n in 0..3_u8 {
            let id = format!("seed{n}");
            lay(
                stage,
                &id,
                SEEDS.stack(1),
                f32::from(n).mul_add(0.4, 2.6),
                1.0,
                0.45,
            );
            stage.pulse(&id);
        }
    });
}

fn growing(stage: &mut Stage) {
    for n in 0..3 {
        stage.remove(&format!("seed{n}"));
    }
    stage.later(5, |stage| {
        stage.set_state("ground3_1", vanilla_blocks::FARMLAND.default_state());
        stage.pulse("ground3_1");
        stage.sound(&sound_events::ITEM_HOE_TILL, 1.0);
    });
    stage.later(25, |stage| {
        stage.model("crop", &model("magic_herb_stage0"), 3.0, 1.0, 1.0);
        stage.sound(&sound_events::ITEM_CROP_PLANT, 1.0);
    });
    for age in 1..=3_u32 {
        stage.later(25 + age * 30, move |stage| {
            stage.set_model("crop", &model(&format!("magic_herb_stage{age}")));
            stage.pulse("crop");
        });
    }
    stage.later(115, |stage| stage.glow("crop", MAGIC));
}

fn harvest(stage: &mut Stage) {
    stage.later(10, |stage| {
        stage.remove("crop");
        stage.sound(&sound_events::BLOCK_CROP_BREAK, 1.0);
        lay(stage, "leaf0", LEAF.stack(1), 2.6, 1.0, 0.45);
        lay(stage, "leaf1", LEAF.stack(1), 3.0, 1.0, 0.45);
        lay(stage, "seed", SEEDS.stack(1), 3.4, 1.0, 0.45);
        for id in ["leaf0", "leaf1", "seed"] {
            stage.pulse(id);
        }
    });
}

// ---- the broth and the papirosa --------------------------------------------------------------

fn broth() -> Guide {
    Guide::new("magic-broth", "Магический отвар и папироса", WIDTH, DEPTH)
        .opens_with(&LEAF)
        .step(
            "Поставь котёл на горящий костёр: без огня отвар не варится.",
            60,
            fire,
        )
        .step("Вылей в котёл 3 ведра молока.", 80, milk)
        .step(
            "Брось в молоко 10 магических листьев: ПКМ листом по котлу.",
            140,
            leaves,
        )
        .step(
            "Отвара хватает на 5 мисок, набирай миской. Он даёт +4 сердца и замедление на 2 минуты и левитацию на 10 секунд.",
            130,
            bowls,
        )
        .step(
            "2 магических листа и бумага на верстаке — магическая папироса.",
            80,
            rolling,
        )
        .step(
            "Держи огниво во второй руке и зажми ПКМ: папироса курится 8 секунд. +2 сердца и замедление; больше трёх за 5 минут — тошнота и темнота.",
            140,
            smoking,
        )
}

fn fire(stage: &mut Stage) {
    floor(stage, &vanilla_blocks::STONE_BRICKS);
    stage.later(15, |stage| {
        let campfire = vanilla_blocks::CAMPFIRE.default_state();
        stage.block("campfire", campfire, 3.0, 1.0, 1.0);
        stage.sound(&sound_events::BLOCK_CAMPFIRE_CRACKLE, 1.0);
    });
    stage.later(35, |stage| {
        let empty = vanilla_blocks::CAULDRON.default_state();
        stage.block("cauldron", empty, 3.0, 2.0, 1.0);
        stage.pulse("cauldron");
        stage.sound(&sound_events::BLOCK_ANVIL_PLACE, 1.6);
    });
}

fn milk(stage: &mut Stage) {
    let bucket = ItemStack::new(&vanilla_items::MILK_BUCKET);
    stage.item("bucket", bucket, 3.0, 3.2, 1.0, 0.6);
    for level in 1..=3_u8 {
        stage.later(u32::from(level) * 20, move |stage| {
            // Milk is powder snow to the client, as in the world.
            let milk = cauldron(&vanilla_blocks::POWDER_SNOW_CAULDRON, level);
            stage.set_state("cauldron", milk);
            stage.pulse("cauldron");
            stage.pulse("bucket");
            stage.sound(&sound_events::ITEM_BUCKET_EMPTY, 1.0);
        });
    }
}

fn leaves(stage: &mut Stage) {
    stage.remove("bucket");
    stage.item("leaf", LEAF.stack(1), 3.0, 3.2, 1.0, 0.6);
    stage.label("count", plain("0 / 10"), 3.0, 3.6, 1.0);
    for n in 1..=10_u32 {
        stage.later(5 + n * 10, move |stage| {
            stage.pulse("leaf");
            stage.set_text("count", &plain(&format!("{n} / 10")));
            stage.sound(&sound_events::ENTITY_GENERIC_SPLASH, 1.4);
        });
    }
    stage.later(120, |stage| {
        stage.remove("leaf");
        stage.set_state("cauldron", cauldron(&vanilla_blocks::WATER_CAULDRON, 3));
        stage.glow("cauldron", MAGIC);
        stage.pulse("cauldron");
        let [_, red, green, blue] = MAGIC.to_be_bytes();
        let ready = plain("Отвар готов!").color(Color::Rgb(red, green, blue));
        stage.set_text("count", &ready);
        stage.sound(&sound_events::BLOCK_BREWING_STAND_BREW, 1.0);
    });
}

fn bowls(stage: &mut Stage) {
    stage.remove("count");
    stage.glow("cauldron", 0);
    let levels: [u8; 5] = [3, 2, 2, 1, 0];
    for (n, level) in (0_u8..).zip(levels) {
        stage.later(10 + u32::from(n) * 18, move |stage| {
            let id = format!("broth{n}");
            lay(stage, &id, BROTH.stack(1), 1.0 + f32::from(n), 2.0, 0.5);
            stage.pulse(&id);
            stage.sound(&sound_events::ITEM_BOTTLE_FILL, 1.0);
            let left = if level == 0 {
                vanilla_blocks::CAULDRON.default_state()
            } else {
                cauldron(&vanilla_blocks::WATER_CAULDRON, level)
            };
            stage.set_state("cauldron", left);
        });
    }
}

fn rolling(stage: &mut Stage) {
    for n in 0..5 {
        stage.remove(&format!("broth{n}"));
    }
    stage.later(5, |stage| {
        lay(stage, "c-leaf0", LEAF.stack(1), 0.0, 2.0, 0.5);
    });
    stage.later(10, |stage| {
        lay(stage, "c-leaf1", LEAF.stack(1), 1.0, 2.0, 0.5);
    });
    stage.later(15, |stage| {
        let paper = ItemStack::new(&vanilla_items::PAPER);
        lay(stage, "c-paper", paper, 2.0, 2.0, 0.5);
    });
    stage.later(20, |stage| stage.label("c-eq", plain("="), 3.0, 1.1, 2.0));
    stage.later(35, |stage| {
        lay(stage, "c-papirosa", PAPIROSA.stack(1), 4.0, 2.0, 0.6);
        stage.glow("c-papirosa", MAGIC);
        stage.pulse("c-papirosa");
        stage.sound(&sound_events::ENTITY_VILLAGER_WORK_CARTOGRAPHER, 1.0);
    });
}

fn smoking(stage: &mut Stage) {
    for id in ["c-leaf0", "c-leaf1", "c-paper", "c-eq"] {
        stage.remove(id);
    }
    stage.glow("c-papirosa", 0);
    stage.later(10, |stage| {
        let head = stage.player_head();
        lay(stage, "head", head, 5.0, 2.0, 0.6);
    });
    stage.later(20, |stage| {
        let flint = ItemStack::new(&vanilla_items::FLINT_AND_STEEL);
        lay(stage, "flint", flint, 6.0, 2.0, 0.5);
        stage.pulse("flint");
    });
    stage.later(40, |stage| {
        stage.sound(&sound_events::ITEM_FLINTANDSTEEL_USE, 1.0);
        stage.glow("c-papirosa", EMBER);
        stage.pulse("c-papirosa");
    });
    for n in 0..4_u32 {
        stage.later(60 + n * 20, |stage| {
            stage.sound(&sound_events::BLOCK_CAMPFIRE_CRACKLE, 1.4);
            stage.pulse("c-papirosa");
        });
    }
}
