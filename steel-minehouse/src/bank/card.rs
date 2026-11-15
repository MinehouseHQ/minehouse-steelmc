//! «Карта `MineHouse`». Carries the card id from the site in custom data; the site decides
//! whether it is still valid and whose it is, so a card picked up by someone else pays for
//! nothing (the Fabric `Card`).

use serde_json::Value;
use simdnbt::owned::NbtCompound;
use steel_core::player::Player;
use steel_registry::{
    data_components::{
        CustomData,
        vanilla_components::{CUSTOM_DATA, ItemLore, LORE},
    },
    item_stack::ItemStack,
};
use text_components::format::Color;

use super::gui::line;
use crate::content::{CARD, ID_KEY};

/// The key in `custom_data` that holds what the site said about the card.
const KEY: &str = "minehouse_bank_card";

/// A card stack from the site's `card` object and the owner's nickname. `account` is the
/// name of a non-personal account, shown on the card.
#[must_use]
pub fn make(card: &Value, owner: &str, account: Option<&str>) -> Option<ItemStack> {
    let id = card.get("id")?.as_str()?;
    let number = card.get("number")?.as_str()?;
    let tier = card
        .get("tier_title")
        .and_then(Value::as_str)
        .unwrap_or("Классическая");

    let mut stack = CARD.stack(1);
    let mut data = NbtCompound::new();
    data.insert("id", id);
    data.insert("number", number);
    data.insert("owner", owner);
    let mut tag = NbtCompound::new();
    tag.insert(ID_KEY, CARD.id);
    tag.insert(KEY, data);
    stack.set(CUSTOM_DATA, CustomData::try_from_compound(tag)?);

    let account = account.map_or_else(String::new, |name| format!(" · счёт «{name}»"));
    if let Ok(lore) = ItemLore::new(vec![
        line(number, Color::Aqua),
        line(&format!("{tier} · {owner}{account}"), Color::Gray),
        line("ПКМ по терминалу: оплатить", Color::DarkGray),
    ]) {
        stack.set(LORE, lore);
    }
    Some(stack)
}

fn field(stack: &ItemStack, name: &str) -> Option<String> {
    if !CARD.is(stack) {
        return None;
    }
    let data = stack.get(CUSTOM_DATA)?;
    let card = data.as_compound().compound(KEY)?;
    card.string(name).map(|text| text.to_str().into_owned())
}

/// The id of the card `stack` is, if it is one.
#[must_use]
pub fn id(stack: &ItemStack) -> Option<String> {
    field(stack, "id")
}

/// The card number printed on the stack: «MH 1234 5678».
#[must_use]
pub fn number(stack: &ItemStack) -> String {
    field(stack, "number").unwrap_or_default()
}

/// The stacks a player pays from: the main hand first, then the 36 slots.
fn carried(player: &Player) -> Vec<ItemStack> {
    let inventory = player.inventory.lock();
    let mut stacks = vec![inventory.get_selected_item().clone()];
    stacks.extend(inventory.get_items().iter().cloned());
    stacks
}

/// Every distinct card among `stacks`, in their order.
fn distinct(stacks: Vec<ItemStack>) -> Vec<ItemStack> {
    let mut seen = Vec::new();
    stacks
        .into_iter()
        .filter(|stack| {
            let Some(id) = id(stack) else {
                return false;
            };
            if seen.contains(&id) {
                return false;
            }
            seen.push(id);
            true
        })
        .collect()
}

/// Every distinct card the player carries, the one in the main hand first.
#[must_use]
pub fn all(player: &Player) -> Vec<ItemStack> {
    distinct(carried(player))
}

/// The card in the main hand, or else (unless `hand_only`) the first card in the inventory.
#[must_use]
pub fn find(player: &Player, hand_only: bool) -> Option<String> {
    let stacks = carried(player);
    let hand = stacks.first().and_then(id);
    if hand.is_some() || hand_only {
        return hand;
    }
    stacks.iter().find_map(id)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use steel_registry::vanilla_items;
    use text_components::resolving::NoResolutor;

    use super::*;

    fn card(id: &str) -> ItemStack {
        make(
            &json!({"id": id, "number": "MH 1234 5678", "tier_title": "Золотая"}),
            "Steve",
            None,
        )
        .unwrap_or_else(|| panic!("a card is made"))
    }

    fn lore(stack: &ItemStack) -> Vec<String> {
        stack
            .get(LORE)
            .into_iter()
            .flat_map(ItemLore::lines)
            .map(|line| line.to_plain(&NoResolutor))
            .collect()
    }

    #[test]
    fn a_card_carries_what_the_site_said() {
        steel_registry::init_vanilla_registry();
        let stack = card("c-1");
        assert!(CARD.is(&stack));
        assert_eq!(id(&stack).as_deref(), Some("c-1"));
        assert_eq!(number(&stack), "MH 1234 5678");
        assert_eq!(
            lore(&stack),
            vec![
                "MH 1234 5678",
                "Золотая · Steve",
                "ПКМ по терминалу: оплатить"
            ]
        );
    }

    #[test]
    fn a_card_of_a_shared_account_names_it_and_the_tier_has_a_default() {
        steel_registry::init_vanilla_registry();
        let stack = make(
            &json!({"id": "c-2", "number": "MH 1"}),
            "Alex",
            Some("Магазин"),
        )
        .unwrap_or_else(|| panic!("a card is made"));
        assert_eq!(lore(&stack)[1], "Классическая · Alex · счёт «Магазин»");
        assert_eq!(make(&json!({"number": "MH 1"}), "Alex", None), None);
    }

    #[test]
    fn paper_is_not_a_card_and_cards_are_counted_once() {
        steel_registry::init_vanilla_registry();
        let paper = ItemStack::new(&vanilla_items::PAPER);
        assert_eq!(id(&paper), None);
        assert_eq!(number(&paper), "");
        let cards = distinct(vec![card("a"), paper, card("b"), card("a")]);
        let ids: Vec<Option<String>> = cards.iter().map(id).collect();
        assert_eq!(ids, vec![Some("a".to_owned()), Some("b".to_owned())]);
    }
}
