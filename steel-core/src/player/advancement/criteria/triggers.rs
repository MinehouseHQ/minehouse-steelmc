//! Vanilla `CriteriaTriggers` fired by players.

use steel_registry::data_components::vanilla_components::PotionContents;
use steel_registry::item_stack::ItemStack;
use steel_registry::{REGISTRY, TaggedRegistryExt as _};
use steel_utils::Identifier;

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_utils::{BlockPos, BlockStateId};

use super::predicates::{ConditionContext, ContextPredicate};
use super::trigger;
use crate::entity::damage::DamageSource;
use crate::entity::{Entity, SharedEntity};
use crate::inventory::container::Container as _;
use crate::player::Player;
use crate::world::World;

/// `minecraft:tick`
pub(crate) const TICK: Identifier = Identifier::vanilla_static("tick");
/// `minecraft:location`
pub(crate) const LOCATION: Identifier = Identifier::vanilla_static("location");
/// `minecraft:slept_in_bed`
pub(crate) const SLEPT_IN_BED: Identifier = Identifier::vanilla_static("slept_in_bed");
/// `minecraft:hero_of_the_village` (vanilla `CriteriaTriggers.RAID_WIN`)
pub(crate) const RAID_WIN: Identifier = Identifier::vanilla_static("hero_of_the_village");
/// `minecraft:voluntary_exile` (vanilla `CriteriaTriggers.RAID_OMEN`)
pub(crate) const RAID_OMEN: Identifier = Identifier::vanilla_static("voluntary_exile");
/// `minecraft:brewed_potion`
pub(crate) const BREWED_POTION: Identifier = Identifier::vanilla_static("brewed_potion");
/// `minecraft:filled_bucket`
pub(crate) const FILLED_BUCKET: Identifier = Identifier::vanilla_static("filled_bucket");
/// `minecraft:inventory_changed`
pub(crate) const INVENTORY_CHANGED: Identifier = Identifier::vanilla_static("inventory_changed");
/// `minecraft:recipe_unlocked`
pub(crate) const RECIPE_UNLOCKED: Identifier = Identifier::vanilla_static("recipe_unlocked");
/// Vanilla `CriteriaTriggers.RECIPE_CRAFTED`.
pub(crate) const RECIPE_CRAFTED: Identifier = Identifier::vanilla_static("recipe_crafted");
/// Vanilla `CriteriaTriggers.CRAFTER_RECIPE_CRAFTED`.
pub(crate) const CRAFTER_RECIPE_CRAFTED: Identifier =
    Identifier::vanilla_static("crafter_recipe_crafted");
/// Vanilla `CriteriaTriggers.PLAYER_KILLED_ENTITY`.
pub(crate) const PLAYER_KILLED_ENTITY: Identifier =
    Identifier::vanilla_static("player_killed_entity");
/// Vanilla `CriteriaTriggers.ENTITY_KILLED_PLAYER`.
pub(crate) const ENTITY_KILLED_PLAYER: Identifier =
    Identifier::vanilla_static("entity_killed_player");
/// Vanilla `CriteriaTriggers.PLAYER_HURT_ENTITY`.
pub(crate) const PLAYER_HURT_ENTITY: Identifier = Identifier::vanilla_static("player_hurt_entity");
/// Vanilla `CriteriaTriggers.ENTITY_HURT_PLAYER`.
pub(crate) const ENTITY_HURT_PLAYER: Identifier = Identifier::vanilla_static("entity_hurt_player");
/// Vanilla `CriteriaTriggers.CONSUME_ITEM`.
pub(crate) const CONSUME_ITEM: Identifier = Identifier::vanilla_static("consume_item");
/// Vanilla `CriteriaTriggers.BRED_ANIMALS`.
pub(crate) const BRED_ANIMALS: Identifier = Identifier::vanilla_static("bred_animals");
/// Vanilla `CriteriaTriggers.TAME_ANIMAL`.
pub(crate) const TAME_ANIMAL: Identifier = Identifier::vanilla_static("tame_animal");
/// Vanilla `CriteriaTriggers.PLACED_BLOCK`.
pub(crate) const PLACED_BLOCK: Identifier = Identifier::vanilla_static("placed_block");
/// Vanilla `CriteriaTriggers.ITEM_USED_ON_BLOCK`.
pub(crate) const ITEM_USED_ON_BLOCK: Identifier = Identifier::vanilla_static("item_used_on_block");
/// Vanilla `CriteriaTriggers.ENTER_BLOCK`.
pub(crate) const ENTER_BLOCK: Identifier = Identifier::vanilla_static("enter_block");
/// Vanilla `CriteriaTriggers.CHANGED_DIMENSION`.
pub(crate) const CHANGED_DIMENSION: Identifier = Identifier::vanilla_static("changed_dimension");
/// Vanilla `CriteriaTriggers.EFFECTS_CHANGED`.
pub(crate) const EFFECTS_CHANGED: Identifier = Identifier::vanilla_static("effects_changed");
/// Vanilla `CriteriaTriggers.SUMMONED_ENTITY`.
pub(crate) const SUMMONED_ENTITY: Identifier = Identifier::vanilla_static("summoned_entity");
/// Vanilla `CriteriaTriggers.ENCHANTED_ITEM`.
pub(crate) const ENCHANTED_ITEM: Identifier = Identifier::vanilla_static("enchanted_item");
/// Vanilla `CriteriaTriggers.KILLED_BY_ARROW`.
pub(crate) const KILLED_BY_ARROW: Identifier = Identifier::vanilla_static("killed_by_arrow");
/// Vanilla `CriteriaTriggers.SHOT_CROSSBOW`.
pub(crate) const SHOT_CROSSBOW: Identifier = Identifier::vanilla_static("shot_crossbow");

