//! АР: diamond ore and deepslate diamond ore, one ore is one АР. Withdrawals hand out
//! deepslate diamond ore (the Fabric `Ores`).

use steel_core::player::{Player, player_inventory::Prediction};
use steel_registry::{item_stack::ItemStack, vanilla_items};

/// Whether `stack` is money.
#[must_use]
pub fn is_ore(stack: &ItemStack) -> bool {
    stack.is(&vanilla_items::DIAMOND_ORE) || stack.is(&vanilla_items::DEEPSLATE_DIAMOND_ORE)
}

/// How `amount` ores are split into stacks.
fn stacks(amount: i32) -> impl Iterator<Item = i32> {
    let full = amount.max(0) / 64;
    let rest = amount.max(0) % 64;
    (0..full).map(|_| 64).chain((rest > 0).then_some(rest))
}

/// Gives `amount` deepslate diamond ore; what doesn't fit drops at the player's feet.
pub fn give(player: &Player, amount: i32) {
    for count in stacks(amount) {
        give_stack(
            player,
            ItemStack::with_count(&vanilla_items::DEEPSLATE_DIAMOND_ORE, count),
        );
    }
}

/// How many items like `sample` the 36 slots `items` still take.
fn room(items: &[ItemStack], sample: &ItemStack) -> i32 {
    let max = sample.max_stack_size();
    items
        .iter()
        .map(|stack| {
            if stack.is_empty() {
                max
            } else if ItemStack::is_same_item_same_components(stack, sample) {
                (max.min(stack.max_stack_size()) - stack.count()).max(0)
            } else {
                0
            }
        })
        .sum()
}

/// Whether `count` items like `sample` fit into the main inventory (not armor or offhand).
#[must_use]
pub fn fits(player: &Player, sample: &ItemStack, count: i32) -> bool {
    room(player.inventory.lock().get_items(), sample) >= count
}

/// Inventory first, the rest is dropped.
pub fn give_stack(player: &Player, stack: ItemStack) {
    player.add_item_or_drop(stack, Prediction::ServerOnly);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ores_are_split_into_full_stacks_and_a_rest() {
        assert_eq!(stacks(0).collect::<Vec<_>>(), Vec::<i32>::new());
        assert_eq!(stacks(-5).collect::<Vec<_>>(), Vec::<i32>::new());
        assert_eq!(stacks(64).collect::<Vec<_>>(), vec![64]);
        assert_eq!(stacks(130).collect::<Vec<_>>(), vec![64, 64, 2]);
    }

    #[test]
    fn both_diamond_ores_are_money_and_nothing_else() {
        steel_registry::init_vanilla_registry();
        assert!(is_ore(&ItemStack::new(&vanilla_items::DIAMOND_ORE)));
        assert!(is_ore(&ItemStack::new(
            &vanilla_items::DEEPSLATE_DIAMOND_ORE
        )));
        assert!(!is_ore(&ItemStack::new(&vanilla_items::DIAMOND)));
    }

    #[test]
    fn room_counts_empty_slots_and_stacks_of_the_same_item() {
        steel_registry::init_vanilla_registry();
        let ore = ItemStack::new(&vanilla_items::DEEPSLATE_DIAMOND_ORE);
        let items = vec![
            ItemStack::empty(),
            ItemStack::with_count(&vanilla_items::DEEPSLATE_DIAMOND_ORE, 60),
            ItemStack::with_count(&vanilla_items::DIAMOND_ORE, 1),
            ItemStack::with_count(&vanilla_items::DEEPSLATE_DIAMOND_ORE, 64),
        ];
        assert_eq!(room(&items, &ore), 68);
    }
}
