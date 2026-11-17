//! Copies a placed block entity's data back onto an item, as vanilla's
//! creative pick block with data does.

use simdnbt::owned::NbtCompound;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::data_components::components::{BlockEntityData, CustomData};
use steel_registry::data_components::vanilla_components::BLOCK_ENTITY_DATA;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_entity_types as types;

use crate::block_entity::BlockEntity;

/// Vanilla `BaseContainerBlockEntity.removeComponentsFromTag`.
const BASE_CONTAINER_KEYS: &[&str] = &["CustomName", "lock", "Items"];
/// Vanilla `RandomizableContainerBlockEntity.removeComponentsFromTag`.
const RANDOMIZABLE_CONTAINER_KEYS: &[&str] =
    &["CustomName", "lock", "Items", "LootTable", "LootTableSeed"];

/// The saved fields vanilla's `removeComponentsFromTag` overrides discard for a block entity
/// type, because item components carry them instead.
fn component_tag_keys(block_entity_type: BlockEntityTypeRef) -> &'static [&'static str] {
    let randomizable = [
        &types::CHEST,
        &types::TRAPPED_CHEST,
        &types::BARREL,
        &types::DISPENSER,
        &types::DROPPER,
        &types::HOPPER,
        &types::SHULKER_BOX,
        &types::CRAFTER,
    ];
    let base_container = [
        &types::FURNACE,
        &types::BLAST_FURNACE,
        &types::SMOKER,
        &types::BREWING_STAND,
    ];
    let is = |candidate: BlockEntityTypeRef| block_entity_type == candidate;
    if randomizable.into_iter().any(is) {
        RANDOMIZABLE_CONTAINER_KEYS
    } else if base_container.into_iter().any(is) {
        BASE_CONTAINER_KEYS
    } else if is(&types::BANNER) {
        &["patterns", "CustomName"]
    } else if is(&types::BEACON) {
        &["CustomName", "lock"]
    } else if is(&types::BEEHIVE) {
        &["bees"]
    } else if is(&types::CAMPFIRE) || is(&types::CHISELED_BOOKSHELF) || is(&types::SHELF) {
        &["Items"]
    } else if is(&types::COMMAND_BLOCK) {
        &["CustomName", "conditionMet", "powered"]
    } else if is(&types::DECORATED_POT) {
        &["sherds", "item"]
    } else if is(&types::ENCHANTING_TABLE) {
        &["CustomName"]
    } else if is(&types::SIGN) || is(&types::HANGING_SIGN) {
        &["front_text", "back_text", "is_waxed"]
    } else if is(&types::SKULL) {
        &["profile", "note_block_sound", "custom_name"]
    } else {
        &[]
    }
}

/// Vanilla `BlockEntity.removeComponentsFromTag`: drops the fields of `nbt` that item
/// components represent for this block entity type.
pub(crate) fn remove_components_from_tag(
    block_entity_type: BlockEntityTypeRef,
    nbt: &mut NbtCompound,
) {
    for key in component_tag_keys(block_entity_type) {
        while nbt.remove(key).is_some() {}
    }
}

/// Vanilla `BlockItem.setBlockEntityData`: stores `nbt` as the stack's block entity data, or
/// clears it when nothing is left.
fn set_block_entity_data(
    stack: &mut ItemStack,
    block_entity_type: BlockEntityTypeRef,
    mut nbt: NbtCompound,
) {
    while nbt.remove("id").is_some() {}
    match CustomData::try_from_compound(nbt).filter(|data| !data.is_empty()) {
        Some(data) => stack.set(
            BLOCK_ENTITY_DATA,
            BlockEntityData::new(block_entity_type, data),
        ),
        None => stack.remove(BLOCK_ENTITY_DATA),
    }
}

/// Vanilla `ServerGamePacketListenerImpl.addBlockDataToItem`: the block entity's own data
/// and its components, copied onto the picked `stack`.
pub(crate) fn add_block_data_to_item(block_entity: &dyn BlockEntity, stack: &mut ItemStack) {
    let mut nbt = block_entity.save_custom_only();
    remove_components_from_tag(block_entity.get_type(), &mut nbt);
    set_block_entity_data(stack, block_entity.get_type(), nbt);
    stack.apply_components(&block_entity.collect_components());
}

