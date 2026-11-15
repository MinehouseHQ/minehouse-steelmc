//! Brewing the magic broth: milk in a cauldron, magic leaves into the milk over a fire, and
//! the broth ladled out with bowls. A port of the Fabric `Cauldrons`, `MilkCauldronBlock`,
//! `BrothCauldronBlock` and `BrothItem`.
//!
//! The two cauldrons are blocks of ours that need no pack: clients see the milk as a powder
//! snow cauldron and the broth as a water cauldron. So a client finds nothing to do with a
//! bucket of milk or a bowl there and also "uses" the item in hand, which would drink the
//! milk or the broth just ladled: after a click on a cauldron of ours the player's item use
//! is turned down for that tick ([`just_touched`]).

use std::{ptr, sync::Arc};

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_core::{
    behavior::{
        BlockBehavior, BlockPlaceContext, ITEM_BEHAVIORS, InteractionResult, InventoryAccess,
        ItemBehavior, UseItemContext, UseOnContext, blocks::CampfireBlock,
    },
    entity::{Entity as _, LivingEntity, LivingEntityRef, MobEffectInstance},
    inventory::lock::ContainerId,
    player::{
        Player,
        player_inventory::{PlayerInventory, Prediction},
    },
    world::World,
};
use steel_protocol::packets::game::SoundSource;
use steel_registry::{
    blocks::{
        Block, BlockRegistry, StateBooleanData, StateByteData,
        behavior::BlockConfig,
        block_state_ext::BlockStateExt as _,
        properties::{IntProperty, Property},
    },
    item_stack::ItemStack,
    items::item::BlockHitResult,
    particle_type::ParticleData,
    sound_event::SoundEventRef,
    sound_events as sounds, sound_types, vanilla_blocks, vanilla_items, vanilla_loot_tables,
    vanilla_mob_effects, vanilla_particle_types,
};
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, Identifier, locks::SyncMutex, types::UpdateFlags};
use text_components::{Modifier as _, TextComponent, format::Color};
use uuid::Uuid;

use super::{BROTH, LEAF};

/// How many leaves turn a full cauldron of milk into broth.
pub const LEAVES_NEEDED: u8 = 10;
/// How many bowls a cauldron of broth fills.
pub const SERVINGS_MAX: u8 = 5;
/// The broth sparkles this often, in ticks.
const SPARKLE_TICKS: i32 = 30;

/// Vanilla `MapColor.STONE`.
const MAP_COLOR_STONE: u8 = 11;

/// Buckets of milk in the cauldron: the vanilla cauldron `level`.
pub static LEVEL: IntProperty = IntProperty::new("level", 1, 3);
/// Leaves in the milk. The tenth makes it broth, so nine is the most a cauldron holds.
pub static LEAVES: IntProperty = IntProperty::new("leaves", 0, LEAVES_NEEDED - 1);
/// Bowls of broth left.
pub static SERVINGS: IntProperty = IntProperty::new("servings", 1, SERVINGS_MAX);

