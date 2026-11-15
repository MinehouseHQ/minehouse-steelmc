//! Vanilla `SlotRange` and `SlotRanges`: the named command slots, such as
//! `container.5`, `armor.*` or `player.cursor`, and the slot ids they cover.

use std::sync::LazyLock;

use rustc_hash::FxHashMap;

/// Where a mob's own inventory starts (vanilla `MOB_INVENTORY_SLOT_OFFSET`).
pub const MOB_INVENTORY_SLOT_OFFSET: i32 = 300;
/// How many slots `mob.inventory.*` names (vanilla `MOB_INVENTORY_SIZE`).
pub const MOB_INVENTORY_SIZE: i32 = 8;
/// Where an equine's or nautilus's inventory starts.
pub const HORSE_INVENTORY_SLOT_OFFSET: i32 = 500;
/// The ender chest's first slot.
pub const ENDER_CHEST_SLOT_OFFSET: i32 = 200;
/// A player's cursor and a chested equine's chest share this id.
pub const CURSOR_OR_HORSE_CHEST_SLOT: i32 = 499;
/// A player's 2x2 crafting grid starts here.
pub const PLAYER_CRAFTING_SLOT_OFFSET: i32 = 500;

/// Vanilla `EquipmentSlot.getIndex(98)` for the hands.
pub const MAINHAND_SLOT: i32 = 98;
/// The off hand, one after the main hand.
pub const OFFHAND_SLOT: i32 = 99;
/// Vanilla `EquipmentSlot.getIndex(100)` for the feet.
pub const FEET_SLOT: i32 = 100;
/// The legs armor slot.
pub const LEGS_SLOT: i32 = 101;
/// The chest armor slot.
pub const CHEST_SLOT: i32 = 102;
/// The head armor slot.
pub const HEAD_SLOT: i32 = 103;
/// Vanilla `EquipmentSlot.BODY.getIndex(105)`.
pub const BODY_SLOT: i32 = 105;
/// Vanilla `EquipmentSlot.SADDLE.getIndex(106)`.
pub const SADDLE_SLOT: i32 = 106;

/// One named slot range.
#[derive(Debug, PartialEq, Eq)]
pub struct SlotRange {
    name: String,
    slots: Vec<i32>,
}

impl SlotRange {
    /// The name commands and data use, such as `hotbar.*`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The slot ids, in vanilla's order.
    #[must_use]
    pub fn slots(&self) -> &[i32] {
        &self.slots
    }

    /// Vanilla `SlotRange.size`.
    #[must_use]
    pub const fn size(&self) -> usize {
        self.slots.len()
    }
}

struct SlotRanges {
    ranges: Vec<SlotRange>,
    by_name: FxHashMap<String, usize>,
}

impl SlotRanges {
    fn single(&mut self, name: &str, slot: i32) {
        self.slots(name, &[slot]);
    }

    fn slots(&mut self, name: &str, slots: &[i32]) {
        self.by_name.insert(name.to_owned(), self.ranges.len());
        self.ranges.push(SlotRange {
            name: name.to_owned(),
            slots: slots.to_vec(),
        });
    }

    fn range(&mut self, prefix: &str, offset: i32, size: i32) {
        let all: Vec<i32> = (offset..offset + size).collect();
        for (index, slot) in all.iter().enumerate() {
            self.single(&format!("{prefix}{index}"), *slot);
        }
        self.slots(&format!("{prefix}*"), &all);
    }
}