impl Player {
    /// Vanilla `PlayerTrigger.trigger(player)` for a trigger without
    /// conditions of its own (`tick`, `location`, `slept_in_bed`, ...).
    pub(crate) fn trigger_player_criteria(&self, trigger_id: &Identifier) {
        trigger(self, trigger_id, |_| true);
    }

    /// Vanilla `BrewedPotionTrigger.trigger(player, potion)`.
    pub(crate) fn trigger_brewed_potion(&self, contents: &PotionContents) {
        trigger(self, &BREWED_POTION, |conditions| {
            // Vanilla `PotionsPredicate.matches`.
            conditions.potion.as_ref().is_none_or(|predicate| {
                predicate.potions.as_ref().is_none_or(|potions| {
                    contents.potion().is_some_and(|potion| {
                        let potion = potion.value();
                        potions.contains(&potion.key, |tag| REGISTRY.potions.is_in_tag(potion, tag))
                    })
                })
            })
        });
    }

    /// Vanilla `FilledBucketTrigger.trigger(player, stack)`.
    pub(crate) fn trigger_filled_bucket(&self, stack: &ItemStack) {
        trigger(self, &FILLED_BUCKET, |conditions| {
            conditions
                .item
                .as_ref()
                .is_none_or(|predicate| predicate.test(stack))
        });
    }

    /// Vanilla `InventoryChangeTrigger.trigger(player, inventory, changedItem)`.
    pub(crate) fn trigger_inventory_changed(&self, changed_item: &ItemStack) {
        let items: Vec<ItemStack> = {
            let inventory = self.inventory.lock();
            (0..inventory.get_container_size())
                .map(|slot| inventory.get_item(slot).clone())
                .collect()
        };
        let mut slots_full = 0;
        let mut slots_empty = 0;
        let mut slots_occupied = 0;
        for stack in &items {
            if stack.is_empty() {
                slots_empty += 1;
            } else {
                slots_occupied += 1;
                if stack.count() >= stack.max_stack_size() {
                    slots_full += 1;
                }
            }
        }

        trigger(self, &INVENTORY_CHANGED, |conditions| {
            let slots = &conditions.slots;
            if !(slots.full.matches(slots_full)
                && slots.empty.matches(slots_empty)
                && slots.occupied.matches(slots_occupied))
            {
                return false;
            }
            match conditions.items.as_slice() {
                [] => true,
                [predicate] => !changed_item.is_empty() && predicate.test(changed_item),
                predicates => {
                    // Every predicate must match some stack; each stack
                    // removes all predicates it matches.
                    let mut remaining: Vec<_> = predicates.iter().collect();
                    for stack in &items {
                        if remaining.is_empty() {
                            return true;
                        }
                        if !stack.is_empty() {
                            remaining.retain(|predicate| !predicate.test(stack));
                        }
                    }
                    remaining.is_empty()
                }
            }
        });
    }

    /// Vanilla `KilledTrigger.trigger(player, entity, killingBlow)`, for
    /// `player_killed_entity` and `entity_killed_player`.
    pub(crate) fn trigger_killed(
        &self,
        trigger_id: &Identifier,
        entity: &dyn Entity,
        killing_blow: &DamageSource,
    ) {
        let world = self.get_world();
        let position = self.position();
        let entity_context = ConditionContext::entity(&world, position, entity);
        trigger(self, trigger_id, |conditions| {
            conditions
                .killing_blow
                .as_ref()
                .is_none_or(|predicate| predicate.matches(&world, position, killing_blow))
                && test_context(conditions.entity.as_ref(), &entity_context)
        });
    }

