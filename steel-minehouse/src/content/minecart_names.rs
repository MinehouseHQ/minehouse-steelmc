//! Name tags on minecarts (the Fabric `MinecartNames`). A named name tag names a minecart
//! like a mob: a right click, sneaking or not, and the tag is used up (not in creative).
//! Vanilla only lets name tags on living entities. Any cart works: plain, chest, hopper,
//! furnace, TNT, spawner, command block.
//!
//! The name shows like a mob's, only while a player points at the cart: the vanilla client
//! does that for any entity with a custom name, so the cart only gets the name. A silent
//! word in the name (see `silent_tags`) makes the cart silent.

use steel_core::{behavior::InteractionResult, entity::Entity, player::Player};
use steel_protocol::packets::game::SoundSource;
use steel_registry::{
    data_components::vanilla_components::CUSTOM_NAME, sound_events, vanilla_items,
};
use steel_utils::{BlockPos, types::InteractionHand};

use super::silent_tags;

/// `player` used `hand` on `cart`. `Some` when a name tag was in it.
pub(super) fn interact(
    cart: &dyn Entity,
    player: &Player,
    hand: InteractionHand,
) -> Option<InteractionResult> {
    if player.is_spectator() {
        return None;
    }
    let held = player.inventory.lock().get_item_in_hand(hand).clone();
    if !held.is(&vanilla_items::NAME_TAG) {
        return None;
    }
    let name = held.get(CUSTOM_NAME)?.clone();
    if cart.custom_name().as_ref() == Some(&name) {
        // Already called that: the tag is kept.
        return Some(InteractionResult::Success);
    }
    cart.set_custom_name(Some(name.clone()));
    cart.set_custom_name_visible(false);
    silent_tags::named(cart, &name);
    if !player.has_infinite_materials() {
        player.inventory.lock().get_item_in_hand_mut(hand).shrink(1);
    }
    if let Some(world) = cart.level() {
        let at = cart.position();
        world.play_sound(
            &sound_events::ITEM_BOOK_PAGE_TURN,
            SoundSource::Neutral,
            BlockPos::containing(at.x, at.y, at.z),
            1.0,
            1.0,
            None,
        );
    }
    Some(InteractionResult::Success)
}