/// A cauldron of ours: shaped and mined like the vanilla one.
const fn cauldron(path: &'static str, properties: &'static [&'static dyn Property]) -> Block {
    let vanilla = &vanilla_blocks::CAULDRON;
    Block::new(
        Identifier::new_static("minehouse-herbs", path),
        BlockConfig::new()
            .can_occlude(false)
            .explosion_resistance(2.0)
            .destroy_time(2.0)
            .requires_correct_tool_for_drops(true)
            .sound_type(sound_types::STONE)
            // Either cauldron drops the cauldron, without what was in it.
            .loot_table(Some(&vanilla_loot_tables::BLOCKS_CAULDRON)),
        properties,
    )
    .with_light_properties(vanilla.light_properties)
    .with_map_color(StateByteData::new(MAP_COLOR_STONE, &[]))
    .with_suffocating(StateBooleanData::FALSE)
    .with_redstone_conductor(StateBooleanData::FALSE)
    .with_shapes(
        vanilla.collision_shape,
        vanilla.support_shape,
        vanilla.outline_shape,
        vanilla.occlusion_shape,
        vanilla.interaction_shape,
        vanilla.visual_shape,
    )
}

/// A cauldron with 1–3 buckets of milk and, when it is full, the leaves thrown in so far.
pub static MILK_CAULDRON: Block = cauldron("milk_cauldron", &[&LEAVES, &LEVEL]);
/// A cauldron of magic broth.
pub static BROTH_CAULDRON: Block = cauldron("broth_cauldron", &[&SERVINGS]);

/// A cauldron of broth just brewed: all its bowls.
fn full_of_broth() -> BlockStateId {
    BROTH_CAULDRON
        .default_state()
        .set_value(&SERVINGS, SERVINGS_MAX)
}

/// The water level a cauldron with `servings` bowls of broth is drawn at.
const fn broth_level(servings: u8) -> u8 {
    let level = (servings * 3).div_ceil(SERVINGS_MAX);
    if level == 0 { 1 } else { level }
}

/// The vanilla layered cauldron `block` filled to what `level` reads off a state of ours.
fn layered(blocks: &BlockRegistry, block: &'static Block, level: Option<&str>) -> BlockStateId {
    level
        .and_then(|level| blocks.state_id_from_block_properties(block, &[("level", level)]))
        .unwrap_or_else(|| blocks.get_default_state_id(block))
}

/// Has clients see milk as powder snow and broth as water, each to its level, and either
/// cauldron break like an empty one.
pub(super) fn show_to_clients(blocks: &mut BlockRegistry) {
    let empty = |blocks: &BlockRegistry, _| blocks.get_default_state_id(&vanilla_blocks::CAULDRON);
    super::blocks::show_as(
        blocks,
        &MILK_CAULDRON,
        |blocks, state| {
            let level = blocks.get_property_str(state, "level");
            layered(blocks, &vanilla_blocks::POWDER_SNOW_CAULDRON, level)
        },
        empty,
    );
    super::blocks::show_as(
        blocks,
        &BROTH_CAULDRON,
        |blocks, state| {
            let servings = blocks
                .get_property_str(state, "servings")
                .and_then(|servings| servings.parse().ok())
                .unwrap_or(SERVINGS_MAX);
            let level = broth_level(servings).to_string();
            layered(blocks, &vanilla_blocks::WATER_CAULDRON, Some(&level))
        },
        empty,
    );
}

/// The game time of each player's last click on a cauldron of ours.
static TOUCHED: SyncMutex<Option<FxHashMap<Uuid, i64>>> = SyncMutex::new(None);

fn touched(player: &Player, world: &World) {
    TOUCHED
        .lock()
        .get_or_insert_default()
        .insert(player.uuid(), world.game_time());
}

/// Whether the player's click this tick went to a cauldron of ours, so the item use that
/// comes with it is not one the player meant.
fn just_touched(player: &Player, world: &World) -> bool {
    let at = TOUCHED
        .lock()
        .as_ref()
        .and_then(|touched| touched.get(&player.uuid()).copied());
    // The two packets of one click may fall on either side of a tick.
    at.is_some_and(|at| (0..=1).contains(&(world.game_time() - at)))
}

fn overlay(player: &Player, text: String, color: Color) {
    player.send_overlay_message(&TextComponent::plain(text).color(color));
}

/// Tells the player how far the brew is.
fn status(player: &Player, milk: u8, leaves: u8) {
    let text = if milk < 3 {
        format!("Молоко: {milk}/3")
    } else {
        format!("Магических листьев: {leaves}/{LEAVES_NEEDED}")
    };
    overlay(player, text, Color::LightPurple);
}

/// Swaps one of the stack in `hand` for `result`, as a bucket is filled or emptied.
fn fill_result(player: &Player, hand: InteractionHand, inv: &InventoryAccess, result: ItemStack) {
    let overflow = inv.with_guard(|guard| {
        let id = ContainerId::from_arc(&player.inventory);
        let Some(inventory) = guard.get_typed_mut::<PlayerInventory>(id) else {
            return result;
        };
        inventory.apply_filled_result(hand, result, player.has_infinite_materials(), false)
    });
    if !overflow.is_empty() {
        let _ = player.drop_item(overflow, false, Prediction::Predicted);
    }
}

fn center(pos: BlockPos, height: f64) -> DVec3 {
    DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + height,
        f64::from(pos.z()) + 0.5,
    )
}

fn block_sound(world: &World, sound: SoundEventRef, pos: BlockPos, volume: f32, pitch: f32) {
    world.play_sound(sound, SoundSource::Blocks, pos, volume, pitch, None);
}

/// Pours a bucket of milk into the cauldron at `pos`, which then holds `milk` buckets.
fn pour_milk(
    world: &Arc<World>,
    pos: BlockPos,
    player: &Player,
    hand: InteractionHand,
    inv: &InventoryAccess,
    milk: u8,
) {
    touched(player, world);
    fill_result(player, hand, inv, ItemStack::new(&vanilla_items::BUCKET));
    let state = MILK_CAULDRON.default_state().set_value(&LEVEL, milk);
    world.set_block(pos, state, UpdateFlags::UPDATE_ALL);
    block_sound(world, &sounds::ITEM_BUCKET_EMPTY, pos, 1.0, 1.0);
    status(player, milk, 0);
}

