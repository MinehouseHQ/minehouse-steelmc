//! Item, block and entity interaction triggers (vanilla `CriteriaTriggers`
//! `GENERATE_LOOT`, `USED_TOTEM`, `START_RIDING`, `PLAYER_INTERACTED_WITH_ENTITY`,
//! `CONSTRUCT_BEACON`, `USING_ITEM`, `ITEM_DURABILITY_CHANGED`, `TRADE`,
//! `FISHING_ROD_HOOKED`, `TARGET_BLOCK_HIT`, `BEE_NEST_DESTROYED`,
//! `DEFAULT_BLOCK_USE`, `ANY_BLOCK_USE` and `ALLAY_DROP_ITEM_ON_BLOCK`).

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::item_stack::{DurabilityChange, ItemStack};
use steel_registry::{REGISTRY, TaggedRegistryExt as _};
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, Identifier};

use super::predicates::ConditionContext;
use super::trigger;
use super::triggers::test_context;
use crate::entity::Entity;
use crate::player::Player;

/// Vanilla `CriteriaTriggers.GENERATE_LOOT`.
pub(crate) const GENERATE_LOOT: Identifier =
    Identifier::vanilla_static("player_generates_container_loot");
/// Vanilla `CriteriaTriggers.USED_TOTEM`.
pub(crate) const USED_TOTEM: Identifier = Identifier::vanilla_static("used_totem");
/// Vanilla `CriteriaTriggers.START_RIDING_TRIGGER`.
pub(crate) const START_RIDING: Identifier = Identifier::vanilla_static("started_riding");
/// Vanilla `CriteriaTriggers.PLAYER_INTERACTED_WITH_ENTITY`.
pub(crate) const PLAYER_INTERACTED_WITH_ENTITY: Identifier =
    Identifier::vanilla_static("player_interacted_with_entity");
/// Vanilla `CriteriaTriggers.CONSTRUCT_BEACON`.
pub(crate) const CONSTRUCT_BEACON: Identifier = Identifier::vanilla_static("construct_beacon");
/// Vanilla `CriteriaTriggers.USING_ITEM`.
pub(crate) const USING_ITEM: Identifier = Identifier::vanilla_static("using_item");
/// Vanilla `CriteriaTriggers.ITEM_DURABILITY_CHANGED`.
pub(crate) const ITEM_DURABILITY_CHANGED: Identifier =
    Identifier::vanilla_static("item_durability_changed");
/// Vanilla `CriteriaTriggers.TRADE`.
pub(crate) const TRADE: Identifier = Identifier::vanilla_static("villager_trade");
/// Vanilla `CriteriaTriggers.FISHING_ROD_HOOKED`.
pub(crate) const FISHING_ROD_HOOKED: Identifier = Identifier::vanilla_static("fishing_rod_hooked");
/// Vanilla `CriteriaTriggers.TARGET_BLOCK_HIT`.
pub(crate) const TARGET_BLOCK_HIT: Identifier = Identifier::vanilla_static("target_hit");
/// Vanilla `CriteriaTriggers.BEE_NEST_DESTROYED`.
pub(crate) const BEE_NEST_DESTROYED: Identifier = Identifier::vanilla_static("bee_nest_destroyed");
/// Vanilla `CriteriaTriggers.DEFAULT_BLOCK_USE`.
pub(crate) const DEFAULT_BLOCK_USE: Identifier = Identifier::vanilla_static("default_block_use");
/// Vanilla `CriteriaTriggers.ANY_BLOCK_USE`, an `ItemUsedOnLocationTrigger`.
pub(crate) const ANY_BLOCK_USE: Identifier = Identifier::vanilla_static("any_block_use");
/// Vanilla `CriteriaTriggers.ALLAY_DROP_ITEM_ON_BLOCK`, an
/// `ItemUsedOnLocationTrigger`.
pub(crate) const ALLAY_DROP_ITEM_ON_BLOCK: Identifier =
    Identifier::vanilla_static("allay_drop_item_on_block");

/// Vanilla `applyDamage`'s `ITEM_DURABILITY_CHANGED` for a stack worn by
/// `owner`: only players have advancements.
pub(crate) fn report_durability_change<E: Entity + ?Sized>(
    owner: &E,
    change: Option<DurabilityChange>,
) {
    if let Some(player) = owner.as_player() {
        player.trigger_item_durability_changed(change);
    }
}

impl Player {
    /// Vanilla `DefaultBlockInteractionTrigger.trigger(player, pos)`: the
    /// `location` conditions see the block without a tool.
    pub(crate) fn trigger_default_block_use(&self, pos: BlockPos) {
        let world = self.get_world();
        let context = ConditionContext {
            world: &world,
            origin: Some(DVec3::new(
                f64::from(pos.x()) + 0.5,
                f64::from(pos.y()) + 0.5,
                f64::from(pos.z()) + 0.5,
            )),
            this: Some(self),
            tool: None,
            block_state: Some(world.get_block_state(pos)),
        };
        trigger(self, &DEFAULT_BLOCK_USE, |conditions| {
            test_context(conditions.location.as_ref(), &context)
        });
    }

    /// Vanilla `ItemStack.hurtAndBreak(amount, player, hand)` on the held
    /// stack, firing `ITEM_DURABILITY_CHANGED` once the inventory unlocks.
    pub(crate) fn hurt_item_in_hand(&self, hand: InteractionHand, amount: i32) {
        let change =
            self.inventory
                .lock()
                .hurt_item_in_hand(hand, amount, self.has_infinite_materials());
        self.trigger_item_durability_changed(change);
    }

