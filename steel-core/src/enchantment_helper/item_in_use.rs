use steel_registry::item_stack::ItemStack;

use crate::entity::{Entity, LivingEntity};
use crate::inventory::equipment::EquipmentSlot;
use crate::player::advancement::criteria::report_durability_change;

/// Access to the enchanted stack without holding equipment locks across entity effects.
/// Retaliatory damage can inspect the owner's weapon while applying a victim's enchantments.
pub(super) enum EnchantedItemInUse<'a> {
    Stack(&'a mut ItemStack),
    Equipped {
        owner: &'a dyn LivingEntity,
        slot: EquipmentSlot,
    },
}

impl EnchantedItemInUse<'_> {
    pub(super) fn with_item(&self, visitor: &mut dyn FnMut(&ItemStack)) {
        match self {
            Self::Stack(stack) => visitor(stack),
            Self::Equipped { owner, slot } => owner.with_equipment_slot(*slot, visitor),
        }
    }

    /// Vanilla `ChangeItemDamage.apply`: `hurtAndBreak` with the item's owner,
    /// a player owner hearing `ITEM_DURABILITY_CHANGED`. `stack_owner` owns a
    /// bare stack; an equipped stack belongs to its wearer.
    pub(super) fn hurt_and_break(
        &mut self,
        amount: i32,
        infinite_materials: bool,
        stack_owner: Option<&dyn Entity>,
    ) -> bool {
        match self {
            Self::Stack(stack) => {
                let (broke, change) = stack.hurt_and_break_tracked(amount, infinite_materials);
                if let Some(owner) = stack_owner {
                    report_durability_change(owner, change);
                }
                broke
            }
            Self::Equipped { owner, slot } => {
                let mut broke = false;
                let mut change = None;
                owner.with_equipment_slot_mut(*slot, &mut |stack| {
                    (broke, change) = stack.hurt_and_break_tracked(amount, infinite_materials);
                });
                report_durability_change(*owner, change);
                broke
            }
        }
    }
}
