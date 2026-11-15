//! Our own items on a vanilla client.
//!
//! On the Fabric server these were registry items that Polymer turned into vanilla ones on
//! the way to the client. Here an item of ours *is* a vanilla item: the base item with a name,
//! a lore, an item model of the server resource pack, and its id in `custom_data`. The id and
//! the model are the ones the Fabric mods used, so the same resource pack draws them.

mod admin_mode;
mod afk;
mod all_recipes;
mod anvil_repair;
mod any_shape_portals;
pub(crate) mod blocks;
mod cauldron;
mod cauldron_conversion;
mod cloud;
mod commands;
mod couplings;
mod end_lock;
mod first_join;
mod herbs;
mod leaf_decay;
pub mod map_markers;
mod map_view;
mod metro;
mod minecart_names;
mod nametags;
mod papirosa;
mod rail_graph;
mod recipes;
mod ride;
mod see;
mod silent_tags;
mod speed;
mod station_menu;
mod stations;
mod trains;
mod transfers;
mod waxed_frames;
mod waxed_items;
mod xp_orbs;

use std::{str::FromStr as _, sync::LazyLock};

use simdnbt::owned::NbtCompound;
use steel_core::{
    behavior::{
        ItemBehavior, blocks::vegetation::set_leaf_decay_observer, extend_block_behaviors,
        items::set_name_tag_observer, set_item_behavior_override,
    },
    command::{CommandRegistrationError, CommandRegistry},
    entity::entities::objects::{
        display_ui::set_item_frame_observer,
        items::{set_experience_orb_observer, set_item_entity_observer},
        vehicles::set_minecart_observer,
    },
    inventory::craft_observer::set_craft_observer,
    player::{
        death_observer::set_player_death_observer, input_observer::set_player_input_observer,
    },
    scoreboard::set_team_packet_observer,
    server::Server,
    set_portal_shape_observer,
    world::set_sleep_observer,
};
use steel_registry::{
    data_components::{
        CustomData,
        vanilla_components::{
            CONSUMABLE, CUSTOM_DATA, Consumable, FOOD, ITEM_MODEL, ITEM_NAME, ItemLore,
            ItemStackTemplate, ItemUseAnimation, LORE, MAX_STACK_SIZE, USE_REMAINDER, UseRemainder,
        },
    },
    extend_registry,
    item_stack::ItemStack,
    items::Item,
    recipe::{Ingredient, set_extension_item_id},
    sound_event::{SoundEvent, SoundEventHolder},
    sound_events, vanilla_items,
};
use steel_utils::Identifier;
use text_components::TextComponent;

use crate::{
    bank::{atm_block::ATM_BLOCK, banker, terminal_block::TERMINAL_BLOCK, transfer, zones},
    guides,
    moderation::format::render,
};

/// The key in `custom_data` that holds the id of an item of ours.
pub const ID_KEY: &str = "minehouse_item";

/// An item of ours.
pub struct CustomItem {
    /// The id, as on the Fabric server: `minehouse-tweaks:magnifier`. Also its item model.
    pub id: &'static str,
    /// The vanilla item the client sees when it has no resource pack.
    pub base: &'static LazyLock<Item>,
    /// The name.
    pub name: &'static str,
    /// The lines under the name.
    pub lore: &'static [&'static str],
    /// How many fit a stack.
    pub max_stack: i32,
    /// How it is drunk or smoked, if it is.
    pub consume: Option<Consume>,
}

/// How an item of ours is consumed.
pub struct Consume {
    /// For how long, in seconds.
    pub seconds: f32,
    /// How the player holds it meanwhile.
    pub animation: ItemUseAnimation,
    /// The sound of it, every few ticks.
    pub sound: &'static SoundEvent,
    /// What is left in the hand.
    pub remainder: Option<&'static LazyLock<Item>>,
}

/// «Лупа»: shows the history of a block to any player.
pub static MAGNIFIER: CustomItem = CustomItem {
    id: "minehouse-tweaks:magnifier",
    base: &vanilla_items::PAPER,
    name: "Лупа",
    lore: &[
        "ЛКМ по блоку: кто поставил и сломал",
        "ПКМ по сундуку: кто положил и забрал",
        "ПКМ по грани: что было на месте рядом",
        "Ещё клик по тому же блоку: дальше",
    ],
    max_stack: 1,
    consume: None,
};