    /// Vanilla `LootTableTrigger.trigger(player, lootTable)`.
    pub(crate) fn trigger_generate_loot(&self, loot_table: &Identifier) {
        trigger(self, &GENERATE_LOOT, |conditions| {
            conditions
                .loot_tables
                .as_ref()
                .is_some_and(|tables| tables.0.contains(loot_table))
        });
    }

    /// Vanilla `UsedTotemTrigger.trigger`, `UsingItemTrigger.trigger` and
    /// the other triggers that test one stack against `item`.
    pub(crate) fn trigger_item_criteria(&self, trigger_id: &Identifier, stack: &ItemStack) {
        trigger(self, trigger_id, |conditions| {
            conditions
                .item
                .as_ref()
                .is_none_or(|predicate| predicate.test(stack))
        });
    }

    /// Vanilla `PlayerInteractTrigger.trigger(player, itemStack, interactedWith)`.
    pub(crate) fn trigger_interacted_with_entity(&self, stack: &ItemStack, entity: &dyn Entity) {
        self.trigger_item_and_entity(&PLAYER_INTERACTED_WITH_ENTITY, stack, entity);
    }

    /// The triggers that test one stack against `item` and one entity against
    /// `entity` (vanilla `PlayerInteractTrigger` and `PickedUpItemTrigger`).
    pub(crate) fn trigger_item_and_entity(
        &self,
        trigger_id: &Identifier,
        stack: &ItemStack,
        entity: &dyn Entity,
    ) {
        let world = self.get_world();
        let context = ConditionContext::entity(&world, self.position(), entity);
        trigger(self, trigger_id, |conditions| {
            conditions
                .item
                .as_ref()
                .is_none_or(|predicate| predicate.test(stack))
                && test_context(conditions.entity.as_ref(), &context)
        });
    }

    /// Vanilla `ConstructBeaconTrigger.trigger(player, levels)`.
    pub(crate) fn trigger_construct_beacon(&self, levels: i32) {
        trigger(self, &CONSTRUCT_BEACON, |conditions| {
            conditions.level.matches(levels)
        });
    }

    /// Vanilla `ItemDurabilityTrigger.trigger(player, itemStack, newDurability)`,
    /// fired once the wearing code released its locks.
    pub(crate) fn trigger_item_durability_changed(&self, change: Option<DurabilityChange>) {
        let Some(DurabilityChange { stack, new_damage }) = change else {
            return;
        };
        let durability = stack.get_max_damage() - new_damage;
        let delta = stack.get_damage_value() - new_damage;
        trigger(self, &ITEM_DURABILITY_CHANGED, |conditions| {
            conditions
                .item
                .as_ref()
                .is_none_or(|predicate| predicate.test(&stack))
                && conditions.durability.matches(durability)
                && conditions.delta.matches(delta)
        });
    }

    /// Vanilla `TradeTrigger.trigger(player, villager, itemStack)`.
    pub(crate) fn trigger_trade(&self, villager: &dyn Entity, stack: &ItemStack) {
        let world = self.get_world();
        let context = ConditionContext::entity(&world, self.position(), villager);
        trigger(self, &TRADE, |conditions| {
            test_context(conditions.villager.as_ref(), &context)
                && conditions
                    .item
                    .as_ref()
                    .is_none_or(|predicate| predicate.test(stack))
        });
    }

    /// Vanilla `FishingRodHookedTrigger.trigger(player, rod, hook, items)`:
    /// `hooked_in` is the hooked entity, or the hook itself. A hooked item
    /// entity passes `item_entity_stack`.
    pub(crate) fn trigger_fishing_rod_hooked(
        &self,
        rod: &ItemStack,
        hooked_in: &dyn Entity,
        item_entity_stack: Option<&ItemStack>,
        items: &[ItemStack],
    ) {
        let world = self.get_world();
        let context = ConditionContext::entity(&world, self.position(), hooked_in);
        trigger(self, &FISHING_ROD_HOOKED, |conditions| {
            conditions
                .rod
                .as_ref()
                .is_none_or(|predicate| predicate.test(rod))
                && test_context(conditions.entity.as_ref(), &context)
                && conditions.item.as_ref().is_none_or(|predicate| {
                    item_entity_stack.is_some_and(|stack| predicate.test(stack))
                        || items.iter().any(|stack| predicate.test(stack))
                })
        });
    }

    /// Vanilla `TargetBlockTrigger.trigger(player, projectile, hitPosition,
    /// signalStrength)`.
    pub(crate) fn trigger_target_block_hit(&self, projectile: &dyn Entity, signal_strength: i32) {
        let world = self.get_world();
        let context = ConditionContext::entity(&world, self.position(), projectile);
        trigger(self, &TARGET_BLOCK_HIT, |conditions| {
            conditions.signal_strength.matches(signal_strength)
                && test_context(conditions.projectile.as_ref(), &context)
        });
    }

    /// Vanilla `BeeNestDestroyedTrigger.trigger(player, state, itemStack,
    /// numBeesInside)`.
    pub(crate) fn trigger_bee_nest_destroyed(
        &self,
        state: BlockStateId,
        tool: &ItemStack,
        bees_inside: i32,
    ) {
        trigger(self, &BEE_NEST_DESTROYED, |conditions| {
            conditions.blocks.as_ref().is_none_or(|blocks| {
                let block = state.get_block();
                blocks.contains(&block.key, |tag| REGISTRY.blocks.is_in_tag(block, tag))
            }) && conditions
                .state
                .as_ref()
                .is_none_or(|predicate| predicate.matches(state))
                && conditions
                    .item
                    .as_ref()
                    .is_none_or(|predicate| predicate.test(tool))
                && conditions.num_bees_inside.matches(bees_inside)
        });
    }
}