#[cfg(test)]
mod tests {
    use std::array;
    use std::sync::Arc;

    use steel_registry::data_components::vanilla_components::{
        CONTAINER, SIGN_TEXT_FRONT, SignText, WAXED,
    };
    use steel_registry::{DyeColor, init_vanilla_registry, vanilla_blocks, vanilla_items};
    use steel_utils::BlockPos;
    use text_components::TextComponent;

    use super::*;
    use crate::block_entity::container_name::contents_from_items;
    use crate::block_entity::entities::{CampfireBlockEntity, LecternBlockEntity, SignBlockEntity};
    use crate::test_support::fresh_test_world;

    #[test]
    fn a_picked_sign_carries_its_text_as_components_only() {
        init_vanilla_registry();
        let world_fixture = fresh_test_world("pick_sign_data");
        let sign = SignBlockEntity::new(
            Arc::downgrade(&world_fixture.world),
            BlockPos::new(0, 64, 0),
            vanilla_blocks::OAK_SIGN.default_state(),
        );
        let mut messages = array::from_fn(|_| TextComponent::new());
        messages[0] = TextComponent::plain("hello");
        let text = SignText::new(messages, None, DyeColor::Blue, true);
        let mut placed = ItemStack::new(&vanilla_items::OAK_SIGN);
        placed.set(SIGN_TEXT_FRONT, text.clone());
        placed.set(WAXED, ());
        sign.apply_components_from_item_stack(&placed);

        let mut picked = ItemStack::new(&vanilla_items::OAK_SIGN);
        add_block_data_to_item(&sign, &mut picked);

        assert_eq!(picked.get(SIGN_TEXT_FRONT), Some(&text));
        assert!(picked.has(WAXED));
        assert!(
            !picked.has(BLOCK_ENTITY_DATA),
            "the text is not repeated in the block entity data"
        );
    }

    #[test]
    fn a_picked_campfire_carries_its_food() {
        init_vanilla_registry();
        let world_fixture = fresh_test_world("pick_campfire_data");
        let campfire = CampfireBlockEntity::new(
            Arc::downgrade(&world_fixture.world),
            BlockPos::new(0, 64, 0),
            vanilla_blocks::CAMPFIRE.default_state(),
        );
        let food = [
            ItemStack::empty(),
            ItemStack::new(&vanilla_items::BEEF),
            ItemStack::empty(),
            ItemStack::empty(),
        ];
        let mut placed = ItemStack::new(&vanilla_items::CAMPFIRE);
        placed.set(CONTAINER, contents_from_items(&food));
        campfire.apply_components_from_item_stack(&placed);

        let mut picked = ItemStack::new(&vanilla_items::CAMPFIRE);
        add_block_data_to_item(&campfire, &mut picked);

        assert_eq!(picked.get(CONTAINER), Some(&contents_from_items(&food)));
        let data = picked
            .get(BLOCK_ENTITY_DATA)
            .expect("the cooking times stay in the block entity data");
        assert!(data.data().copy_tag().get("Items").is_none());
        assert!(data.data().copy_tag().get("CookingTimes").is_some());
    }

    #[test]
    fn a_picked_lectern_keeps_its_book_in_the_block_entity_data() {
        init_vanilla_registry();
        let world_fixture = fresh_test_world("pick_lectern_data");
        let lectern = LecternBlockEntity::new(
            Arc::downgrade(&world_fixture.world),
            BlockPos::new(0, 64, 0),
            vanilla_blocks::LECTERN.default_state(),
        );
        lectern.set_book(ItemStack::new(&vanilla_items::WRITABLE_BOOK));

        let mut picked = ItemStack::new(&vanilla_items::LECTERN);
        add_block_data_to_item(&lectern, &mut picked);

        let data = picked
            .get(BLOCK_ENTITY_DATA)
            .expect("a lectern keeps its book as block entity data");
        assert_eq!(data.block_entity_type(), &types::LECTERN);
        let tag = data.data().copy_tag();
        assert!(tag.get("Book").is_some());
        assert!(tag.get("id").is_none());
    }
}