/// «Облачко»: the cloud block in a hand.
pub static CLOUD: CustomItem = CustomItem {
    id: "minehouse-tweaks:cloud",
    base: &vanilla_items::WHITE_WOOL,
    name: "Облачко",
    lore: &[],
    max_stack: 64,
    consume: None,
};

/// «Станция метро»: the station block in a hand.
pub static STATION: CustomItem = CustomItem {
    id: "minehouse-tweaks:metro_station",
    base: &vanilla_items::WAXED_CHISELED_COPPER,
    name: "Станция метро",
    lore: &[],
    max_stack: 64,
    consume: None,
};

/// «Стрелка метро»: the junction block in a hand.
pub static JUNCTION: CustomItem = CustomItem {
    id: "minehouse-tweaks:metro_junction",
    base: &vanilla_items::WAXED_CUT_COPPER,
    name: "Стрелка метро",
    lore: &[],
    max_stack: 64,
    consume: None,
};

/// «Карта MineHouse»: pays at terminals; what the site said about it is in its custom data.
pub static CARD: CustomItem = CustomItem {
    id: "minehouse-bank:card",
    base: &vanilla_items::PAPER,
    name: "Карта MineHouse",
    lore: &[],
    max_stack: 1,
    consume: None,
};

/// «Банкомат»: the ATM block in a hand.
pub static ATM: CustomItem = CustomItem {
    id: "minehouse-bank:atm",
    base: &vanilla_items::LECTERN,
    name: "Банкомат",
    lore: &[],
    max_stack: 64,
    consume: None,
};

/// «Терминал оплаты»: the terminal block in a hand.
pub static TERMINAL: CustomItem = CustomItem {
    id: "minehouse-bank:terminal",
    base: &vanilla_items::STONE_BUTTON,
    name: "Терминал оплаты",
    lore: &[],
    max_stack: 64,
    consume: None,
};

/// «Семена магического растения»: fall from the wild plant and from the crop, and sow it.
pub static SEEDS: CustomItem = CustomItem {
    id: "minehouse-herbs:magic_seeds",
    base: &vanilla_items::BEETROOT_SEEDS,
    name: "Семена магического растения",
    lore: &["Сажать на вспаханную землю"],
    max_stack: 64,
    consume: None,
};

/// «Магический лист»: what the magic herb gives.
pub static LEAF: CustomItem = CustomItem {
    id: "minehouse-herbs:magic_leaf",
    base: &vanilla_items::PAPER,
    name: "Магический лист",
    lore: &[
        "10 листьев в котёл с 3 вёдрами молока —",
        "магический отвар",
    ],
    max_stack: 64,
    consume: None,
};

/// «Блок магических листьев»: nine leaves in one block.
pub static LEAF_BLOCK: CustomItem = CustomItem {
    id: "minehouse-herbs:magic_leaf_block",
    base: &vanilla_items::WARPED_WART_BLOCK,
    name: "Блок магических листьев",
    lore: &["9 магических листьев,", "на верстаке разбирается обратно"],
    max_stack: 64,
    consume: None,
};

/// «Магический отвар»: a bowl out of the cauldron of broth.
pub static BROTH: CustomItem = CustomItem {
    id: "minehouse-herbs:magic_broth",
    base: &vanilla_items::MUSHROOM_STEW,
    name: "Магический отвар",
    lore: &["+4 сердца, замедление и левитация"],
    max_stack: 1,
    consume: Some(Consume {
        seconds: 1.6,
        animation: ItemUseAnimation::Drink,
        sound: &sound_events::ENTITY_GENERIC_DRINK,
        remainder: Some(&vanilla_items::BOWL),
    }),
};

/// «Магическая папироса»: two magic leaves rolled in paper.
pub static PAPIROSA: CustomItem = CustomItem {
    id: "minehouse-herbs:magic_papirosa",
    base: &vanilla_items::PAPER,
    name: "Магическая папироса",
    lore: &[
        "+2 сердца и замедление",
        "Прикуривать с огнивом во второй руке",
        "Больше трёх подряд — поплохеет",
    ],
    max_stack: 16,
    consume: Some(Consume {
        seconds: papirosa::SMOKE_SECONDS,
        animation: ItemUseAnimation::TootHorn,
        // The sound of a consumable repeats every 4 ticks; the puffs make their own.
        sound: &sound_events::INTENTIONALLY_EMPTY,
        remainder: None,
    }),
};

