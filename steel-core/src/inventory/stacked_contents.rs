//! Vanilla `StackedContents` and `StackedItemContents`: counts of the items a player
//! has, and whether, and how many times, a recipe's ingredients can be taken from them.

use steel_registry::data_components::vanilla_components::CUSTOM_NAME;
use steel_registry::item_stack::ItemStack;
use steel_registry::recipe::{Ingredient, IngredientItem, PlacementInfo};

/// Vanilla `StackedContents.IngredientInfo`: one ingredient slot of a recipe.
pub trait IngredientInfo<T> {
    /// Whether `item` satisfies this ingredient.
    fn accepts_item(&self, item: T) -> bool;
}

impl IngredientInfo<IngredientItem> for Ingredient {
    fn accepts_item(&self, item: IngredientItem) -> bool {
        Self::accepts_item(self, item)
    }
}

/// Vanilla `Inventory.isUsableForCrafting`: undamaged, unenchanted and unnamed.
#[must_use]
pub fn is_usable_for_crafting(stack: &ItemStack) -> bool {
    !stack.is_damaged() && !stack.is_enchanted() && !stack.has(CUSTOM_NAME)
}

/// Vanilla `StackedContents`: how many of each item are available.
///
/// Vanilla keeps the counts in an identity hash map, which it walks in an order no
/// player can rely on; this keeps them in the order the items were first counted.
#[derive(Debug, Clone)]
pub struct StackedContents<T> {
    amounts: Vec<(T, i32)>,
}

impl<T> Default for StackedContents<T> {
    fn default() -> Self {
        Self {
            amounts: Vec::new(),
        }
    }
}

impl<T: Copy + Eq> StackedContents<T> {
    fn amount(&self, item: T) -> i32 {
        self.amounts
            .iter()
            .find(|(counted, _)| *counted == item)
            .map_or(0, |(_, amount)| *amount)
    }

    fn add_to(&mut self, item: T, delta: i32) -> i32 {
        if let Some((_, amount)) = self
            .amounts
            .iter_mut()
            .find(|(counted, _)| *counted == item)
        {
            let previous = *amount;
            *amount = amount.wrapping_add(delta);
            previous
        } else {
            self.amounts.push((item, delta));
            0
        }
    }

    fn has_at_least(&self, item: T, count: i32) -> bool {
        self.amount(item) >= count
    }

    fn take(&mut self, item: T, amount: i32) {
        let previous = self.add_to(item, -amount);
        assert!(
            previous >= amount,
            "Took {amount} items, but only had {previous}"
        );
    }

    fn put(&mut self, item: T, count: i32) {
        self.add_to(item, count);
    }

    /// Vanilla `StackedContents.tryPick`: whether every ingredient can take `amount`
    /// of a different item. `output` gets the item each ingredient took, in ingredient
    /// order, when they all can.
    pub fn try_pick<I: IngredientInfo<T>>(
        &mut self,
        ingredients: &[I],
        amount: i32,
        output: Option<&mut dyn FnMut(T)>,
    ) -> bool {
        let mut picker = RecipePicker::new(self, ingredients);
        picker.try_pick(self, amount, output)
    }

    /// Vanilla `StackedContents.tryPickAll`: how many times, up to `max_size`, the
    /// ingredients can all be picked, with `output` getting the items of that pick.
    pub fn try_pick_all<I: IngredientInfo<T>>(
        &mut self,
        ingredients: &[I],
        max_size: i32,
        output: Option<&mut dyn FnMut(T)>,
    ) -> i32 {
        let mut picker = RecipePicker::new(self, ingredients);
        picker.try_pick_all(self, max_size, output)
    }

    /// Vanilla `StackedContents.clear`.
    pub fn clear(&mut self) {
        self.amounts.clear();
    }

    /// Vanilla `StackedContents.account`.
    pub fn account(&mut self, item: T, count: i32) {
        self.put(item, count);
    }

    fn unique_available_ingredient_items<I: IngredientInfo<T>>(&self, ingredients: &[I]) -> Vec<T> {
        self.amounts
            .iter()
            .filter(|(item, amount)| {
                *amount > 0
                    && ingredients
                        .iter()
                        .any(|ingredient| ingredient.accepts_item(*item))
            })
            .map(|(item, _)| *item)
            .collect()
    }