    /// Vanilla `PlayerHurtEntityTrigger.trigger`.
    pub(crate) fn trigger_player_hurt_entity(
        &self,
        victim: &dyn Entity,
        source: &DamageSource,
        damage: f32,
        blocked: bool,
    ) {
        let world = self.get_world();
        let position = self.position();
        let victim_context = ConditionContext::entity(&world, position, victim);
        trigger(self, &PLAYER_HURT_ENTITY, |conditions| {
            conditions.damage.as_ref().is_none_or(|predicate| {
                predicate.matches(&world, position, source, damage, damage, blocked)
            }) && test_context(conditions.entity.as_ref(), &victim_context)
        });
    }

    /// Vanilla `EntityHurtPlayerTrigger.trigger`.
    pub(crate) fn trigger_entity_hurt_player(
        &self,
        source: &DamageSource,
        damage: f32,
        blocked: bool,
    ) {
        let world = self.get_world();
        let position = self.position();
        trigger(self, &ENTITY_HURT_PLAYER, |conditions| {
            conditions.damage.as_ref().is_none_or(|predicate| {
                predicate.matches(&world, position, source, damage, damage, blocked)
            })
        });
    }

    /// Vanilla `ConsumeItemTrigger.trigger(player, itemStack)`.
    pub(crate) fn trigger_consume_item(&self, stack: &ItemStack) {
        trigger(self, &CONSUME_ITEM, |conditions| {
            conditions
                .item
                .as_ref()
                .is_none_or(|predicate| predicate.test(stack))
        });
    }

    /// Vanilla `BredAnimalsTrigger.trigger(player, parent, partner, child)`.
    pub(crate) fn trigger_bred_animals(
        &self,
        parent: &dyn Entity,
        partner: &dyn Entity,
        child: Option<&dyn Entity>,
    ) {
        let world = self.get_world();
        let position = self.position();
        let parent = ConditionContext::entity(&world, position, parent);
        let partner = ConditionContext::entity(&world, position, partner);
        let child = child.map(|child| ConditionContext::entity(&world, position, child));
        trigger(self, &BRED_ANIMALS, |conditions| {
            let child_matches = conditions.child.as_ref().is_none_or(|predicate| {
                child
                    .as_ref()
                    .is_some_and(|context| predicate.test(context))
            });
            let parents = |first: &ConditionContext<'_>, second: &ConditionContext<'_>| {
                test_context(conditions.parent.as_ref(), first)
                    && test_context(conditions.partner.as_ref(), second)
            };
            child_matches && (parents(&parent, &partner) || parents(&partner, &parent))
        });
    }

    /// Vanilla `TameAnimalTrigger.trigger(player, animal)` and
    /// `SummonedEntityTrigger.trigger(player, entity)`: one entity tested
    /// against `entity`.
    pub(crate) fn trigger_entity_criteria(&self, trigger_id: &Identifier, entity: &dyn Entity) {
        let world = self.get_world();
        let context = ConditionContext::entity(&world, self.position(), entity);
        trigger(self, trigger_id, |conditions| {
            test_context(conditions.entity.as_ref(), &context)
        });
    }

    /// Vanilla `ItemUsedOnLocationTrigger.trigger(player, pos, tool)`, for
    /// `placed_block`, `item_used_on_block` and
    /// `allay_drop_item_on_block`.
    pub(crate) fn trigger_item_used_on_location(
        &self,
        trigger_id: &Identifier,
        pos: BlockPos,
        tool: &ItemStack,
    ) {
        let world = self.get_world();
        let context = ConditionContext {
            world: &world,
            origin: Some(DVec3::new(
                f64::from(pos.x()) + 0.5,
                f64::from(pos.y()) + 0.5,
                f64::from(pos.z()) + 0.5,
            )),
            this: Some(self),
            tool: Some(tool),
            block_state: Some(world.get_block_state(pos)),
        };
        trigger(self, trigger_id, |conditions| {
            test_context(conditions.location.as_ref(), &context)
        });
    }

