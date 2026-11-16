//! Vanilla `BundleItem` and `BundleContents.Mutable`.

use std::sync::Arc;

use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::components::{
    BundleContents, CheckedFraction, bundle_stack_weight,
};
use steel_registry::data_components::vanilla_components::BUNDLE_CONTENTS;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::{ItemStackTemplate, sound_events};

use crate::behavior::context::{InteractionResult, UseItemContext};
use crate::behavior::{ITEM_BEHAVIORS, ItemBehavior, ItemUseAnimation};
use crate::entity::entities::ItemEntity;
use crate::entity::{Entity, LivingEntity, LivingEntityRef};
use crate::inventory::click::MouseButton;
use crate::inventory::lock::ContainerLockGuard;
use crate::inventory::slots::Slot;
use crate::player::Player;
use crate::player::player_inventory::Prediction;
use crate::world::World;

/// Vanilla `BundleItem.getUseDuration`.
const USE_DURATION: i32 = 200;
/// Vanilla `BundleItem.TICKS_AFTER_FIRST_THROW`.
const TICKS_AFTER_FIRST_THROW: i32 = 10;
/// Vanilla `BundleItem.TICKS_BETWEEN_THROWS`.
const TICKS_BETWEEN_THROWS: i32 = 2;

/// Vanilla `BundleItem`.
#[item_behavior]
pub struct BundleItem;

impl ItemBehavior for BundleItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        context.player.start_using_item(context.hand);
        InteractionResult::Success
    }

    fn get_use_duration(&self, _stack: &ItemStack, _user: &dyn LivingEntity) -> i32 {
        USE_DURATION
    }

    fn get_use_animation(&self, _stack: &ItemStack) -> ItemUseAnimation {
        ItemUseAnimation::Bundle
    }

    fn on_use_tick(
        &self,
        world: &Arc<World>,
        user: LivingEntityRef<'_>,
        stack: &mut ItemStack,
        ticks_remaining: i32,
    ) {
        let Some(player) = user.living().as_player() else {
            return;
        };
        let is_first_tick = ticks_remaining == USE_DURATION;
        if is_first_tick
            || (ticks_remaining < USE_DURATION - TICKS_AFTER_FIRST_THROW
                && ticks_remaining % TICKS_BETWEEN_THROWS == 0)
        {
            drop_one_content(world, player, stack);
        }
    }

    fn override_stacked_on_other(
        &self,
        bundle: &mut ItemStack,
        slot: &dyn Slot,
        guard: &mut ContainerLockGuard,
        button: MouseButton,
        player: &Player,
    ) -> bool {
        let Some(initial) = bundle.get(BUNDLE_CONTENTS) else {
            return false;
        };
        let mut contents = BundleMutable::from_contents(initial);
        let other_is_empty = slot.get_item(guard).is_empty();
        match button {
            MouseButton::Left if !other_is_empty => {
                if contents.try_transfer(slot, guard, player) > 0 {
                    play_insert_sound(player);
                } else {
                    play_insert_fail_sound(player);
                }
            }
            MouseButton::Right if other_is_empty => {
                if let Some(removed) = contents.remove_one() {
                    let mut remainder = slot.safe_insert(guard, removed, i32::MAX);
                    if remainder.count() > 0 {
                        contents.try_insert(&mut remainder);
                    } else {
                        play_remove_one_sound(player);
                    }
                }
            }
            MouseButton::Left | MouseButton::Right => return false,
        }
        bundle.set(BUNDLE_CONTENTS, contents.to_immutable());
        true
    }

    fn override_other_stacked_on_me(
        &self,
        bundle: &mut ItemStack,
        carried: &mut ItemStack,
        slot: &dyn Slot,
        guard: &mut ContainerLockGuard,
        button: MouseButton,
        player: &Player,
    ) -> bool {
        if button == MouseButton::Left && carried.is_empty() {
            toggle_selected_item(bundle, BundleContents::NO_SELECTED_ITEM_INDEX);
            return false;
        }
        let Some(initial) = bundle.get(BUNDLE_CONTENTS) else {
            return false;
        };
        let mut contents = BundleMutable::from_contents(initial);
        match button {
            MouseButton::Left => {
                if slot.allow_modification(guard, player) && contents.try_insert(carried) > 0 {
                    play_insert_sound(player);
                } else {
                    play_insert_fail_sound(player);
                }
            }
            MouseButton::Right if carried.is_empty() => {
                if slot.allow_modification(guard, player)
                    && let Some(removed) = contents.remove_one()
                {
                    play_remove_one_sound(player);
                    *carried = removed;
                }
            }
            MouseButton::Right => {
                toggle_selected_item(bundle, BundleContents::NO_SELECTED_ITEM_INDEX);
                return false;
            }
        }
        bundle.set(BUNDLE_CONTENTS, contents.to_immutable());
        true
    }

    fn on_destroyed(&self, entity: &ItemEntity, world: &Arc<World>) {
        let mut item = entity.get_item();
        let Some(contents) = item.get(BUNDLE_CONTENTS).cloned() else {
            return;
        };
        item.set(BUNDLE_CONTENTS, BundleContents::empty());
        entity.set_item(item);
        on_container_destroyed(
            entity,
            world,
            contents.items().iter().map(ItemStackTemplate::create),
        );
    }
}

