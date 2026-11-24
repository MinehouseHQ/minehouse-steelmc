//! Waxed and invisible item frames (the Fabric `WaxedFrames`).
//!
//! Sneaking shears hide or show an item frame, empty or not; an invisible frame turns visible
//! again once its item is taken out, so an emptied one cannot get lost.
//!
//! Honeycomb waxes an item frame (sneak to put the honeycomb in instead), a sneaking axe
//! scrapes the wax off, like copper. A waxed frame is vanilla's fixed frame: the item cannot
//! be turned or taken and the frame cannot be broken. Right clicks on it go to the block it
//! hangs on, so a frame on a chest still opens the chest.

use glam::DVec3;
use steel_core::{
    behavior::InteractionResult,
    entity::{
        Entity as _, HangingEntity as _,
        entities::{ItemFrameEntity, objects::display_ui::ItemFrameObserver},
    },
    player::{Player, game_mode::use_block},
};
use steel_protocol::packets::game::SoundSource;
use steel_registry::{
    items::item::BlockHitResult, level_events, sound_event::SoundEventRef, sound_events,
    vanilla_item_tags::ItemTag, vanilla_items,
};
use steel_utils::types::InteractionHand;

/// Waxes, scrapes and hides item frames.
pub(super) struct WaxedFrames;

/// The one observer of item frames.
pub(super) static WAXED_FRAMES: WaxedFrames = WaxedFrames;

fn sound(frame: &ItemFrameEntity, sound: SoundEventRef) {
    if let Some(world) = frame.level() {
        world.play_sound(
            sound,
            SoundSource::Blocks,
            frame.frame_pos(),
            1.0,
            1.0,
            None,
        );
    }
}

/// Wears the tool in `hand` by one use.
fn wear(player: &Player, hand: InteractionHand) {
    let creative = player.has_infinite_materials();
    player
        .inventory
        .lock()
        .get_item_in_hand_mut(hand)
        .hurt_and_break(1, creative);
}

/// The same as clicking the block the frame hangs on, without placing the held item.
fn use_block_behind(
    frame: &ItemFrameEntity,
    player: &Player,
    hand: InteractionHand,
    location: DVec3,
) -> InteractionResult {
    // Vanilla skips the block when sneaking with something in hand.
    let holds_something = {
        let inventory = player.inventory.lock();
        !inventory
            .get_item_in_hand(InteractionHand::MainHand)
            .is_empty()
            || !inventory
                .get_item_in_hand(InteractionHand::OffHand)
                .is_empty()
    };
    if player.is_secondary_use_active() && holds_something {
        return InteractionResult::Pass;
    }
    let Some(world) = frame.level() else {
        return InteractionResult::Pass;
    };
    let direction = frame.direction();
    let hit = BlockHitResult {
        location,
        direction,
        block_pos: frame.frame_pos().relative(direction.opposite()),
        miss: false,
        inside: false,
        world_border_hit: false,
    };
    use_block(player, &world, hand, &hit)
}

impl ItemFrameObserver for WaxedFrames {
    fn interact(
        &self,
        frame: &ItemFrameEntity,
        player: &Player,
        hand: InteractionHand,
        location: DVec3,
    ) -> Option<InteractionResult> {
        if player.is_spectator() {
            return None;
        }
        let held = player.inventory.lock().get_item_in_hand(hand).clone();
        let sneaking = player.is_secondary_use_active();

        // Sneaking shears hide or show a frame, empty or not, waxed or not.
        if sneaking && held.is(&vanilla_items::SHEARS) {
            frame.set_invisible(!frame.is_invisible());
            wear(player, hand);
            sound(frame, &sound_events::ITEM_SHEARS_SNIP);
            return Some(InteractionResult::Success);
        }

        if !frame.is_fixed() {
            // Sneaking with honeycomb puts it into the frame like any other item.
            if !held.is(&vanilla_items::HONEYCOMB) || sneaking {
                return None;
            }
            frame.set_fixed(true);
            if !player.has_infinite_materials() {
                player.inventory.lock().get_item_in_hand_mut(hand).shrink(1);
            }
            sound(frame, &sound_events::ITEM_HONEYCOMB_WAX_ON);
            if let Some(world) = frame.level() {
                world.level_event(level_events::PARTICLES_WAX_ON, frame.frame_pos(), 0, None);
            }
            return Some(InteractionResult::Success);
        }

        if sneaking && !held.is_empty() && held.item().has_tag(&ItemTag::AXES) {
            frame.set_fixed(false);
            wear(player, hand);
            sound(frame, &sound_events::ITEM_AXE_WAX_OFF);
            if let Some(world) = frame.level() {
                world.level_event(level_events::PARTICLES_WAX_OFF, frame.frame_pos(), 0, None);
            }
            return Some(InteractionResult::Success);
        }

        Some(use_block_behind(frame, player, hand, location))
    }

    fn emptied(&self, frame: &ItemFrameEntity) {
        if frame.is_invisible() {
            frame.set_invisible(false);
        }
    }
}
