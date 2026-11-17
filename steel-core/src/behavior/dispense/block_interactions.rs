//! Dispenser behaviors that work on the block or entity in front: the glass
//! bottle, glowstone, brush, honeycomb and water potion entries of Vanilla
//! `DispenseItemBehavior.bootStrap`, and the powder snow bucket.

use std::sync::atomic::{AtomicBool, Ordering};

use glam::DVec3;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::data_components::vanilla_components::{POTION_CONTENTS, PotionContents};
use steel_registry::item_stack::ItemStack;
use steel_registry::particle_type::ParticleData;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_fluid_tags::FluidTag;
use steel_registry::{
    REGISTRY, level_events, sound_events, vanilla_blocks, vanilla_game_events, vanilla_items,
    vanilla_particle_types, vanilla_potions,
};
use steel_utils::Downcast as _;
use steel_utils::types::UpdateFlags;

use super::entity_placement::target_box;
use super::{BlockSource, consume_with_remainder, default_dispense, default_execute};
use crate::behavior::ITEM_BEHAVIORS;
use crate::behavior::blocks::{BeehiveBlock, RespawnAnchorBlock};
use crate::behavior::waxables::get_waxed_from_normal_variant;
use crate::block_entity::entities::BeeReleaseStatus;
use crate::entity::entities::mobs::passive::ArmadilloEntity;
use crate::world::game_event::GameEventContext;

/// Honey level at which a glass bottle can be filled from a hive.
const FULL_HONEY_LEVEL: u8 = 5;
/// Durability a brush loses for each armadillo it brushes.
const BRUSH_DURABILITY_COST: i32 = 16;
/// Splash particles the water potion sends when it makes mud.
const MUD_SPLASH_PARTICLES: usize = 5;

/// Vanilla's brush behavior is a single `OptionalDispenseItemBehavior` whose
/// `success` flag is only ever cleared: after the first miss on the server,
/// every later brush clicks as a failure even when it brushes a scute off.
static BRUSH_SUCCESS: AtomicBool = AtomicBool::new(true);

/// Vanilla `Items.GLASS_BOTTLE` dispenser behavior: fills the bottle with honey
/// from a full hive or with water, or drops it as a failure.
pub(super) fn glass_bottle(source: &BlockSource<'_>, stack: ItemStack) -> (ItemStack, bool) {
    let world = source.world;
    let target = source.target();
    let state = world.get_block_state(target);
    if state.get_block().has_tag(&BlockTag::BEEHIVES)
        && state
            .try_get_value(&BlockStateProperties::LEVEL_HONEY)
            .is_some_and(|level| level >= FULL_HONEY_LEVEL)
    {
        BeehiveBlock::release_bees_and_reset_honey_level(
            world,
            state,
            target,
            None,
            BeeReleaseStatus::BeeReleased,
        );
        let honey = ItemStack::new(&vanilla_items::HONEY_BOTTLE);
        return (take_liquid(source, stack, honey), true);
    }
    if state.get_fluid_state().fluid_id.has_tag(&FluidTag::WATER) {
        let water =
            PotionContents::create_item_stack(&vanilla_items::POTION, &vanilla_potions::WATER);
        return (take_liquid(source, stack, water), true);
    }
    (default_execute(source, stack), false)
}

fn take_liquid(source: &BlockSource<'_>, stack: ItemStack, filled: ItemStack) -> ItemStack {
    source.world.game_event(
        &vanilla_game_events::FLUID_PICKUP,
        source.pos,
        &GameEventContext::new(None, None),
    );
    consume_with_remainder(source, stack, filled)
}

/// Vanilla `Items.GLOWSTONE` dispenser behavior: charges the respawn anchor in
/// front, fails on a full one, or drops the glowstone.
pub(super) fn glowstone(source: &BlockSource<'_>, mut stack: ItemStack) -> (ItemStack, bool) {
    let target = source.target();
    let state = source.world.get_block_state(target);
    if state.get_block() != &vanilla_blocks::RESPAWN_ANCHOR {
        return (default_execute(source, stack), true);
    }
    if state.get_value(&BlockStateProperties::RESPAWN_ANCHOR_CHARGES)
        == RespawnAnchorBlock::MAX_CHARGES
    {
        return (stack, false);
    }
    RespawnAnchorBlock::charge(None, source.world, target, state);
    stack.shrink(1);
    (stack, true)
}

