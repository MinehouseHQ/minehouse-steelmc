//! Washing blocks in a cauldron (the Fabric `CauldronConversion`). A stack thrown into a
//! cauldron of water changes all at once: dirt, coarse dirt and rooted dirt become mud, and
//! concrete powder becomes concrete of its color. Mud takes no water. Concrete takes one
//! level of water for each started stack of 64; what the water did not cover stays powder
//! in the cauldron and changes once it is filled again.

use std::sync::Arc;

use glam::DVec3;
use steel_core::{
    entity::{
        Entity as _, SharedEntity,
        entities::{ItemEntity, objects::items::ItemEntityObserver},
        next_entity_id,
    },
    world::World,
};
use steel_protocol::packets::game::SoundSource;
use steel_registry::{
    REGISTRY, RegistryExt as _,
    blocks::{block_state_ext::BlockStateExt as _, properties::BlockStateProperties},
    item_stack::ItemStack,
    items::ItemRef,
    sound_events as sounds,
    vanilla_block_tags::BlockTag,
    vanilla_blocks, vanilla_entities, vanilla_items,
};
use steel_utils::{Identifier, types::UpdateFlags};

/// Every how many ticks an item looks at the block it lies in.
const CHECK_EVERY_TICKS: i32 = 5;
/// The items one level of water covers.
const STACK: i32 = 64;
/// What ends the name of every concrete powder.
const POWDER: &str = "_concrete_powder";

/// One level of water for each started stack: 1 to 64 items take 1, 65 to 128 take 2.
#[must_use]
pub const fn water_levels(count: i32) -> i32 {
    (count + STACK - 1) / STACK
}

/// The name of the concrete a powder named `powder` sets into, if it is a powder.
#[must_use]
pub fn concrete_of(powder: &str) -> Option<String> {
    let color = powder.strip_suffix(POWDER)?;
    (!color.is_empty()).then(|| format!("{color}_concrete"))
}

/// What `item` turns into in water and whether that takes water. `None` for anything else.
#[must_use]
pub fn converted(item: ItemRef) -> Option<(ItemRef, bool)> {
    let block = item.block?;
    if block.has_tag(&BlockTag::CONVERTIBLE_TO_MUD) {
        return Some((&vanilla_items::MUD, false));
    }
    if item.key.namespace != "minecraft" {
        return None;
    }
    let concrete = concrete_of(&item.key.path)?;
    let concrete = REGISTRY.items.by_key(&Identifier::vanilla(concrete))?;
    Some((concrete, true))
}

/// How a stack of `count` is shared out when the cauldron holds `levels` of water:
/// the items that change and the levels they take. With `uses_water` off all change.
#[must_use]
pub fn share(count: i32, levels: i32, uses_water: bool) -> (i32, i32) {
    if !uses_water {
        return (count, 0);
    }
    let used = water_levels(count).min(levels);
    (count.min(used * STACK), used)
}

/// Washes the stacks lying in cauldrons of water.
pub(super) struct CauldronConversion;

/// The one observer of item entities.
pub(super) static CAULDRON_CONVERSION: CauldronConversion = CauldronConversion;

impl ItemEntityObserver for CauldronConversion {
    fn tick(&self, entity: &ItemEntity, world: &Arc<World>) {
        if entity.tick_count() % CHECK_EVERY_TICKS != 0 || entity.is_removed() {
            return;
        }
        let pos = entity.block_position();
        let cauldron = world.get_block_state(pos);
        if cauldron.get_block() != &vanilla_blocks::WATER_CAULDRON {
            return;
        }
        let stack = entity.get_item();
        let Some((into, uses_water)) = converted(stack.item()) else {
            return;
        };
        let levels = i32::from(cauldron.get_value(&BlockStateProperties::LEVEL_CAULDRON));
        let (count, used) = share(stack.count(), levels, uses_water);
        if count <= 0 {
            return;
        }

        let rest = stack.copy_with_count(stack.count() - count);
        entity.set_item(ItemStack::with_count_and_patch(
            into,
            count,
            stack.components_patch().clone(),
        ));
        if !rest.is_empty() {
            let leftover: SharedEntity = Arc::new(ItemEntity::with_item_and_velocity(
                &vanilla_entities::ITEM,
                next_entity_id(),
                entity.position(),
                rest,
                DVec3::ZERO,
                Arc::downgrade(world),
            ));
            if world.try_add_entity(leftover).is_err() {
                log::warn!(
                    "MineHouse cauldron: the powder left over at {pos:?} could not be put back"
                );
            }
        }

        world.play_sound(
            &sounds::ENTITY_GENERIC_SPLASH,
            SoundSource::Blocks,
            pos,
            0.5,
            1.0,
            None,
        );
        if used > 0 {
            let left = levels - used;
            let state = match u8::try_from(left) {
                Ok(left) if left > 0 => {
                    cauldron.set_value(&BlockStateProperties::LEVEL_CAULDRON, left)
                }
                _ => vanilla_blocks::CAULDRON.default_state(),
            };
            world.set_block(pos, state, UpdateFlags::UPDATE_ALL);
        }
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use super::*;

    #[test]
    fn one_level_for_each_started_stack() {
        assert_eq!(water_levels(1), 1);
        assert_eq!(water_levels(64), 1);
        assert_eq!(water_levels(65), 2);
        assert_eq!(water_levels(128), 2);
        assert_eq!(water_levels(129), 3);
    }

    #[test]
    fn the_water_covers_what_it_can() {
        assert_eq!(share(64, 3, true), (64, 1));
        assert_eq!(share(10, 1, true), (10, 1));
        // Two stacks in one drop over one level: 64 change, the rest waits.
        assert_eq!(share(100, 1, true), (64, 1));
        assert_eq!(share(100, 3, true), (100, 2));
        assert_eq!(share(100, 1, false), (100, 0));
    }

    #[test]
    fn powder_names_give_their_concrete() {
        assert_eq!(
            concrete_of("red_concrete_powder").as_deref(),
            Some("red_concrete")
        );
        assert_eq!(
            concrete_of("light_blue_concrete_powder").as_deref(),
            Some("light_blue_concrete")
        );
        assert_eq!(concrete_of("red_concrete"), None);
        assert_eq!(concrete_of("_concrete_powder"), None);
    }

    #[test]
    fn dirt_becomes_mud_and_powder_concrete() {
        init_vanilla_registry();
        assert_eq!(
            converted(&vanilla_items::DIRT),
            Some((&*vanilla_items::MUD, false))
        );
        assert_eq!(
            converted(&vanilla_items::COARSE_DIRT),
            Some((&*vanilla_items::MUD, false))
        );
        assert_eq!(
            converted(&vanilla_items::ROOTED_DIRT),
            Some((&*vanilla_items::MUD, false))
        );
        assert_eq!(
            converted(&vanilla_items::LIME_CONCRETE_POWDER),
            Some((&*vanilla_items::LIME_CONCRETE, true))
        );
        assert_eq!(converted(&vanilla_items::GRASS_BLOCK), None);
        assert_eq!(converted(&vanilla_items::MUD), None);
        assert_eq!(converted(&vanilla_items::STICK), None);
    }
}
