//! Our own items on a vanilla client.
//!
//! On the Fabric server these were registry items that Polymer turned into vanilla ones on
//! the way to the client. Here an item of ours *is* a vanilla item: the base item with a name,
//! a lore, an item model of the server resource pack, and its id in `custom_data`. The id and
//! the model are the ones the Fabric mods used, so the same resource pack draws them.

mod blocks;
mod cloud;
mod commands;
mod recipes;

use std::{str::FromStr as _, sync::LazyLock};

use simdnbt::owned::NbtCompound;
use steel_core::{
    behavior::{ItemBehavior, extend_block_behaviors, set_item_behavior_override},
    command::{CommandRegistrationError, CommandRegistry},
};
use steel_registry::{
    data_components::{
        CustomData,
        vanilla_components::{CUSTOM_DATA, ITEM_MODEL, ITEM_NAME, ItemLore, LORE, MAX_STACK_SIZE},
    },
    extend_registry,
    item_stack::ItemStack,
    items::Item,
    vanilla_items,
};
use steel_utils::Identifier;
use text_components::TextComponent;

use crate::moderation::format::render;

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
};

/// «Облачко»: the cloud block in a hand.
pub static CLOUD: CustomItem = CustomItem {
    id: "minehouse-tweaks:cloud",
    base: &vanilla_items::WHITE_WOOL,
    name: "Облачко",
    lore: &[],
    max_stack: 64,
};

/// Every item of ours.
pub static ITEMS: &[&CustomItem] = &[&MAGNIFIER, &CLOUD];

/// What using `stack` does when it is an item of ours that does not behave as its vanilla
/// item: clouds are white wool to the client, and must not be placed as wool.
fn use_behavior(stack: &ItemStack) -> Option<&'static dyn ItemBehavior> {
    CLOUD
        .is(stack)
        .then_some(&cloud::CLOUD_ITEM as &dyn ItemBehavior)
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
        if !self.lore.is_empty()
            && let Ok(lore) = ItemLore::new(self.lore.iter().map(|line| lore_line(line)).collect())
        {
            stack.set(LORE, lore);
        }
        if let Ok(model) = Identifier::from_str(self.id) {
            stack.set(ITEM_MODEL, model);
        }
        stack.set(MAX_STACK_SIZE, self.max_stack);
        let mut tag = NbtCompound::new();
        tag.insert(ID_KEY, self.id);
        if let Some(data) = CustomData::try_from_compound(tag) {
            stack.set(CUSTOM_DATA, data);
        }
        stack
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
    if !extend_registry(recipes::register) {
        log::warn!("MineHouse items: the registry is already built, our recipes are off");
    }
    commands::register(registry)
}

/// Says in the log what of ours the built registry holds.
pub fn start() {
    log::info!(
        "MineHouse items on: {} items, {} of {} recipes",
        ITEMS.len(),
        recipes::registered(),
        recipes::KEYS.len()
    );
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
    fn items_are_found_by_full_and_short_id() {
        assert!(by_id("minehouse-tweaks:magnifier").is_some());
        assert!(by_id("magnifier").is_some());
        assert!(by_id("telescope").is_none());
    }
}