/// Whether a lit campfire is right under `pos`: the milk does not brew cold.
fn heated(world: &World, pos: BlockPos) -> bool {
    CampfireBlock::is_lit_campfire(world.get_block_state(pos.below()))
}

/// One more leaf in a full cauldron of milk; the last one brews the broth.
fn add_leaf(world: &Arc<World>, pos: BlockPos, state: BlockStateId) -> u8 {
    let leaves = state.get_value(&LEAVES) + 1;
    let top = center(pos, 0.95);
    let particles = |particle, count, spread: DVec3, speed| {
        world.send_particles(ParticleData::simple(particle), top, count, spread, speed);
    };
    if leaves >= LEAVES_NEEDED {
        world.set_block(pos, full_of_broth(), UpdateFlags::UPDATE_ALL);
        block_sound(world, &sounds::BLOCK_BREWING_STAND_BREW, pos, 1.0, 0.8);
        block_sound(world, &sounds::BLOCK_ENCHANTMENT_TABLE_USE, pos, 0.8, 1.2);
        particles(
            &vanilla_particle_types::WITCH,
            30,
            DVec3::new(0.3, 0.2, 0.3),
            0.05,
        );
        particles(&vanilla_particle_types::PORTAL, 40, DVec3::splat(0.3), 0.3);
    } else {
        let state = state.set_value(&LEAVES, leaves);
        world.set_block(pos, state, UpdateFlags::UPDATE_ALL);
        block_sound(world, &sounds::ENTITY_GENERIC_SPLASH, pos, 0.4, 1.4);
        let spread = DVec3::new(0.25, 0.05, 0.25);
        particles(&vanilla_particle_types::HAPPY_VILLAGER, 4, spread, 0.0);
    }
    leaves
}

/// How the cauldron of milk behaves.
pub(super) struct MilkCauldron;

impl MilkCauldron {
    /// A leaf from the hand into the milk, if the cauldron is full and over a fire.
    fn take_leaf(
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        inv: &InventoryAccess,
    ) -> InteractionResult {
        let milk = state.get_value(&LEVEL);
        if milk < 3 {
            let text = format!("Сначала налей 3 ведра молока ({milk}/3)");
            overlay(player, text, Color::Yellow);
            return InteractionResult::Fail;
        }
        if !heated(world, pos) {
            let text = "Разожги костёр под котлом, без огня отвар не сварится";
            overlay(player, text.to_owned(), Color::Yellow);
            return InteractionResult::Fail;
        }
        if !player.has_infinite_materials() {
            inv.with_item(|stack| stack.shrink(1));
        }
        let leaves = add_leaf(world, pos, state);
        status(player, 3, leaves);
        InteractionResult::Success
    }
}

impl BlockBehavior for MilkCauldron {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(MILK_CAULDRON.default_state())
    }

    fn use_item_on(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        hand: InteractionHand,
        _hit_result: &BlockHitResult,
        inv: &mut InventoryAccess,
    ) -> InteractionResult {
        let milk = state.get_value(&LEVEL);
        let leaves = state.get_value(&LEAVES);
        let held = inv.with_item(|stack| stack.clone());
        if LEAF.is(&held) {
            touched(player, world);
            return Self::take_leaf(state, world, pos, player, inv);
        }
        if super::id_of(&held).is_some() {
            return InteractionResult::TryEmptyHandInteraction;
        }
        if held.is(&vanilla_items::MILK_BUCKET) {
            touched(player, world);
            if milk >= 3 || leaves > 0 {
                return InteractionResult::Fail;
            }
            pour_milk(world, pos, player, hand, inv, milk + 1);
            return InteractionResult::Success;
        }
        if held.is(&vanilla_items::BUCKET) {
            touched(player, world);
            // Milk goes back into a bucket only as it came: three buckets, nothing added.
            if milk < 3 || leaves > 0 {
                return InteractionResult::Fail;
            }
            fill_result(
                player,
                hand,
                inv,
                ItemStack::new(&vanilla_items::MILK_BUCKET),
            );
            let empty = vanilla_blocks::CAULDRON.default_state();
            world.set_block(pos, empty, UpdateFlags::UPDATE_ALL);
            block_sound(world, &sounds::ITEM_BUCKET_FILL, pos, 1.0, 1.0);
            return InteractionResult::Success;
        }
        InteractionResult::TryEmptyHandInteraction
    }
}

/// How the cauldron of broth behaves.
pub(super) struct BrothCauldron;

