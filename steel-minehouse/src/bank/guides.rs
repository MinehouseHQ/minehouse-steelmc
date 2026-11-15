//! The guides of the bank (the Fabric `BankGuides`): the ATM and the card, and the payment
//! terminal (the till, the vending machine, the lock). They open with sneak + right click in
//! the air with a card or a terminal, and with `/guide`, while the bank is on.
//!
//! The scene is 7 by 3 cells, the floor at y 0, the middle at x 3, z 1.

use steel_registry::{
    blocks::BlockRef, item_stack::ItemStack, sound_events, vanilla_blocks, vanilla_items,
};
use text_components::{Modifier as _, TextComponent, format::Color};

use crate::{
    content::{CARD, TERMINAL},
    guides::{Guide, stage::Stage},
};

const WIDTH: u8 = 7;
const DEPTH: u8 = 3;
const AQUA: u32 = 0x004F_E0D4;
const GOLD: u32 = 0x00F5_D33D;
const GREEN: u32 = 0x0055_FF55;
const ATM_MODEL: &str = "minehouse-bank:atm";
const TERMINAL_MODEL: &str = "minehouse-bank:terminal";
const LOCKED_MODEL: &str = "minehouse-bank:terminal_locked";

/// The guides of the bank.
#[must_use]
pub fn all() -> Vec<Guide> {
    vec![atm(), terminal()]
}

fn open() -> bool {
    super::bank().is_some()
}

fn colored(text: &str, rgb: u32) -> TextComponent {
    let [_, red, green, blue] = rgb.to_be_bytes();
    TextComponent::plain(text.to_owned()).color(Color::Rgb(red, green, blue))
}

