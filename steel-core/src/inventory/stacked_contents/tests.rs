use steel_registry::item_stack::ItemStack;
use steel_registry::recipe::{IngredientItem, PlacementInfo};
use steel_registry::{REGISTRY, init_vanilla_registry, vanilla_items};
use steel_utils::Identifier;

use super::{IngredientInfo, StackedContents, StackedItemContents};

/// An ingredient accepting the listed items.
struct Accepts(&'static [char]);

impl IngredientInfo<char> for Accepts {
    fn accepts_item(&self, item: char) -> bool {
        self.0.contains(&item)
    }
}

fn picked(contents: &mut StackedContents<char>, ingredients: &[Accepts], amount: i32) -> Vec<char> {
    let mut items = Vec::new();
    assert!(contents.try_pick(ingredients, amount, Some(&mut |item| items.push(item))));
    items
}

#[test]
fn each_ingredient_takes_a_different_item_and_reassigns_to_fit() {
    let mut contents = StackedContents::default();
    contents.account('x', 1);
    contents.account('y', 1);
    // `x` goes to the first ingredient first, then moves to the second so both fit.
    let ingredients = [Accepts(&['x', 'y']), Accepts(&['x'])];
    assert_eq!(picked(&mut contents, &ingredients, 1), ['y', 'x']);

    let both_want_x = [Accepts(&['x']), Accepts(&['x'])];
    assert!(!contents.try_pick(&both_want_x, 1, None));
    contents.account('x', 1);
    assert!(contents.try_pick(&both_want_x, 1, None));
}

#[test]
fn picking_leaves_the_counts_as_they_were() {
    let mut contents = StackedContents::default();
    contents.account('x', 3);
    let ingredients = [Accepts(&['x'])];
    assert!(contents.try_pick(&ingredients, 3, None));
    assert!(!contents.try_pick(&ingredients, 4, None));
    assert_eq!(contents.result_upper_bound(&ingredients), 3);
    assert!(contents.try_pick(&ingredients, 0, None));
}

#[test]
fn pick_all_finds_the_largest_amount() {
    let mut contents = StackedContents::default();
    contents.account('x', 7);
    contents.account('y', 2);
    let ingredients = [Accepts(&['x']), Accepts(&['x', 'y'])];
    // Each ingredient takes the whole amount from one item: three `x` each fits in
    // seven, while four would need a second item with four.
    let mut items = Vec::new();
    assert_eq!(
        contents.try_pick_all(&ingredients, i32::MAX, Some(&mut |item| items.push(item))),
        3
    );
    assert_eq!(items, ['x', 'x']);
    assert_eq!(contents.try_pick_all(&ingredients, 2, None), 2);
    assert_eq!(contents.try_pick_all(&[Accepts(&['z'])], i32::MAX, None), 0);
}

#[test]
fn only_stacks_usable_for_crafting_count_as_simple_stacks() {
    init_vanilla_registry();
    let mut contents = StackedItemContents::default();
    contents.account_simple_stack(&ItemStack::with_count(&vanilla_items::OAK_PLANKS, 3));
    let mut damaged = ItemStack::new(&vanilla_items::IRON_PICKAXE);
    damaged.set_damage_value(5);
    contents.account_simple_stack(&damaged);

    let crafting_table = REGISTRY
        .recipes
        .by_key(&Identifier::vanilla_static("crafting_table"))
        .expect("vanilla crafting table recipe");
    let placement: &PlacementInfo = crafting_table
        .data()
        .properties()
        .and_then(|properties| properties.placement.as_ref())
        .expect("crafting table has a placement");
    assert!(!contents.can_craft(placement, 1, None));
    contents.account_stack(&ItemStack::new(&vanilla_items::OAK_PLANKS));
    let mut items = Vec::new();
    assert!(contents.can_craft(placement, 1, Some(&mut |item| items.push(item))));
    assert_eq!(
        items,
        [IngredientItem::of(&ItemStack::new(&vanilla_items::OAK_PLANKS)); 4]
    );
    assert_eq!(
        contents.biggest_craftable_stack(placement, i32::MAX, None),
        1
    );
}