impl BlockBehavior for BrothCauldron {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(full_of_broth())
    }

    fn use_item_on(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        hand: InteractionHand,
        _hit_result: &BlockHitResult,
        inv: &mut InventoryAccess,
    ) -> InteractionResult {
        let is_bowl =
            inv.with_item(|stack| stack.is(&vanilla_items::BOWL) && super::id_of(stack).is_none());
        if !is_bowl {
            return InteractionResult::TryEmptyHandInteraction;
        }
        touched(player, world);
        fill_result(player, hand, inv, BROTH.stack(1));
        let servings = state.get_value(&SERVINGS) - 1;
        let left = if servings > 0 {
            state.set_value(&SERVINGS, servings)
        } else {
            vanilla_blocks::CAULDRON.default_state()
        };
        world.set_block(pos, left, UpdateFlags::UPDATE_ALL);
        block_sound(world, &sounds::ITEM_BOTTLE_FILL, pos, 1.0, 0.9);
        InteractionResult::Success
    }

    fn on_place(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        old_state: BlockStateId,
        _moved_by_piston: bool,
    ) {
        if !ptr::eq(old_state.get_block(), &raw const BROTH_CAULDRON) {
            world.schedule_block_tick_default(pos, &BROTH_CAULDRON, SPARKLE_TICKS);
        }
    }

    /// Magic sparkles over the broth while it lasts.
    fn tick(&self, _state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        world.send_particles(
            ParticleData::simple(&vanilla_particle_types::WITCH),
            center(pos, 0.9),
            2,
            DVec3::new(0.2, 0.05, 0.2),
            0.0,
        );
        world.schedule_block_tick_default(pos, &BROTH_CAULDRON, SPARKLE_TICKS);
    }
}

/// A bucket of milk: poured into an empty cauldron, and not drunk by the click that poured
/// it. Otherwise the vanilla bucket.
pub(super) struct MilkBucket;

impl ItemBehavior for MilkBucket {
    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        let pos = context.hit_result.block_pos;
        let is_cauldron = ptr::eq(
            context.world.get_block_state(pos).get_block(),
            &raw const vanilla_blocks::CAULDRON,
        );
        if !is_cauldron || context.player.is_spectator() {
            return ITEM_BEHAVIORS
                .get_behavior(&vanilla_items::MILK_BUCKET)
                .use_on(context);
        }
        let (world, player, hand) = (context.world, context.player, context.hand);
        pour_milk(world, pos, player, hand, &context.inv, 1);
        InteractionResult::Success
    }

    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        if just_touched(context.player, context.world) {
            return InteractionResult::Fail;
        }
        ITEM_BEHAVIORS
            .get_behavior(&vanilla_items::MILK_BUCKET)
            .use_item(context)
    }
}

/// What the broth does to whoever drinks it: four more hearts and slowness for two
/// minutes, and ten seconds off the ground.
fn drink_broth(world: &World, entity: &dyn LivingEntity) {
    const TWO_MINUTES: i32 = 2 * 60 * 20;
    for (effect, ticks, amplifier) in [
        (&vanilla_mob_effects::ABSORPTION, TWO_MINUTES, 1),
        (&vanilla_mob_effects::SLOWNESS, TWO_MINUTES, 0),
        (&vanilla_mob_effects::LEVITATION, 10 * 20, 0),
    ] {
        let _ = entity.add_mob_effect(MobEffectInstance::with_duration(effect, ticks, amplifier));
    }
    world.send_particles(
        ParticleData::simple(&vanilla_particle_types::WITCH),
        entity.position() + DVec3::Y,
        20,
        DVec3::new(0.4, 0.6, 0.4),
        0.05,
    );
}

/// «Магический отвар» in a bowl.
pub(super) struct Broth;

impl ItemBehavior for Broth {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        if just_touched(context.player, context.world) {
            return InteractionResult::Fail;
        }
        ITEM_BEHAVIORS.get_behavior(BROTH.base).use_item(context)
    }

    fn finish_using(
        &self,
        stack: &mut ItemStack,
        world: &Arc<World>,
        user: LivingEntityRef<'_>,
    ) -> ItemStack {
        drink_broth(world, user.living());
        ITEM_BEHAVIORS
            .get_behavior(BROTH.base)
            .finish_using(stack, world, user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broth_is_drawn_lower_as_bowls_are_taken() {
        let levels: Vec<u8> = (1..=SERVINGS_MAX).map(broth_level).collect();
        assert_eq!(levels, [1, 2, 2, 3, 3]);
    }

    #[test]
    fn a_cauldron_of_ours_is_shaped_and_mined_like_the_vanilla_one() {
        let vanilla = &vanilla_blocks::CAULDRON;
        for block in [&MILK_CAULDRON, &BROTH_CAULDRON] {
            assert_eq!(block.get_collision_shape(0), vanilla.get_collision_shape(0));
            assert_eq!(block.config.destroy_time, vanilla.config.destroy_time);
        }
        assert_eq!(MILK_CAULDRON.state_count(), 30);
        assert_eq!(BROTH_CAULDRON.state_count(), 5);
    }
}