fn plain(text: &str) -> TextComponent {
    TextComponent::plain(text.to_owned())
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

fn card(stage: &mut Stage, id: &str, x: f32, y: f32, z: f32, size: f32) {
    stage.item(id, CARD.stack(1), x, y, z, size);
    stage.glow(id, AQUA);
}

fn atm() -> Guide {
    Guide::new("bank-atm", "Банк: банкомат и карта", WIDTH, DEPTH)
        .visible_when(open)
        .opens_with(&CARD)
        .step(
            "Банкоматы стоят в городе. ПКМ по банкомату открывает счёт в АР и выдаёт карту MineHouse.",
            90,
            |stage| {
                floor(stage, &vanilla_blocks::SMOOTH_STONE);
                stage.later(20, |stage| {
                    stage.model("atm", ATM_MODEL, 3.0, 1.0, 1.0);
                    stage.pulse("atm");
                    stage.sound(&sound_events::BLOCK_STONE_PLACE, 1.0);
                });
                stage.later(45, |stage| {
                    let head = stage.player_head();
                    stage.item("head", head, 3.0, 1.0, 2.6, 0.6);
                });
                stage.later(65, |stage| {
                    card(stage, "card", 3.0, 2.1, 2.2, 0.5);
                    stage.pulse("card");
                    stage.sound(&sound_events::BLOCK_NOTE_BLOCK_CHIME, 1.4);
                });
            },
        )
        .step(
            "Положи алмазную руду в слоты банкомата и нажми «Внести». 1 руда = 1 АР, глубинная тоже.",
            110,
            |stage| {
                stage.remove("card");
                stage.label("balance", plain("0 АР"), 3.0, 2.9, 1.0);
                for n in 0..4_u8 {
                    stage.later(5 + u32::from(n) * 8, move |stage| {
                        let ore = if n % 2 == 0 {
                            &vanilla_items::DIAMOND_ORE
                        } else {
                            &vanilla_items::DEEPSLATE_DIAMOND_ORE
                        };
                        let x = 1.6 + f32::from(n) * 0.9;
                        stage.item(&format!("ore{n}"), ItemStack::new(ore), x, 1.2, 2.4, 0.4);
                    });
                }
                stage.later(45, |stage| {
                    for n in 0..4 {
                        stage.move_to(&format!("ore{n}"), 3.0, 1.9, 1.0, 0.0, 15);
                    }
                    stage.sound(&sound_events::UI_BUTTON_CLICK, 1.0);
                });
                stage.later(65, |stage| {
                    for n in 0..4 {
                        stage.remove(&format!("ore{n}"));
                    }
                    stage.pulse("atm");
                    stage.set_text("balance", &colored("4 АР", AQUA));
                    stage.pulse("balance");
                    stage.sound(&sound_events::BLOCK_NOTE_BLOCK_CHIME, 1.4);
                });
            },
        )
        .step(
            "«Снять АР» выдаёт глубинную алмазную руду обратно, сколько попросишь.",
            120,
            |stage| {
                stage.later(15, |stage| {
                    let ore = ItemStack::new(&vanilla_items::DEEPSLATE_DIAMOND_ORE);
                    stage.item("out", ore, 3.0, 1.9, 1.0, 0.4);
                    stage.move_to("out", 3.0, 1.2, 2.4, 0.0, 15);
                    stage.set_text("balance", &plain("3 АР"));
                    stage.pulse("balance");
                    stage.sound(&sound_events::BLOCK_DISPENSER_DISPENSE, 1.0);
                });
            },
        )
        .step(
            "Баланс, история и блокировка карты — на сайте minehouse.pro/bank. Потерял карту: «Перевыпустить карту» в банкомате, старая перестанет работать.",
            110,
            |stage| {
                stage.remove("out");
                stage.later(10, |stage| {
                    card(stage, "card", 3.0, 2.1, 2.2, 0.5);
                    stage.pulse("card");
                });
            },
        )
}

fn terminal() -> Guide {
    Guide::new("bank-terminal", "Банк: терминал оплаты", WIDTH, DEPTH)
        .visible_when(open)
        .opens_with(&TERMINAL)
        .step(
            "Терминал продаёт банкомат за 5 АР. Поставь его на прилавок или на сундук.",
            80,
            |stage| {
                floor(stage, &vanilla_blocks::SPRUCE_PLANKS);
                stage.later(15, |stage| {
                    let chest = vanilla_blocks::CHEST.default_state();
                    stage.block("chest", chest, 3.0, 1.0, 1.0);
                });
                stage.later(35, |stage| {
                    stage.model("terminal", TERMINAL_MODEL, 3.0, 2.0, 1.0);
                    stage.pulse("terminal");
                    stage.sound(&sound_events::BLOCK_STONE_PLACE, 1.2);
                });
            },
        )
        .step(
            "ПКМ по новому терминалу: выбери счёт (свой подставится сам, чужой — по номеру MH-1234-5678) и режим.",
            90,
            |stage| {
                stage.later(10, |stage| {
                    stage.label("number", colored("MH-1234-5678", AQUA), 3.0, 3.1, 1.0);
                });
            },
        )
        .step(
            "Касса: ты выставляешь сумму, покупатель платит ПКМ по терминалу с картой в инвентаре. Деньги сразу на счёте.",
            110,
            |stage| {
                stage.remove("number");
                stage.later(10, |stage| {
                    stage.label("bill", plain("К оплате: 12 АР"), 3.0, 3.1, 1.0);
                    stage.pulse("bill");
                });
                stage.later(35, |stage| {
                    let head = stage.player_head();
                    stage.item("buyer", head, 3.0, 1.0, 2.6, 0.6);
                });
                stage.later(55, |stage| {
                    card(stage, "card", 3.0, 1.8, 2.2, 0.45);
                    stage.move_to("card", 3.0, 2.3, 1.4, 0.0, 15);
                });
                stage.later(80, |stage| {
                    stage.remove("card");
                    stage.set_text("bill", &colored("Оплачено", GREEN));
                    stage.pulse("bill");
                    stage.sound(&sound_events::BLOCK_NOTE_BLOCK_CHIME, 1.4);
                });
            },
        )
        .step(
            "Автомат: товар лежит в сундуке под терминалом и продаётся сам, даже когда тебя нет в игре. Сундук открыт всем: можно заплатить картой через терминал или наличкой — положить руду в сундук и забрать товар.",
            130,
            |stage| {
                stage.remove("bill");
                stage.label("price", plain("Хлеб × 4 — 2 АР"), 3.0, 3.1, 1.0);
                stage.later(30, |stage| {
                    let bread = ItemStack::with_count(&vanilla_items::BREAD, 4);
                    stage.item("goods", bread, 3.0, 1.5, 1.0, 0.4);
                    stage.move_to("goods", 3.0, 1.4, 2.4, 0.0, 20);
                    stage.sound(&sound_events::BLOCK_CHEST_OPEN, 1.0);
                });
                stage.later(70, |stage| {
                    stage.label("sold", colored("+2 АР", AQUA), 3.0, 3.5, 1.0);
                    stage.pulse("sold");
                    stage.sound(&sound_events::BLOCK_NOTE_BLOCK_CHIME, 1.4);
                });
            },
        )
        .step(
            "«Сохранить и закрепить» вешает замок. Снять его можно только сломав терминал, и это видно в журнале блоков и на сайте.",
            100,
            |stage| {
                stage.remove("goods");
                stage.remove("sold");
                stage.remove("buyer");
                stage.later(15, |stage| {
                    stage.set_model("terminal", LOCKED_MODEL);
                    stage.glow("terminal", GOLD);
                    stage.pulse("terminal");
                    stage.sound(&sound_events::BLOCK_CHAIN_PLACE, 1.0);
                });
            },
        )
}