    /// Vanilla `EnterBlockTrigger.trigger(player, state)`.
    pub(crate) fn trigger_enter_block(&self, state: BlockStateId) {
        trigger(self, &ENTER_BLOCK, |conditions| {
            conditions.blocks.as_ref().is_none_or(|blocks| {
                let block = state.get_block();
                blocks.contains(&block.key, |tag| REGISTRY.blocks.is_in_tag(block, tag))
            }) && conditions
                .state
                .as_ref()
                .is_none_or(|predicate| predicate.matches(state))
        });
    }

    /// Vanilla `ChangeDimensionTrigger.trigger(player, from, to)`.
    pub(crate) fn trigger_changed_dimension(&self, from: &Identifier, to: &Identifier) {
        trigger(self, &CHANGED_DIMENSION, |conditions| {
            conditions.from.as_ref().is_none_or(|key| key == from)
                && conditions.to.as_ref().is_none_or(|key| key == to)
        });
    }

    /// Vanilla `EffectsChangedTrigger.trigger(player, source)`.
    pub(crate) fn trigger_effects_changed(&self, source: Option<&dyn Entity>) {
        let world = self.get_world();
        let source = source.map(|source| ConditionContext::entity(&world, self.position(), source));
        trigger(self, &EFFECTS_CHANGED, |conditions| {
            conditions
                .effects
                .as_ref()
                .is_none_or(|predicate| predicate.matches(self))
                && conditions.source.as_ref().is_none_or(|predicate| {
                    source
                        .as_ref()
                        .is_some_and(|context| predicate.test(context))
                })
        });
    }

    /// Vanilla `ShotCrossbowTrigger.trigger(player, crossbow)`.
    pub(crate) fn trigger_shot_crossbow(&self, crossbow: &ItemStack) {
        trigger(self, &SHOT_CROSSBOW, |conditions| {
            conditions
                .item
                .as_ref()
                .is_none_or(|predicate| predicate.test(crossbow))
        });
    }

    /// Vanilla `EnchantedItemTrigger.trigger(player, itemStack, levels)`.
    pub(crate) fn trigger_enchanted_item(&self, stack: &ItemStack, levels: i32) {
        trigger(self, &ENCHANTED_ITEM, |conditions| {
            conditions
                .item
                .as_ref()
                .is_none_or(|predicate| predicate.test(stack))
                && conditions.levels.matches(levels)
        });
    }

    /// Vanilla `KilledByArrowTrigger.trigger(player, victims, firedByWeapon)`.
    pub(crate) fn trigger_killed_by_arrow(
        &self,
        victims: &[SharedEntity],
        fired_by_weapon: Option<&ItemStack>,
    ) {
        let world = self.get_world();
        let position = self.position();
        let contexts: Vec<ConditionContext<'_>> = victims
            .iter()
            .map(|victim| ConditionContext::entity(&world, position, victim.as_ref()))
            .collect();
        let mut types: Vec<Identifier> = Vec::new();
        for victim in victims {
            let key = victim.entity_type().key.clone();
            if !types.contains(&key) {
                types.push(key);
            }
        }
        let unique_types = i32::try_from(types.len()).unwrap_or(i32::MAX);
        trigger(self, &KILLED_BY_ARROW, |conditions| {
            if let Some(predicate) = &conditions.fired_from_weapon
                && !fired_by_weapon.is_some_and(|weapon| predicate.test(weapon))
            {
                return false;
            }
            let mut remaining: Vec<&ConditionContext<'_>> = contexts.iter().collect();
            for predicate in &conditions.victims {
                let Some(found) = remaining.iter().position(|context| predicate.test(context))
                else {
                    return false;
                };
                remaining.remove(found);
            }
            conditions.unique_entity_types.matches(unique_types)
        });
    }
}

/// An optional loot condition on a context: missing conditions pass.
fn test_context(predicate: Option<&ContextPredicate>, context: &ConditionContext<'_>) -> bool {
    predicate.is_none_or(|predicate| predicate.test(context))
}

/// Vanilla's `getEntitiesOfClass(ServerPlayer.class, summoned.getBoundingBox().inflate(range))`
/// loop of `SummonedEntityTrigger.trigger`: every non-spectator player near a
/// golem or wither built from blocks.
pub(crate) fn trigger_summoned_entity_nearby(world: &World, summoned: &dyn Entity, range: f64) {
    let area = summoned.bounding_box().inflate(range);
    let players = world.get_entities_in_aabb_matching(&area, |entity| {
        entity.as_player().is_some() && !entity.is_spectator()
    });
    for player in &players {
        if let Some(player) = player.as_player() {
            player.trigger_entity_criteria(&SUMMONED_ENTITY, summoned);
        }
    }
}
