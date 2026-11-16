//! Vanilla `CauldronInteractions`: what a held item does to a cauldron.
//!
//! Each cauldron block owns one dispatcher (`EMPTY`, `WATER`, `LAVA`,
//! `POWDER_SNOW`). Tag entries are checked before item entries and an
//! unmatched item falls through to the empty-hand interaction, exactly like
//! vanilla `CauldronInteraction.Dispatcher.get`.

use std::sync::Arc;

use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, IntProperty};
use steel_registry::data_components::vanilla_components::{
    BANNER_PATTERNS, BannerPatternLayers, DYED_COLOR, POTION_CONTENTS, PotionContents,
};
use steel_registry::fluid::FluidStateExt as _;
use steel_registry::game_events::GameEventRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    sound_events, vanilla_blocks, vanilla_custom_stats, vanilla_game_events, vanilla_items,
    vanilla_potions,
};
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::{InteractionResult, InventoryAccess};
use crate::player::Player;
use crate::player::player_inventory::Prediction;
use crate::world::World;
use crate::world::game_event::GameEventContext;

const LEVEL_CAULDRON: &IntProperty = &BlockStateProperties::LEVEL_CAULDRON;

/// Vanilla `CauldronInteraction.Dispatcher`: which interaction table a
/// cauldron uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CauldronDispatcher {
    /// `CauldronInteractions.EMPTY`.
    Empty,
    /// `CauldronInteractions.WATER`.
    Water,
    /// `CauldronInteractions.LAVA`.
    Lava,
    /// `CauldronInteractions.POWDER_SNOW`.
    PowderSnow,
}

/// One vanilla interaction lambda.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interaction {
    FillWater,
    FillLava,
    FillPowderSnow,
    EmptyWaterPotion,
    FillBucket,
    FillBottle,
    AddWaterPotion,
    RemoveDye,
    CleanBanner,
    CleanShulkerBox,
}

impl CauldronDispatcher {
    /// Vanilla `Dispatcher.get`: tags first, then the item map.
    fn get(self, stack: &ItemStack) -> Option<Interaction> {
        let item = stack.item();
        if self == Self::Water && item.has_tag(&ItemTag::CAULDRON_CAN_REMOVE_DYE) {
            return Some(Interaction::RemoveDye);
        }
        // `addDefaultInteractions` is registered on every dispatcher.
        if stack.is(&vanilla_items::WATER_BUCKET) {
            return Some(Interaction::FillWater);
        }
        if stack.is(&vanilla_items::LAVA_BUCKET) {
            return Some(Interaction::FillLava);
        }
        if stack.is(&vanilla_items::POWDER_SNOW_BUCKET) {
            return Some(Interaction::FillPowderSnow);
        }
        match self {
            Self::Empty => stack
                .is(&vanilla_items::POTION)
                .then_some(Interaction::EmptyWaterPotion),
            Self::Water => {
                if stack.is(&vanilla_items::BUCKET) {
                    Some(Interaction::FillBucket)
                } else if stack.is(&vanilla_items::GLASS_BOTTLE) {
                    Some(Interaction::FillBottle)
                } else if stack.is(&vanilla_items::POTION) {
                    Some(Interaction::AddWaterPotion)
                } else if item.has_tag(&ItemTag::BANNERS) {
                    Some(Interaction::CleanBanner)
                } else if item.has_tag(&ItemTag::SHULKER_BOXES)
                    && !stack.is(&vanilla_items::SHULKER_BOX)
                {
                    // `Items.DYED_SHULKER_BOX`: the sixteen colored boxes.
                    Some(Interaction::CleanShulkerBox)
                } else {
                    None
                }
            }
            Self::Lava | Self::PowderSnow => stack
                .is(&vanilla_items::BUCKET)
                .then_some(Interaction::FillBucket),
        }
    }