/// Every item of ours.
pub static ITEMS: &[&CustomItem] = &[
    &MAGNIFIER,
    &CLOUD,
    &STATION,
    &JUNCTION,
    &SEEDS,
    &LEAF,
    &LEAF_BLOCK,
    &BROTH,
    &PAPIROSA,
    &CARD,
    &ATM,
    &TERMINAL,
];

static ATM_ITEM: blocks::PlacesBlock = blocks::PlacesBlock(&ATM_BLOCK);
static TERMINAL_ITEM: blocks::PlacesBlock = blocks::PlacesBlock(&TERMINAL_BLOCK);
static STATION_ITEM: blocks::PlacesBlock = blocks::PlacesBlock(&metro::STATION_BLOCK);
static JUNCTION_ITEM: blocks::PlacesBlock = blocks::PlacesBlock(&metro::JUNCTION_BLOCK);
static LEAF_BLOCK_ITEM: blocks::PlacesBlock = blocks::PlacesBlock(&herbs::LEAF_BLOCK);
static SEEDS_ITEM: blocks::PlacesBlock = blocks::PlacesBlock(&herbs::CROP_BLOCK);

/// What using `stack` does when it is an item of ours that does not behave as its vanilla
/// item: clouds are white wool to the client, and must not be placed as wool.
fn use_behavior(stack: &ItemStack) -> Option<&'static dyn ItemBehavior> {
    let Some(id) = id_of(stack) else {
        return stack
            .is(&vanilla_items::MILK_BUCKET)
            .then_some(&cauldron::MilkBucket);
    };
    if id == CLOUD.id {
        Some(&cloud::CLOUD_ITEM)
    } else if id == STATION.id {
        Some(&STATION_ITEM)
    } else if id == JUNCTION.id {
        Some(&JUNCTION_ITEM)
    } else if id == SEEDS.id {
        Some(&SEEDS_ITEM)
    } else if id == LEAF_BLOCK.id {
        Some(&LEAF_BLOCK_ITEM)
    } else if id == BROTH.id {
        Some(&cauldron::Broth)
    } else if id == PAPIROSA.id {
        Some(&papirosa::Papirosa)
    } else if id == CARD.id {
        Some(&guides::Opener)
    } else if id == ATM.id {
        Some(&ATM_ITEM)
    } else if id == TERMINAL.id {
        Some(&TERMINAL_ITEM)
    } else {
        None
    }
}

impl Consume {
    fn apply(&self, stack: &mut ItemStack) {
        let sound = SoundEventHolder::Registry(self.sound);
        if let Ok(consumable) = Consumable::new(self.seconds, self.animation, sound, false, vec![])
        {
            stack.set(CONSUMABLE, consumable);
        }
        if let Some(remainder) = self.remainder {
            let left = ItemStackTemplate::new(LazyLock::force(remainder));
            stack.set(USE_REMAINDER, UseRemainder::new(left));
        }
    }
}

impl CustomItem {
    /// The part of the id after the namespace: `magnifier`.
    #[must_use]
    pub fn short_id(&self) -> &'static str {
        self.id.split_once(':').map_or(self.id, |(_, path)| path)
    }

    /// A stack of `count` of this item.
    #[must_use]
    pub fn stack(&self, count: i32) -> ItemStack {
        let mut stack = ItemStack::with_count(LazyLock::force(self.base), count);
        stack.set(ITEM_NAME, TextComponent::plain(self.name));
        let guide = guides::lore_of(self);
        let lines: Vec<&str> = self.lore.iter().copied().chain(guide.as_deref()).collect();
        if !lines.is_empty()
            && let Ok(lore) = ItemLore::new(lines.into_iter().map(lore_line).collect())
        {
            stack.set(LORE, lore);
        }
        if let Ok(model) = Identifier::from_str(self.id) {
            stack.set(ITEM_MODEL, model);
        }
        stack.set(MAX_STACK_SIZE, self.max_stack);
        // The base item may be food (the broth is a mushroom stew to the client): an
        // item of ours is consumed only the way it says.
        stack.remove(FOOD);
        stack.remove(CONSUMABLE);
        stack.remove(USE_REMAINDER);
        if let Some(consume) = &self.consume {
            consume.apply(&mut stack);
        }
        let mut tag = NbtCompound::new();
        tag.insert(ID_KEY, self.id);
        if let Some(data) = CustomData::try_from_compound(tag) {
            stack.set(CUSTOM_DATA, data);
        }
        stack
    }

    /// This item as an ingredient of a recipe: the base item does not do.
    #[must_use]
    pub fn ingredient(&self) -> Ingredient {
        Ingredient::Extension {
            item: self.base,
            id: self.id,
        }
    }

    /// Whether `stack` is this item.
    #[must_use]
    pub fn is(&self, stack: &ItemStack) -> bool {
        id_of(stack).as_deref() == Some(self.id)
    }
}