/// Vanilla `SlotRanges.SLOTS`, built in the same order.
static SLOT_RANGES: LazyLock<SlotRanges> = LazyLock::new(|| {
    let mut ranges = SlotRanges {
        ranges: Vec::new(),
        by_name: FxHashMap::default(),
    };
    ranges.single("contents", 0);
    ranges.range("container.", 0, 54);
    ranges.range("hotbar.", 0, 9);
    ranges.range("inventory.", 9, 27);
    ranges.range("enderchest.", ENDER_CHEST_SLOT_OFFSET, 27);
    ranges.range(
        "mob.inventory.",
        MOB_INVENTORY_SLOT_OFFSET,
        MOB_INVENTORY_SIZE,
    );
    ranges.range("horse.", HORSE_INVENTORY_SLOT_OFFSET, 15);
    ranges.single("weapon", MAINHAND_SLOT);
    ranges.single("weapon.mainhand", MAINHAND_SLOT);
    ranges.single("weapon.offhand", OFFHAND_SLOT);
    ranges.slots("weapon.*", &[MAINHAND_SLOT, OFFHAND_SLOT]);
    ranges.single("armor.head", HEAD_SLOT);
    ranges.single("armor.chest", CHEST_SLOT);
    ranges.single("armor.legs", LEGS_SLOT);
    ranges.single("armor.feet", FEET_SLOT);
    ranges.single("armor.body", BODY_SLOT);
    ranges.slots(
        "armor.*",
        &[HEAD_SLOT, CHEST_SLOT, LEGS_SLOT, FEET_SLOT, BODY_SLOT],
    );
    ranges.single("saddle", SADDLE_SLOT);
    ranges.single("horse.chest", CURSOR_OR_HORSE_CHEST_SLOT);
    ranges.single("player.cursor", CURSOR_OR_HORSE_CHEST_SLOT);
    ranges.range("player.crafting.", PLAYER_CRAFTING_SLOT_OFFSET, 4);
    ranges
});

/// Vanilla `SlotRanges.nameToIds`.
#[must_use]
pub fn slot_range(name: &str) -> Option<&'static SlotRange> {
    let ranges = &*SLOT_RANGES;
    ranges.by_name.get(name).map(|&index| &ranges.ranges[index])
}

/// Vanilla `SlotRanges.allNames`, in vanilla's order.
pub fn all_slot_range_names() -> impl Iterator<Item = &'static str> {
    SLOT_RANGES.ranges.iter().map(SlotRange::name)
}

/// Vanilla `SlotRanges.singleSlotNames`.
pub fn single_slot_names() -> impl Iterator<Item = &'static str> {
    SLOT_RANGES
        .ranges
        .iter()
        .filter(|range| range.size() == 1)
        .map(SlotRange::name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_resolve_to_vanilla_slot_ids() {
        assert_eq!(slot_range("contents").map(SlotRange::slots), Some(&[0][..]));
        assert_eq!(
            slot_range("container.53").map(SlotRange::slots),
            Some(&[53][..])
        );
        assert_eq!(
            slot_range("inventory.0").map(SlotRange::slots),
            Some(&[9][..])
        );
        assert_eq!(
            slot_range("enderchest.26").map(SlotRange::slots),
            Some(&[226][..])
        );
        assert_eq!(
            slot_range("horse.14").map(SlotRange::slots),
            Some(&[514][..])
        );
        assert_eq!(
            slot_range("armor.*").map(SlotRange::slots),
            Some(&[103, 102, 101, 100, 105][..])
        );
        assert_eq!(
            slot_range("weapon.*").map(SlotRange::slots),
            Some(&[98, 99][..])
        );
        assert_eq!(
            slot_range("player.crafting.3").map(SlotRange::slots),
            Some(&[503][..])
        );
        assert_eq!(slot_range("hotbar.*").map(SlotRange::size), Some(9));
        assert!(slot_range("hotbar.9").is_none());
        assert!(slot_range("armor").is_none());
    }

    #[test]
    fn names_keep_vanilla_order() {
        let names: Vec<_> = all_slot_range_names().take(3).collect();
        assert_eq!(names, ["contents", "container.0", "container.1"]);
        assert_eq!(all_slot_range_names().last(), Some("player.crafting.*"));
        assert!(single_slot_names().all(|name| !name.ends_with('*') || name == "weapon"));
        assert!(single_slot_names().any(|name| name == "saddle"));
    }
}