    /// Vanilla `AbstractCauldronBlock.useItemOn`.
    pub fn use_item_on(
        self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        hand: InteractionHand,
        inv: &InventoryAccess,
    ) -> InteractionResult {
        let held = inv.with_item(|stack| stack.clone());
        let Some(interaction) = self.get(&held) else {
            return InteractionResult::TryEmptyHandInteraction;
        };
        let use_ctx = Use {
            state,
            world,
            pos,
            player,
            hand,
            inv,
            held: &held,
        };
        match interaction {
            Interaction::FillWater => use_ctx.empty_bucket(
                vanilla_blocks::WATER_CAULDRON
                    .default_state()
                    .set_value(LEVEL_CAULDRON, 3),
                &sound_events::ITEM_BUCKET_EMPTY,
            ),
            Interaction::FillLava => {
                if is_under_water(world, pos) {
                    InteractionResult::Consume
                } else {
                    use_ctx.empty_bucket(
                        vanilla_blocks::LAVA_CAULDRON.default_state(),
                        &sound_events::ITEM_BUCKET_EMPTY_LAVA,
                    )
                }
            }
            Interaction::FillPowderSnow => {
                if is_under_water(world, pos) {
                    InteractionResult::Consume
                } else {
                    use_ctx.empty_bucket(
                        vanilla_blocks::POWDER_SNOW_CAULDRON
                            .default_state()
                            .set_value(LEVEL_CAULDRON, 3),
                        &sound_events::ITEM_BUCKET_EMPTY_POWDER_SNOW,
                    )
                }
            }
            Interaction::FillBucket => self.fill_bucket(&use_ctx),
            Interaction::EmptyWaterPotion => use_ctx.empty_water_potion(None),
            Interaction::AddWaterPotion => {
                if state.get_value(LEVEL_CAULDRON) == LEVEL_CAULDRON.max {
                    InteractionResult::TryEmptyHandInteraction
                } else {
                    use_ctx.empty_water_potion(Some(
                        state.set_value(LEVEL_CAULDRON, state.get_value(LEVEL_CAULDRON) + 1),
                    ))
                }
            }
            Interaction::FillBottle => use_ctx.fill_bottle(),
            Interaction::RemoveDye => use_ctx.remove_dye(),
            Interaction::CleanBanner => use_ctx.clean_banner(),
            Interaction::CleanShulkerBox => use_ctx.clean_shulker_box(),
        }
    }

    /// The bucket entries of `WATER`, `LAVA` and `POWDER_SNOW`.
    fn fill_bucket(self, use_ctx: &Use<'_>) -> InteractionResult {
        let (result, sound, can_fill) = match self {
            Self::Water => (
                &vanilla_items::WATER_BUCKET,
                &sound_events::ITEM_BUCKET_FILL,
                use_ctx.state.get_value(LEVEL_CAULDRON) == LEVEL_CAULDRON.max,
            ),
            Self::Lava => (
                &vanilla_items::LAVA_BUCKET,
                &sound_events::ITEM_BUCKET_FILL_LAVA,
                true,
            ),
            Self::PowderSnow => (
                &vanilla_items::POWDER_SNOW_BUCKET,
                &sound_events::ITEM_BUCKET_FILL_POWDER_SNOW,
                use_ctx.state.get_value(LEVEL_CAULDRON) == LEVEL_CAULDRON.max,
            ),
            Self::Empty => return InteractionResult::TryEmptyHandInteraction,
        };
        if !can_fill {
            return InteractionResult::TryEmptyHandInteraction;
        }
        use_ctx.fill_bucket(ItemStack::new(result), sound);
        InteractionResult::Success
    }
}

/// The arguments every vanilla interaction lambda receives.
struct Use<'a> {
    state: BlockStateId,
    world: &'a Arc<World>,
    pos: BlockPos,
    player: &'a Player,
    hand: InteractionHand,
    inv: &'a InventoryAccess,
    held: &'a ItemStack,
}