/// A grey line that is not slanted, unlike a lore line left to the client.
fn lore_line(text: &str) -> TextComponent {
    let mut line = render("&7{text}", &[("text", text)]);
    line.format = line.format.italic(false);
    line
}

/// The id of the item of ours `stack` is, if it is one.
#[must_use]
pub fn id_of(stack: &ItemStack) -> Option<String> {
    stack
        .get(CUSTOM_DATA)?
        .as_compound()
        .string(ID_KEY)
        .map(|id| id.to_str().into_owned())
}

/// The id of the item of ours `stack` is, for recipes: an item of ours is not the vanilla
/// item it is made of, so a magnifier is not paper for a book.
fn recipe_id(stack: &ItemStack) -> Option<&'static str> {
    let id = id_of(stack)?;
    ITEMS.iter().find(|item| item.id == id).map(|item| item.id)
}

/// The item of ours with this id, full or short.
#[must_use]
pub fn by_id(id: &str) -> Option<&'static CustomItem> {
    ITEMS
        .iter()
        .copied()
        .find(|item| item.id == id || item.short_id() == id)
}

/// Registers `/mhgive`, and our blocks and recipes if the registry is not built yet (it is
/// not when the server starts; it may be in a test that built it first).
pub fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    if !extend_registry(blocks::register) || !extend_block_behaviors(blocks::register_behaviors) {
        log::warn!("MineHouse blocks: the registry is already built, our blocks are off");
    } else if !set_item_behavior_override(use_behavior) {
        log::warn!("MineHouse blocks: another extension decides what our items do when used");
    }
    if !set_extension_item_id(recipe_id) {
        log::warn!("MineHouse items: another extension tells items apart for recipes");
    }
    if !extend_registry(recipes::register) {
        log::warn!("MineHouse items: the registry is already built, our recipes are off");
    }
    ride::register(registry)?;
    nametags::register(registry)?;
    admin_mode::register(registry)?;
    see::register(registry)?;
    transfer::register(registry)?;
    zones::commands::register(registry)?;
    banker::commands::register(registry)?;
    guides::register(registry)?;
    commands::register(registry)
}

/// Starts what runs on the server tick. Call once the server is ready for players.
pub fn start_jobs(server: &Server) {
    first_join::start(server);
    afk::start(server);
    nametags::start(server);
    end_lock::start(server);
    all_recipes::start(server);
    admin_mode::start(server);
    see::start(server);
    guides::start(server);
}

