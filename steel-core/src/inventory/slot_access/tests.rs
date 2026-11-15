use std::sync::{Arc, Weak};

use glam::DVec3;
use steel_registry::items::Item;
use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_items};
use steel_utils::locks::IntoShared as _;

use super::*;
use crate::behavior::init_behaviors;
use crate::entity::entities::{DonkeyEntity, ItemEntity, VillagerEntity, ZombieEntity};
use crate::entity::{Entity, LivingEntity as _};
use crate::inventory::container::SimpleContainer;
use crate::inventory::equipment::EquipmentSlot;
use crate::inventory::slot_range::slot_range;
use crate::test_support::{TestPlayerBuilder, fresh_test_world};

fn stack(item: &'static Item) -> ItemStack {
    ItemStack::new(item)
}

/// Every slot a named range resolves to on `entity`, like vanilla
/// `SlotProvider.getSlotsFromRange`.
fn range_slots<'a>(entity: &'a dyn Entity, name: &str) -> Vec<SlotAccess<'a>> {
    slot_range(name)
        .expect("vanilla slot range")
        .slots()
        .iter()
        .filter_map(|&slot| entity.get_slot(slot))
        .collect()
}

#[test]
fn container_slots_exist_only_inside_the_container() {
    init_vanilla_registry();
    let container = SyncMutex::new(SimpleContainer::new(2));
    assert!(SlotAccess::for_container(&container, 2).is_none());
    assert!(SlotAccess::for_container(&container, -1).is_none());
    let slot = SlotAccess::for_container(&container, 1).expect("slot 1 exists");
    assert!(slot.set(stack(&vanilla_items::STONE)));
    assert!(container.lock().get_item(1).is(&vanilla_items::STONE));
    assert!(slot.get().is(&vanilla_items::STONE));
}

#[test]
fn shared_container_slots_read_and_write_through_the_lock() {
    init_vanilla_registry();
    let shared = SimpleContainer::new(3).into_shared();
    let container = ContainerRef::from(Arc::clone(&shared));
    assert!(SlotAccess::for_container_ref(container.clone(), 3).is_none());
    let slot = SlotAccess::for_container_ref(container, 2).expect("slot 2 exists");
    assert!(slot.set(stack(&vanilla_items::DIRT)));
    assert!(shared.lock().get_item(2).is(&vanilla_items::DIRT));
    assert!(slot.get().is(&vanilla_items::DIRT));
}

#[test]
fn player_slots_cover_inventory_ender_chest_cursor_and_crafting() {
    init_vanilla_registry();
    init_behaviors();
    let fixture = fresh_test_world("slot_access_player");
    let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "Slots", 1).build();

    assert_eq!(range_slots(&*player, "container.*").len(), 36);
    assert_eq!(range_slots(&*player, "enderchest.*").len(), 27);
    // Slots 36..54 are only a container's, not a player's.
    assert!(player.get_slot(40).is_none());

    let hotbar = player.get_slot(0).expect("hotbar.0");
    assert!(hotbar.set(stack(&vanilla_items::STONE)));
    assert!(
        player
            .inventory
            .lock()
            .get_item(0)
            .is(&vanilla_items::STONE)
    );

    let ender = player.get_slot(226).expect("enderchest.26");
    assert!(ender.set(stack(&vanilla_items::DIRT)));
    assert!(
        player
            .ender_chest_inventory
            .lock()
            .get_item(26)
            .is(&vanilla_items::DIRT)
    );

    let cursor = player.get_slot(499).expect("player.cursor");
    assert!(cursor.set(stack(&vanilla_items::STICK)));
    assert!(player.carried_item().is(&vanilla_items::STICK));

    let grid = range_slots(&*player, "player.crafting.*");
    assert_eq!(grid.len(), 4);
    assert!(grid[3].set(stack(&vanilla_items::OAK_PLANKS)));
    assert!(
        player
            .crafting_container()
            .lock()
            .get_item(3)
            .is(&vanilla_items::OAK_PLANKS)
    );
}

#[test]
fn armor_slots_only_take_what_fits_but_hands_and_head_take_anything() {
    init_vanilla_registry();
    init_behaviors();
    let zombie = ZombieEntity::new(&vanilla_entities::ZOMBIE, 1, DVec3::ZERO, Weak::new());
    let feet = zombie.get_slot(100).expect("armor.feet");
    assert!(!feet.set(stack(&vanilla_items::STONE)));
    assert!(feet.set(stack(&vanilla_items::IRON_BOOTS)));
    assert!(
        zombie
            .equipment_in_slot(EquipmentSlot::Feet)
            .is(&vanilla_items::IRON_BOOTS)
    );
    assert!(feet.set(ItemStack::empty()));

    let head = zombie.get_slot(103).expect("armor.head");
    assert!(head.set(stack(&vanilla_items::STONE)));
    let mainhand = zombie.get_slot(98).expect("weapon.mainhand");
    assert!(mainhand.set(stack(&vanilla_items::IRON_BOOTS)));
    assert!(zombie.get_slot(300).is_none());
}

#[test]
fn chested_horse_slot_499_adds_and_removes_the_chest() {
    init_vanilla_registry();
    init_behaviors();
    let donkey = DonkeyEntity::new(&vanilla_entities::DONKEY, 1, DVec3::ZERO, Weak::new());
    assert!(donkey.get_slot(500).is_none());
    let chest = donkey.get_slot(499).expect("horse.chest");
    assert!(chest.get().is_empty());
    assert!(!chest.set(stack(&vanilla_items::STONE)));
    assert!(chest.set(stack(&vanilla_items::CHEST)));
    assert!(chest.get().is(&vanilla_items::CHEST));
    // The chest opens up the horse inventory slots.
    let first = donkey.get_slot(500).expect("horse.0 once chested");
    assert!(first.set(stack(&vanilla_items::DIRT)));
    assert!(chest.set(ItemStack::empty()));
    assert!(donkey.get_slot(500).is_none());
}

#[test]
fn villager_inventory_starts_at_slot_300() {
    init_vanilla_registry();
    init_behaviors();
    let villager = VillagerEntity::new(&vanilla_entities::VILLAGER, 1, DVec3::ZERO, Weak::new());
    assert_eq!(range_slots(&villager, "mob.inventory.*").len(), 8);
    let slot = villager.get_slot(300).expect("mob.inventory.0");
    assert!(slot.set(stack(&vanilla_items::BREAD)));
    assert!(slot.get().is(&vanilla_items::BREAD));
}

#[test]
fn item_entities_expose_their_stack_as_slot_0() {
    init_vanilla_registry();
    let item = ItemEntity::new(&vanilla_entities::ITEM, 1, DVec3::ZERO, Weak::new());
    assert!(item.get_slot(1).is_none());
    let contents = item.get_slot(0).expect("contents");
    assert!(contents.set(stack(&vanilla_items::DIAMOND)));
    assert!(item.get_item().is(&vanilla_items::DIAMOND));
}