/// Vanilla `Items.BRUSH` dispenser behavior: brushes a scute off an armadillo
/// in front.
pub(super) fn brush(source: &BlockSource<'_>, mut stack: ItemStack) -> (ItemStack, bool) {
    let armadillos = source
        .world
        .get_entities_in_aabb_matching(&target_box(source), |entity| {
            !entity.is_spectator() && entity.downcast_ref::<ArmadilloEntity>().is_some()
        });
    for entity in &armadillos {
        if let Some(armadillo) = entity.downcast_ref::<ArmadilloEntity>()
            && armadillo.brush_off_scute(&stack)
        {
            stack.hurt_and_break(BRUSH_DURABILITY_COST, false);
            return (stack, BRUSH_SUCCESS.load(Ordering::Relaxed));
        }
    }
    BRUSH_SUCCESS.store(false, Ordering::Relaxed);
    (stack, false)
}

/// Vanilla `Items.HONEYCOMB` dispenser behavior: waxes the copper block in
/// front, or drops the honeycomb.
pub(super) fn honeycomb(source: &BlockSource<'_>, mut stack: ItemStack) -> (ItemStack, bool) {
    let world = source.world;
    let target = source.target();
    let state = world.get_block_state(target);
    let Some(waxed_block) = get_waxed_from_normal_variant(state.get_block()) else {
        return (default_execute(source, stack), true);
    };
    let waxed = REGISTRY.blocks.copy_matching_properties(state, waxed_block);
    world.set_block(target, waxed, UpdateFlags::UPDATE_ALL);
    world.level_event(level_events::PARTICLES_WAX_ON, target, 0, None);
    world.play_sound(
        &sound_events::ITEM_HONEYCOMB_WAX_ON,
        SoundSource::Blocks,
        target,
        1.0,
        1.0,
        None,
    );
    stack.shrink(1);
    (stack, true)
}

/// Vanilla `Items.POTION` dispenser behavior: a water bottle turns the dirt in
/// front into mud; anything else is dropped.
pub(super) fn water_potion(source: &BlockSource<'_>, stack: ItemStack) -> ItemStack {
    let contents = stack.get_or_default(POTION_CONTENTS, PotionContents::empty());
    if !contents.is(&vanilla_potions::WATER) {
        return default_dispense(source, stack);
    }
    let world = source.world;
    let pos = source.pos;
    let target = source.target();
    if !world
        .get_block_state(target)
        .get_block()
        .has_tag(&BlockTag::CONVERTIBLE_TO_MUD)
    {
        return default_dispense(source, stack);
    }

    // Vanilla splashes over the dispenser itself, not over the new mud.
    for _ in 0..MUD_SPLASH_PARTICLES {
        world.send_particles(
            ParticleData::simple(&vanilla_particle_types::SPLASH),
            DVec3::new(
                f64::from(pos.x()) + rand::random::<f64>(),
                f64::from(pos.y()) + 1.0,
                f64::from(pos.z()) + rand::random::<f64>(),
            ),
            1,
            DVec3::ZERO,
            1.0,
        );
    }
    world.play_sound(
        &sound_events::ITEM_BOTTLE_EMPTY,
        SoundSource::Blocks,
        pos,
        1.0,
        1.0,
        None,
    );
    world.game_event(
        &vanilla_game_events::FLUID_PLACE,
        pos,
        &GameEventContext::new(None, None),
    );
    world.set_block(
        target,
        vanilla_blocks::MUD.default_state(),
        UpdateFlags::UPDATE_ALL,
    );
    consume_with_remainder(source, stack, ItemStack::new(&vanilla_items::GLASS_BOTTLE))
}

/// Vanilla filled-bucket dispenser behavior for a `SolidBucketItem`: places
/// the powder snow in front, or drops the bucket.
pub(super) fn solid_bucket(source: &BlockSource<'_>, stack: ItemStack) -> ItemStack {
    let Some(bucket) = ITEM_BEHAVIORS
        .get_behavior(stack.item)
        .as_solid_bucket_item()
    else {
        return default_dispense(source, stack);
    };
    if bucket.empty_contents(source.world, source.target()) {
        return consume_with_remainder(source, stack, ItemStack::new(&vanilla_items::BUCKET));
    }
    default_dispense(source, stack)
}

#[cfg(test)]
mod tests;