/// Says in the log what of ours the built registry holds.
pub fn start() {
    log::info!(
        "MineHouse items on: {} items, {} of {} recipes",
        ITEMS.len(),
        recipes::registered() + waxed_items::registered(),
        recipes::KEYS.len() + waxed_items::RECIPES
    );
    if !set_minecart_observer(&ride::RIDE) {
        log::warn!("MineHouse metro: minecarts are already observed, the track is not learned");
    }
    if !set_player_input_observer(&station_menu::INPUT) {
        log::warn!("MineHouse metro: player input is already observed, the map takes no clicks");
    }
    log::info!(
        "MineHouse metro on: {} stations, {} runs of track, {} coupled carts",
        stations::with(|stations| stations.count()),
        rail_graph::with(|graphs| graphs.count()) / 2,
        trains::count()
    );
    map_markers::changed();
    if !set_sleep_observer(&afk::AFK_SLEEP) {
        log::warn!("MineHouse AFK: sleep is already observed, AFK players count as awake");
    }
    if !set_team_packet_observer(&nametags::NAMETAGS) {
        log::warn!("MineHouse nametags: team packets are already observed, names can't be hidden");
    }
    if !set_player_death_observer(&admin_mode::DEATH) {
        log::warn!("MineHouse admin mode: deaths are already observed, admin items drop on death");
    }
    if !set_leaf_decay_observer(&leaf_decay::LEAF_DECAY) {
        log::warn!("MineHouse leaves: leaf decay is already observed, leaves fall as in vanilla");
    }
    if !set_portal_shape_observer(&any_shape_portals::ANY_SHAPE_PORTALS) {
        log::warn!("MineHouse portals: portal shapes are already observed, only rectangles light");
    }
    if !set_item_entity_observer(&cauldron_conversion::CAULDRON_CONVERSION) {
        log::warn!("MineHouse cauldron: items are already observed, nothing is washed");
    }
    if !set_experience_orb_observer(&xp_orbs::XP_ORBS) {
        log::warn!("MineHouse orbs: experience orbs are already observed, they do not merge");
    }
    if !set_craft_observer(&waxed_items::WAXED_ITEMS) {
        log::warn!("MineHouse wax: crafting is already observed, waxed items can be copied");
    }
    if !set_item_frame_observer(&waxed_frames::WAXED_FRAMES) {
        log::warn!("MineHouse: item frames are already observed, waxed frames are off");
    }
    if !set_name_tag_observer(&silent_tags::SILENT_TAGS) {
        log::warn!("MineHouse: name tags are already observed, silent name tags are off");
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use super::*;

    #[test]
    fn a_stack_of_ours_is_a_vanilla_item_that_knows_its_id() {
        init_vanilla_registry();
        let stack = MAGNIFIER.stack(1);
        assert!(stack.is(&vanilla_items::PAPER));
        assert_eq!(stack.max_stack_size(), 1);
        assert_eq!(id_of(&stack).as_deref(), Some("minehouse-tweaks:magnifier"));
        assert_eq!(
            stack.get(ITEM_MODEL).map(ToString::to_string).as_deref(),
            Some("minehouse-tweaks:magnifier")
        );
        assert_eq!(stack.get(LORE).map(|lore| lore.lines().len()), Some(4));
        assert!(MAGNIFIER.is(&stack));
        // Plain paper is not a magnifier.
        assert!(!MAGNIFIER.is(&ItemStack::with_count(&vanilla_items::PAPER, 1)));
        assert_eq!(
            id_of(&ItemStack::with_count(&vanilla_items::PAPER, 1)),
            None
        );
    }

    #[test]
    fn an_item_of_ours_is_not_its_vanilla_item_in_a_recipe() {
        init_vanilla_registry();
        set_extension_item_id(recipe_id);
        let paper = ItemStack::with_count(&vanilla_items::PAPER, 1);
        let magnifier = MAGNIFIER.stack(1);
        let cloud = CLOUD.stack(1);

        // Vanilla ingredients take the vanilla item only: no book out of magnifiers, no bed
        // out of clouds.
        let plain = Ingredient::Item(&vanilla_items::PAPER);
        assert!(plain.test(&paper));
        assert!(!plain.test(&magnifier));
        let wool = Ingredient::Tag(Identifier::vanilla_static("wool"));
        assert!(wool.test(&ItemStack::with_count(&vanilla_items::WHITE_WOOL, 1)));
        assert!(!wool.test(&cloud));
        let choice: &'static [&'static Item] = Box::leak(Box::new([&*vanilla_items::WHITE_WOOL]));
        assert!(!Ingredient::Choice(choice).test(&cloud));

        // Ours take our item only.
        let ours = MAGNIFIER.ingredient();
        assert!(ours.test(&magnifier));
        assert!(!ours.test(&paper));
        assert!(!ours.test(&cloud));
        assert!(!ours.test(&ItemStack::empty()));
        assert!(ours.eq_ingredient(&MAGNIFIER.ingredient()));
        assert!(!ours.eq_ingredient(&CLOUD.ingredient()));
        // The recipe book of the client shows the vanilla item.
        assert_eq!(ours.get_items().len(), 1);
    }

    #[test]
    fn items_are_found_by_full_and_short_id() {
        assert!(by_id("minehouse-tweaks:magnifier").is_some());
        assert!(by_id("magnifier").is_some());
        assert!(by_id("telescope").is_none());
    }
}
