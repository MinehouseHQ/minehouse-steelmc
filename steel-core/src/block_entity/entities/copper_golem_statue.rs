//! Vanilla `CopperGolemStatueBlockEntity`: keeps the name of the copper
//! golem that turned to copper, so the golem it becomes again keeps it.

use std::collections::BTreeMap;
use std::sync::Weak;

use simdnbt::borrow::BaseNbtCompound as BorrowedNbtCompound;
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, PropertyEnum as _};
use steel_registry::data_components::components::BlockItemStateProperties;
use steel_registry::data_components::vanilla_components::{BLOCK_STATE, CUSTOM_NAME};
use steel_registry::item_stack::ItemStack;
use steel_registry::{REGISTRY, vanilla_block_entity_types};
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};
use text_components::TextComponent;

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;

/// A copper golem statue's block entity.
pub struct CopperGolemStatueBlockEntity {
    base: BlockEntityBase,
}

// SAFETY: This key is owned by Steel and uniquely identifies
// `CopperGolemStatueBlockEntity`.
unsafe impl DowncastType for CopperGolemStatueBlockEntity {
    const TYPE_KEY: DowncastTypeKey =
        DowncastTypeKey::new("steel:block_entity/copper_golem_statue");
}

impl CopperGolemStatueBlockEntity {
    /// Creates a statue block entity.
    #[must_use]
    pub fn new(world: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::COPPER_GOLEM_STATUE,
                world,
                pos,
                state,
            ),
        }
    }

    /// Vanilla `createStatue`: remembers the golem's custom name.
    pub fn create_statue(&self, golem_name: Option<TextComponent>) {
        let mut components = self.base.stored_components();
        components.set(CUSTOM_NAME, golem_name);
        self.base.set_components(components);
        self.base.set_changed();
    }

    /// The name the golem had, which the revived golem takes back. Vanilla
    /// `removeStatue` reads it before making the golem.
    #[must_use]
    pub fn golem_name(&self) -> Option<TextComponent> {
        self.base.stored_components().get(CUSTOM_NAME)
    }

    /// Vanilla `getItem`: the statue item with this statue's components and
    /// its pose.
    #[must_use]
    pub fn item(&self, state: BlockStateId) -> ItemStack {
        let mut item = ItemStack::new(REGISTRY.items.by_block(state.get_block()));
        item.apply_components(&self.collect_components());
        let pose = state.get_value(&BlockStateProperties::COPPER_GOLEM_POSE);
        let mut properties = BTreeMap::new();
        properties.insert(
            BlockStateProperties::COPPER_GOLEM_POSE.name.to_owned(),
            pose.as_str().to_owned(),
        );
        item.set(BLOCK_STATE, BlockItemStateProperties::new(properties));
        item
    }
}

impl BlockEntity for CopperGolemStatueBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, _nbt: &BorrowedNbtCompound<'_>) {}

    fn save_additional(&self, _nbt: &mut NbtCompound) {}

    /// Vanilla sends the statue with an empty update tag.
    fn get_update_tag(&self) -> Option<NbtCompound> {
        Some(NbtCompound::new())
    }
}
