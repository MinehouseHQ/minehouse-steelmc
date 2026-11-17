//! Vanilla `Bucketable`: mobs a water bucket scoops up into a mob bucket.

use simdnbt::owned::NbtCompound;
use steel_registry::data_components::vanilla_components::{
    BUCKET_ENTITY_DATA, CUSTOM_NAME, CustomData,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_items;
use steel_utils::types::InteractionHand;

use crate::behavior::InteractionResult;
use crate::entity::{LivingEntity, Mob, RemovalReason};
use crate::player::Player;
use crate::player::player_inventory::Prediction;

/// Vanilla `Bucketable`.
pub trait Bucketable: Mob {
    /// Vanilla `Bucketable.setFromBucket`.
    fn set_from_bucket_flag(&self, from_bucket: bool);

    /// Vanilla `Bucketable.saveToBucketTag`.
    fn save_to_bucket_tag(&self, bucket: &mut ItemStack) {
        save_default_data_to_bucket_tag(self, bucket);
    }

    /// Vanilla `Bucketable.loadFromBucketTag`.
    fn load_from_bucket_tag(&self, tag: &NbtCompound) {
        load_default_data_from_bucket_tag(self, tag);
    }

    /// Vanilla `Bucketable.getBucketItemStack`.
    fn bucket_item_stack(&self) -> ItemStack;

    /// Vanilla `Bucketable.getPickupSound`.
    fn bucket_pickup_sound(&self) -> SoundEventRef;

    /// Vanilla `Bucketable.canBePickedUpWithBucket`.
    fn can_be_picked_up_with_bucket(&self, stack: &ItemStack) -> bool {
        stack.is(&vanilla_items::WATER_BUCKET)
    }
}

/// Merges `update` into the bucket's `BUCKET_ENTITY_DATA` (vanilla `CustomData.update`).
pub fn update_bucket_entity_data(bucket: &mut ItemStack, update: impl FnOnce(&mut NbtCompound)) {
    let mut tag = bucket
        .get(BUCKET_ENTITY_DATA)
        .map_or_else(NbtCompound::new, CustomData::copy_tag);
    update(&mut tag);
    if let Some(data) = CustomData::try_from_compound(tag) {
        if data.is_empty() {
            bucket.remove(BUCKET_ENTITY_DATA);
        } else {
            bucket.set(BUCKET_ENTITY_DATA, data);
        }
    }
}

/// Vanilla `Bucketable.saveDefaultDataToBucketTag`.
pub fn save_default_data_to_bucket_tag<M: Mob + ?Sized>(mob: &M, bucket: &mut ItemStack) {
    match mob.custom_name() {
        Some(name) => bucket.set(CUSTOM_NAME, name),
        None => bucket.remove(CUSTOM_NAME),
    }
    update_bucket_entity_data(bucket, |tag| {
        if mob.is_no_ai() {
            tag.insert("NoAI", 1_i8);
        }
        if mob.is_silent() {
            tag.insert("Silent", 1_i8);
        }
        if mob.is_no_gravity() {
            tag.insert("NoGravity", 1_i8);
        }
        if mob.has_glowing_tag() {
            tag.insert("Glowing", 1_i8);
        }
        if mob.is_permanently_invulnerable() {
            tag.insert("Invulnerable", 1_i8);
        }
        if mob.is_persistence_required() {
            tag.insert("PersistenceRequired", 1_i8);
        }
        tag.insert("Health", mob.get_health());
    });
}

/// Vanilla `Bucketable.loadDefaultDataFromBucketTag`.
pub fn load_default_data_from_bucket_tag<M: Mob + ?Sized>(mob: &M, tag: &NbtCompound) {
    let flag = |key: &str| tag.byte(key).map(|value| value != 0);
    if let Some(no_ai) = flag("NoAI") {
        mob.set_no_ai(no_ai);
    }
    if let Some(silent) = flag("Silent") {
        mob.set_silent(silent);
    }
    if let Some(no_gravity) = flag("NoGravity") {
        mob.set_no_gravity(no_gravity);
    }
    if let Some(glowing) = flag("Glowing") {
        mob.set_glowing_tag(glowing);
    }
    if let Some(invulnerable) = flag("Invulnerable") {
        mob.set_invulnerable(invulnerable);
    }
    if flag("PersistenceRequired") == Some(true) {
        mob.set_persistence_required();
    }
    if let Some(health) = tag.float("Health") {
        mob.set_health(health);
    }
}

/// Vanilla `Bucketable.bucketMobPickup`: `None` when the held item cannot
/// scoop the mob up.
pub fn bucket_mob_pickup<M: Bucketable + ?Sized>(
    player: &Player,
    hand: InteractionHand,
    mob: &M,
) -> Option<InteractionResult> {
    let can_pick_up = {
        let inventory = player.inventory.lock();
        mob.can_be_picked_up_with_bucket(inventory.get_item_in_hand(hand))
    };
    if !can_pick_up || !LivingEntity::is_alive(mob) {
        return None;
    }

    mob.play_sound(mob.bucket_pickup_sound(), 1.0, 1.0);
    let mut bucket = mob.bucket_item_stack();
    mob.save_to_bucket_tag(&mut bucket);
    let overflow = player.inventory.lock().apply_filled_result(
        hand,
        bucket.clone(),
        player.has_infinite_materials(),
        false,
    );
    if !overflow.is_empty() {
        let _ = player.drop_item(overflow, false, Prediction::Predicted);
    }
    player.trigger_filled_bucket(&bucket);

    if let Some(leashable) = mob.as_leashable() {
        leashable.drop_leash();
    }
    mob.set_removed(RemovalReason::Discarded);
    Some(InteractionResult::Success)
}