    /// Vanilla `StackedContents.getResultUpperBound`: the smallest, over the
    /// ingredients, of the largest count of one item an ingredient accepts.
    #[must_use]
    pub fn result_upper_bound<I: IngredientInfo<T>>(&self, ingredients: &[I]) -> i32 {
        let mut min = i32::MAX;
        'ingredients: for ingredient in ingredients {
            let mut max = 0;
            for &(item, count) in &self.amounts {
                if count > max {
                    if ingredient.accepts_item(item) {
                        max = count;
                    }
                    if max >= min {
                        continue 'ingredients;
                    }
                }
            }
            min = max;
            if max == 0 {
                break;
            }
        }
        min
    }
}

/// Vanilla `StackedContents.RecipePicker`: a bipartite matching of ingredients to
/// items, grown one augmenting path at a time.
struct RecipePicker<'a, T, I> {
    ingredients: &'a [I],
    items: Vec<T>,
    visited_ingredients: Vec<bool>,
    visited_items: Vec<bool>,
    satisfied: Vec<bool>,
    connections: Vec<bool>,
    assigned: Vec<bool>,
    path: Vec<usize>,
}

impl<'a, T: Copy + Eq, I: IngredientInfo<T>> RecipePicker<'a, T, I> {
    fn new(contents: &StackedContents<T>, ingredients: &'a [I]) -> Self {
        let items = contents.unique_available_ingredient_items(ingredients);
        let pairs = ingredients.len() * items.len();
        let mut picker = Self {
            ingredients,
            visited_ingredients: vec![false; ingredients.len()],
            visited_items: vec![false; items.len()],
            satisfied: vec![false; ingredients.len()],
            connections: vec![false; pairs],
            assigned: vec![false; pairs],
            items,
            path: Vec::new(),
        };
        for (ingredient_index, ingredient) in ingredients.iter().enumerate() {
            for item in 0..picker.items.len() {
                if ingredient.accepts_item(picker.items[item]) {
                    let index = picker.pair_index(item, ingredient_index);
                    picker.connections[index] = true;
                }
            }
        }
        picker
    }

    const fn pair_index(&self, item: usize, ingredient: usize) -> usize {
        item * self.ingredients.len() + ingredient
    }

    fn is_assigned(&self, item: usize, ingredient: usize) -> bool {
        self.assigned[self.pair_index(item, ingredient)]
    }

    fn set_assigned(&mut self, item: usize, ingredient: usize, assigned: bool) {
        let index = self.pair_index(item, ingredient);
        debug_assert_ne!(self.assigned[index], assigned);
        self.assigned[index] = assigned;
    }

    fn has_connection(&self, item: usize, ingredient: usize) -> bool {
        self.connections[self.pair_index(item, ingredient)]
    }

    fn clear_all_visited(&mut self) {
        self.visited_ingredients.fill(false);
        self.visited_items.fill(false);
    }

    fn try_pick(
        &mut self,
        contents: &mut StackedContents<T>,
        capacity: i32,
        mut output: Option<&mut dyn FnMut(T)>,
    ) -> bool {
        if capacity <= 0 {
            return true;
        }
        let mut satisfied_ingredient_count = 0;
        while self.try_assigning_new_item(contents, capacity) {
            let path = self.path.clone();
            contents.take(self.items[path[0]], capacity);
            self.satisfied[path[path.len() - 1]] = true;
            satisfied_ingredient_count += 1;
            for i in 0..path.len() - 1 {
                if is_path_index_item(i) {
                    self.set_assigned(path[i], path[i + 1], true);
                } else {
                    self.set_assigned(path[i + 1], path[i], false);
                }
            }
        }

        let is_valid_assignment = satisfied_ingredient_count == self.ingredients.len();
        if !is_valid_assignment {
            output = None;
        }
        self.clear_all_visited();
        self.satisfied.fill(false);
        for ingredient in 0..self.ingredients.len() {
            if let Some(item) =
                (0..self.items.len()).find(|&item| self.is_assigned(item, ingredient))
            {
                self.set_assigned(item, ingredient, false);
                contents.put(self.items[item], capacity);
                if let Some(output) = output.as_mut() {
                    output(self.items[item]);
                }
            }
        }
        debug_assert!(!self.assigned.contains(&true));
        is_valid_assignment
    }