/// Vanilla `ItemUtils.onContainerDestroyed`: the contents spill out where the
/// container item entity was.
pub(crate) fn on_container_destroyed(
    entity: &ItemEntity,
    world: &Arc<World>,
    contents: impl Iterator<Item = ItemStack>,
) {
    let pos = entity.position();
    for stack in contents {
        let _ = world.spawn_item(pos, stack);
    }
}

/// Vanilla `BundleItem.toggleSelectedItem`.
pub fn toggle_selected_item(stack: &mut ItemStack, selected_item: i32) {
    let Some(initial) = stack.get(BUNDLE_CONTENTS) else {
        return;
    };
    let mut contents = BundleMutable::from_contents(initial);
    contents.toggle_selected_item(selected_item);
    stack.set(BUNDLE_CONTENTS, contents.to_immutable());
}

/// Vanilla `BundleItem.dropContent(level, player, stack)`.
fn drop_one_content(world: &World, player: &Player, bundle: &mut ItemStack) {
    let Some(initial) = bundle.get(BUNDLE_CONTENTS) else {
        return;
    };
    if initial.is_empty() {
        return;
    }
    let mut contents = BundleMutable::from_contents(initial);
    let Some(removed) = contents.remove_one() else {
        return;
    };
    play_remove_one_sound(player);
    bundle.set(BUNDLE_CONTENTS, contents.to_immutable());
    let _ = player.drop_item(removed, true, Prediction::Predicted);
    world.play_sound(
        &sound_events::ITEM_BUNDLE_DROP_CONTENTS,
        SoundSource::Players,
        player.block_position(),
        0.8,
        random_pitch(),
        None,
    );
    player.award_stat(&vanilla_stat_types::ITEM_USED, bundle.item());
}

fn random_pitch() -> f32 {
    0.8 + rand::random::<f32>() * 0.4
}

fn play_sound(player: &Player, sound: SoundEventRef, volume: f32, pitch: f32) {
    Entity::play_sound(player, sound, volume, pitch);
}

fn play_remove_one_sound(player: &Player) {
    play_sound(
        player,
        &sound_events::ITEM_BUNDLE_REMOVE_ONE,
        0.8,
        random_pitch(),
    );
}

fn play_insert_sound(player: &Player) {
    play_sound(
        player,
        &sound_events::ITEM_BUNDLE_INSERT,
        0.8,
        random_pitch(),
    );
}

fn play_insert_fail_sound(player: &Player) {
    play_sound(player, &sound_events::ITEM_BUNDLE_INSERT_FAIL, 1.0, 1.0);
}

/// Vanilla `BundleContents.canItemBeInBundle`.
fn can_item_be_in_bundle(stack: &ItemStack) -> bool {
    !stack.is_empty()
        && ITEM_BEHAVIORS
            .get_behavior(stack.item())
            .can_fit_inside_container_items()
}

/// Vanilla `BundleContents.Mutable`.
#[derive(Debug)]
pub struct BundleMutable {
    items: Vec<ItemStack>,
    weight: CheckedFraction,
    selected_item: i32,
}

