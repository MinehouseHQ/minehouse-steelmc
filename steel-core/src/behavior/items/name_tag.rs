use std::sync::OnceLock;

use crate::behavior::{InteractionResult, ItemBehavior};
use crate::entity::LivingEntity;
use crate::player::Player;
use steel_macros::item_behavior;
use steel_registry::data_components::vanilla_components::CUSTOM_NAME;
use steel_registry::item_stack::ItemStack;
use steel_utils::types::InteractionHand;
use text_components::TextComponent;

/// Sees every entity a name tag names.
pub trait NameTagObserver: Send + Sync {
    /// A name tag just gave `target` the name `name`.
    fn named(&self, target: &dyn LivingEntity, name: &TextComponent);
}

static NAME_TAG_OBSERVER: OnceLock<&'static dyn NameTagObserver> = OnceLock::new();

/// Installs the name tag observer. Returns `false` if one is already installed.
pub fn set_name_tag_observer(observer: &'static dyn NameTagObserver) -> bool {
    NAME_TAG_OBSERVER.set(observer).is_ok()
}

/// Vanilla name tag behavior.
#[item_behavior]
pub struct NameTagItem;

impl ItemBehavior for NameTagItem {
    fn interact_living_entity(
        &self,
        stack: &mut ItemStack,
        _player: &Player,
        target: &dyn LivingEntity,
        _hand: InteractionHand,
    ) -> InteractionResult {
        if target.entity_type().can_serialize
            && let Some(component) = stack.get(CUSTOM_NAME)
        {
            if LivingEntity::is_alive(target) {
                target.set_custom_name(Some(component.clone()));
                if let Some(mob) = target.as_mob() {
                    mob.set_persistence_required();
                }
                if let Some(observer) = NAME_TAG_OBSERVER.get() {
                    observer.named(target, component);
                }
                stack.shrink_one();
            }
            InteractionResult::Success
        } else {
            InteractionResult::Pass
        }
    }
}