    /// Vanilla `tryAssigningNewItem`, leaving the augmenting path in `self.path`.
    fn try_assigning_new_item(&mut self, contents: &StackedContents<T>, capacity: i32) -> bool {
        self.clear_all_visited();
        (0..self.items.len()).any(|item| {
            contents.has_at_least(self.items[item], capacity)
                && self.find_new_item_assignment_path(item)
        })
    }

    /// Vanilla `findNewItemAssignmentPath`: a depth-first search, alternating items
    /// and ingredients, for an ingredient that is not satisfied yet.
    fn find_new_item_assignment_path(&mut self, starting_item: usize) -> bool {
        self.path.clear();
        self.visited_items[starting_item] = true;
        self.path.push(starting_item);
        while let Some(&last) = self.path.last() {
            let path_length = self.path.len();
            if is_path_index_item(path_length - 1) {
                if let Some(ingredient) = (0..self.ingredients.len()).find(|&ingredient| {
                    !self.visited_ingredients[ingredient]
                        && self.has_connection(last, ingredient)
                        && !self.is_assigned(last, ingredient)
                }) {
                    self.visited_ingredients[ingredient] = true;
                    self.path.push(ingredient);
                }
            } else {
                if !self.satisfied[last] {
                    return true;
                }
                if let Some(item) = (0..self.items.len())
                    .find(|&item| !self.visited_items[item] && self.is_assigned(item, last))
                {
                    debug_assert!(self.has_connection(item, last));
                    self.visited_items[item] = true;
                    self.path.push(item);
                }
            }
            if self.path.len() == path_length {
                self.path.pop();
            }
        }
        false
    }

    fn try_pick_all(
        &mut self,
        contents: &mut StackedContents<T>,
        max_size: i32,
        output: Option<&mut dyn FnMut(T)>,
    ) -> i32 {
        let mut min = 0_i32;
        // Vanilla's int arithmetic, overflow included.
        let mut max = max_size
            .min(contents.result_upper_bound(self.ingredients))
            .wrapping_add(1);
        loop {
            let mid = min.wrapping_add(max) / 2;
            if self.try_pick(contents, mid, None) {
                if max.wrapping_sub(min) <= 1 {
                    if mid > 0 {
                        self.try_pick(contents, mid, output);
                    }
                    return mid;
                }
                min = mid;
            } else {
                max = mid;
            }
        }
    }
}

const fn is_path_index_item(index: usize) -> bool {
    index & 1 == 0
}

/// Vanilla `StackedItemContents`: [`StackedContents`] over items.
#[derive(Debug, Clone, Default)]
pub struct StackedItemContents {
    raw: StackedContents<IngredientItem>,
}

impl StackedItemContents {
    /// Vanilla `accountSimpleStack`: counts `stack` if it may be used for crafting.
    pub fn account_simple_stack(&mut self, stack: &ItemStack) {
        if is_usable_for_crafting(stack) {
            self.account_stack(stack);
        }
    }

    /// Vanilla `accountStack(ItemStack)`.
    pub fn account_stack(&mut self, stack: &ItemStack) {
        self.account_stack_up_to(stack, stack.max_stack_size());
    }

    /// Vanilla `accountStack(ItemStack, int)`: counts up to `max_count` of `stack`.
    pub fn account_stack_up_to(&mut self, stack: &ItemStack, max_count: i32) {
        if !stack.is_empty() {
            self.raw
                .account(IngredientItem::of(stack), max_count.min(stack.count()));
        }
    }

    /// Vanilla `canCraft(Recipe, int, Output)`: whether the recipe's ingredients can
    /// be taken `amount` times, with `output` getting the item of each ingredient.
    pub fn can_craft(
        &mut self,
        placement: &PlacementInfo,
        amount: i32,
        output: Option<&mut dyn FnMut(IngredientItem)>,
    ) -> bool {
        !placement.is_impossible_to_place()
            && self.raw.try_pick(&placement.ingredients, amount, output)
    }

    /// Vanilla `getBiggestCraftableStack(Recipe, int, Output)`.
    pub fn biggest_craftable_stack(
        &mut self,
        placement: &PlacementInfo,
        max_size: i32,
        output: Option<&mut dyn FnMut(IngredientItem)>,
    ) -> i32 {
        self.raw
            .try_pick_all(&placement.ingredients, max_size, output)
    }

    /// Vanilla `StackedItemContents.clear`.
    pub fn clear(&mut self) {
        self.raw.clear();
    }
}

#[cfg(test)]
mod tests;