impl BundleMutable {
    /// Vanilla `BundleContents.asMutable`: an over-weight bundle starts empty.
    #[must_use]
    pub fn from_contents(contents: &BundleContents) -> Self {
        match contents.weight() {
            Ok(weight) => Self {
                items: contents
                    .items()
                    .iter()
                    .map(ItemStackTemplate::create)
                    .collect(),
                weight,
                selected_item: contents.selected_item_index(),
            },
            Err(_) => Self {
                items: Vec::new(),
                weight: CheckedFraction::ZERO,
                selected_item: BundleContents::NO_SELECTED_ITEM_INDEX,
            },
        }
    }

    /// Vanilla `Mutable.weight`.
    #[must_use]
    pub const fn weight(&self) -> CheckedFraction {
        self.weight
    }

    fn find_stack_index(&self, stack: &ItemStack) -> Option<usize> {
        if !stack.is_stackable() {
            return None;
        }
        self.items
            .iter()
            .position(|item| ItemStack::is_same_item_same_components(item, stack))
    }

    /// Vanilla `Mutable.tryInsert`: moves what fits out of `stack`, returning
    /// how many items went in.
    pub fn try_insert(&mut self, stack: &mut ItemStack) -> i32 {
        if !can_item_be_in_bundle(stack) {
            return 0;
        }
        let Ok(item_weight) = bundle_stack_weight(stack) else {
            return 0;
        };
        let amount = stack
            .count()
            .min(self.weight.remaining_capacity_for(item_weight));
        if amount == 0 {
            return 0;
        }
        let Ok(added) = item_weight.multiply(amount) else {
            return 0;
        };
        let Ok(weight) = self.weight.plus(added) else {
            return 0;
        };
        self.weight = weight;
        if let Some(index) = self.find_stack_index(stack) {
            let existing = self.items.remove(index);
            let merged = existing.copy_with_count(existing.count() + amount);
            stack.shrink(amount);
            self.items.insert(0, merged);
        } else {
            self.items.insert(0, stack.split(amount));
        }
        amount
    }

    /// Vanilla `Mutable.tryTransfer`: takes what fits from `slot`.
    pub fn try_transfer(
        &mut self,
        slot: &dyn Slot,
        guard: &mut ContainerLockGuard,
        player: &Player,
    ) -> i32 {
        let other = slot.get_item(guard);
        let Ok(item_weight) = bundle_stack_weight(other) else {
            return 0;
        };
        if !can_item_be_in_bundle(other) {
            return 0;
        }
        let count = other.count();
        let max_amount = self.weight.remaining_capacity_for(item_weight);
        let mut taken = slot.safe_take(guard, count, max_amount, player);
        self.try_insert(&mut taken)
    }

    /// Vanilla `Mutable.toggleSelectedItem`.
    pub fn toggle_selected_item(&mut self, selected_item: i32) {
        self.selected_item =
            if self.selected_item != selected_item && !self.is_outside_bounds(selected_item) {
                selected_item
            } else {
                BundleContents::NO_SELECTED_ITEM_INDEX
            };
    }

    fn is_outside_bounds(&self, index: i32) -> bool {
        usize::try_from(index).map_or(true, |index| index >= self.items.len())
    }

    /// Vanilla `Mutable.removeOne`: the selected stack, else the newest one.
    pub fn remove_one(&mut self) -> Option<ItemStack> {
        if self.items.is_empty() {
            return None;
        }
        let index = if self.is_outside_bounds(self.selected_item) {
            0
        } else {
            usize::try_from(self.selected_item).unwrap_or(0)
        };
        let stack = self.items.remove(index);
        if let Ok(removed) =
            bundle_stack_weight(&stack).and_then(|weight| weight.multiply(stack.count()))
            && let Ok(weight) = self.weight.subtract(removed)
        {
            self.weight = weight;
        }
        self.toggle_selected_item(BundleContents::NO_SELECTED_ITEM_INDEX);
        Some(stack)
    }

    /// Vanilla `Mutable.toImmutable`.
    #[must_use]
    pub fn to_immutable(&self) -> BundleContents {
        let items = self
            .items
            .iter()
            .filter_map(|item| ItemStackTemplate::from_stack(item).ok())
            .collect();
        BundleContents::with_selected_item(items, self.selected_item)
    }
}

#[cfg(test)]
#[path = "bundle_tests.rs"]
mod tests;