impl Use<'_> {
    /// Vanilla `player.setItemInHand(hand, ItemUtils.createFilledResult(...))`.
    fn set_filled_result(&self, result: ItemStack, limit_creative_stack_size: bool) {
        let infinite = self.player.has_infinite_materials();
        let overflow = self.inv.with_inventory(|inventory| {
            inventory.apply_filled_result(self.hand, result, infinite, limit_creative_stack_size)
        });
        if !overflow.is_empty() {
            let _ = self
                .player
                .drop_item(overflow, false, Prediction::Predicted);
        }
    }

    fn award_item_used(&self) {
        self.player
            .award_stat(&vanilla_stat_types::ITEM_USED, self.held.item());
    }

    fn set_block(&self, state: BlockStateId) {
        self.world
            .set_block(self.pos, state, UpdateFlags::UPDATE_ALL);
    }

    fn play_sound(&self, sound: SoundEventRef) {
        self.world
            .play_sound(sound, SoundSource::Blocks, self.pos, 1.0, 1.0, None);
    }

    fn game_event(&self, event: GameEventRef) {
        self.world
            .game_event(event, self.pos, &GameEventContext::new(None, None));
    }

    /// Vanilla `CauldronInteractions.fillBucket` once `canFill` passed.
    fn fill_bucket(&self, result: ItemStack, sound: SoundEventRef) {
        self.set_filled_result(result, true);
        self.player
            .award_custom_stat(&vanilla_custom_stats::USE_CAULDRON);
        self.award_item_used();
        self.set_block(vanilla_blocks::CAULDRON.default_state());
        self.play_sound(sound);
        self.game_event(&vanilla_game_events::FLUID_PICKUP);
    }

    /// Vanilla `CauldronInteractions.emptyBucket`.
    fn empty_bucket(&self, new_state: BlockStateId, sound: SoundEventRef) -> InteractionResult {
        self.set_filled_result(ItemStack::new(&vanilla_items::BUCKET), true);
        self.player
            .award_custom_stat(&vanilla_custom_stats::FILL_CAULDRON);
        self.award_item_used();
        self.set_block(new_state);
        self.play_sound(sound);
        self.game_event(&vanilla_game_events::FLUID_PLACE);
        InteractionResult::Success
    }

    /// The `POTION` entries of `EMPTY` (`new_state` `None`: a level 1 water
    /// cauldron) and `WATER` (the cycled level).
    fn empty_water_potion(&self, new_state: Option<BlockStateId>) -> InteractionResult {
        let is_water = self
            .held
            .get(POTION_CONTENTS)
            .is_some_and(|potion| potion.is(&vanilla_potions::WATER));
        if !is_water {
            return InteractionResult::TryEmptyHandInteraction;
        }
        self.set_filled_result(ItemStack::new(&vanilla_items::GLASS_BOTTLE), true);
        self.player
            .award_custom_stat(&vanilla_custom_stats::USE_CAULDRON);
        self.award_item_used();
        self.set_block(new_state.unwrap_or_else(|| vanilla_blocks::WATER_CAULDRON.default_state()));
        self.play_sound(&sound_events::ITEM_BOTTLE_EMPTY);
        self.game_event(&vanilla_game_events::FLUID_PLACE);
        InteractionResult::Success
    }

    /// The `GLASS_BOTTLE` entry of `WATER`.
    fn fill_bottle(&self) -> InteractionResult {
        self.set_filled_result(
            PotionContents::create_item_stack(&vanilla_items::POTION, &vanilla_potions::WATER),
            true,
        );
        self.player
            .award_custom_stat(&vanilla_custom_stats::USE_CAULDRON);
        self.award_item_used();
        lower_fill_level(self.state, self.world, self.pos);
        self.play_sound(&sound_events::ITEM_BOTTLE_FILL);
        self.game_event(&vanilla_game_events::FLUID_PICKUP);
        InteractionResult::Success
    }

    /// Vanilla `CauldronInteractions.dyedItemIteration`.
    fn remove_dye(&self) -> InteractionResult {
        if !self.held.has(DYED_COLOR) {
            return InteractionResult::TryEmptyHandInteraction;
        }
        self.inv.with_item(|stack| stack.remove(DYED_COLOR));
        self.player
            .award_custom_stat(&vanilla_custom_stats::CLEAN_ARMOR);
        lower_fill_level(self.state, self.world, self.pos);
        InteractionResult::Success
    }

    /// Vanilla `CauldronInteractions.bannerInteraction`.
    fn clean_banner(&self) -> InteractionResult {
        let layers = self
            .held
            .get(BANNER_PATTERNS)
            .map_or(&[][..], BannerPatternLayers::layers);
        let Some((_, kept)) = layers.split_last() else {
            return InteractionResult::TryEmptyHandInteraction;
        };
        let mut cleaned = self.held.copy_with_count(1);
        cleaned.set(BANNER_PATTERNS, BannerPatternLayers::new(kept.to_vec()));
        self.set_filled_result(cleaned, false);
        self.player
            .award_custom_stat(&vanilla_custom_stats::CLEAN_BANNER);
        lower_fill_level(self.state, self.world, self.pos);
        InteractionResult::Success
    }

    /// Vanilla `CauldronInteractions.shulkerBoxInteraction`: `transmuteCopy`
    /// keeps the box's contents and name.
    fn clean_shulker_box(&self) -> InteractionResult {
        let cleaned = ItemStack::with_count_and_patch(
            &vanilla_items::SHULKER_BOX,
            1,
            self.held.components_patch().clone(),
        );
        self.set_filled_result(cleaned, false);
        self.player
            .award_custom_stat(&vanilla_custom_stats::CLEAN_SHULKER_BOX);
        lower_fill_level(self.state, self.world, self.pos);
        InteractionResult::Success
    }
}

/// Vanilla `LayeredCauldronBlock.lowerFillLevel`.
pub fn lower_fill_level(state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
    let new_level = state.get_value(LEVEL_CAULDRON) - 1;
    let new_state = if new_level == 0 {
        vanilla_blocks::CAULDRON.default_state()
    } else {
        state.set_value(LEVEL_CAULDRON, new_level)
    };
    world.set_block(pos, new_state, UpdateFlags::UPDATE_ALL);
    world.game_event(
        &vanilla_game_events::BLOCK_CHANGE,
        pos,
        &GameEventContext::new(None, Some(new_state)),
    );
}

/// Vanilla `CauldronInteractions.isUnderWater`.
fn is_under_water(world: &World, pos: BlockPos) -> bool {
    world
        .get_block_state(pos.above())
        .get_fluid_state()
        .is_water()
}

#[cfg(test)]
#[path = "cauldron_interactions_tests.rs"]
mod tests;
