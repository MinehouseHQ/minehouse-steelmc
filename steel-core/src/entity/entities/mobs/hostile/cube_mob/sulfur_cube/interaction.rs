//! Players and the cube: feeding, lighting, shearing, swallowing, bucketing,
//! pushing, and picking blocks up from the ground.

use std::sync::Arc;

use glam::DVec3;
use steel_protocol::packets::game::{CTakeItemEntity, SoundSource};
use steel_registry::item_stack::ItemStack;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_game_rules::TNT_EXPLODES;
use steel_registry::{
    REGISTRY, RegistryExt as _, sound_events, vanilla_attributes, vanilla_game_events,
    vanilla_items,
};
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::InteractionHand;
use steel_utils::{ChunkPos, translations};
use text_components::TextComponent;

use super::SulfurCubeEntity;
use crate::behavior::InteractionResult;
use crate::entity::damage::DamageSource;
use crate::entity::entities::ItemEntity;
use crate::entity::{
    AgeableMob, Entity, LivingEntity, Mob, RemovalReason, SharedEntity, Shearable as _,
    bucket_mob_pickup,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::player::Player;
use crate::world::World;

/// Vanilla `PUSH_DISTANCE_THRESHOLD`.
const PUSH_DISTANCE_THRESHOLD: f64 = 1.3;
/// Vanilla `MAX_PLAYER_PUSH_SPEED`.
const MAX_PLAYER_PUSH_SPEED: f64 = 0.5;
/// Vanilla `PLAYER_PUSH_SPEED_SCALE_MULTIPLIER`.
const PLAYER_PUSH_SPEED_SCALE: f32 = 0.3;
/// Vanilla `VEHICLE_PUSH_SPEED_SCALE_MULTIPLIER`.
const VEHICLE_PUSH_SPEED_SCALE: f32 = 0.16;
/// Vanilla `VERTICAL_PUSH_MULTIPLIER`.
const VERTICAL_PUSH_MULTIPLIER: f32 = 0.3;

impl SulfurCubeEntity {
    /// Vanilla `SulfurCube.mobInteract`.
    pub(super) fn interact_with_player(
        &self,
        player: &Player,
        hand: InteractionHand,
    ) -> InteractionResult {
        let held = {
            let inventory = player.inventory.lock();
            let stack = inventory.get_item_in_hand(hand);
            stack.copy_with_count(stack.count())
        };
        if AgeableMob::is_baby(self) {
            if Self::is_food(&held) && self.can_age_up() {
                let age = self.get_age();
                Mob::use_player_item(self, player, hand);
                self.age_up(Self::get_speed_up_seconds_when_feeding(-age), true);
                self.make_sound(Some(&sound_events::ENTITY_SMALL_SULFUR_CUBE_EAT));
                return InteractionResult::Success;
            }
            return self.mob_interact_ageable(player, hand);
        }
        if self.is_primed() {
            return InteractionResult::Pass;
        }
        if self.can_explode()
            && (held.is(&vanilla_items::FLINT_AND_STEEL) || held.is(&vanilla_items::FIRE_CHARGE))
        {
            return self.light_with(player, hand, &held);
        }
        if held.is(&vanilla_items::SHEARS) && self.ready_for_shearing() {
            if let Some(world) = self.level() {
                self.shear(&world, SoundSource::Players, &held);
                self.game_event_with_source_entity(&vanilla_game_events::SHEAR, Some(player));
                player.hurt_item_in_hand(hand, 1);
                // TODO(advancements): PLAYER_SHEARED_EQUIPMENT with the swallowed block.
            }
            return InteractionResult::Success;
        }
        if Self::is_swallowable_item(&held) {
            if !self.equip_item(&held) {
                return InteractionResult::Pass;
            }
            Mob::use_player_item(self, player, hand);
            self.game_event(&vanilla_game_events::ENTITY_INTERACT);
            return InteractionResult::SuccessServer;
        }
        bucket_mob_pickup(player, hand, self)
            .unwrap_or_else(|| self.mob_interact_ageable(player, hand))
    }

    /// The flint and steel / fire charge branch of `mobInteract`.
    fn light_with(
        &self,
        player: &Player,
        hand: InteractionHand,
        held: &ItemStack,
    ) -> InteractionResult {
        if self
            .level()
            .is_some_and(|world| !world.get_game_rule(&TNT_EXPLODES))
        {
            player.send_overlay_message(&TextComponent::translated(
                translations::BLOCK_MINECRAFT_TNT_DISABLED.msg(),
            ));
            return InteractionResult::Pass;
        }
        self.prime_time(false);
        if held.is(&vanilla_items::FLINT_AND_STEEL) {
            player.hurt_item_in_hand(hand, 1);
        } else {
            Mob::use_player_item(self, player, hand);
        }
        player.award_stat(&vanilla_stat_types::ITEM_USED, held.item());
        InteractionResult::SuccessServer
    }

    /// Vanilla `equipItem`: swallows one of `held`, spitting out a different
    /// block it already holds.
    pub fn equip_item(&self, held: &ItemStack) -> bool {
        if AgeableMob::is_baby(self) {
            return false;
        }
        let swallowed = self.equipment_in_slot(EquipmentSlot::Body);
        if !swallowed.is_empty() {
            if held.is(swallowed.item()) {
                return false;
            }
            self.spawn_at_passenger_point(swallowed.copy_with_count(swallowed.count()));
        }
        self.set_body_and_drop_when_killed(held.copy_with_count(1));
        if !swallowed.is_empty() {
            let current = self.equipment_in_slot(EquipmentSlot::Body);
            self.apply_body_item_change(&swallowed, &current);
        }
        self.play_sound(&sound_events::ENTITY_SULFUR_CUBE_ABSORB, 1.0, 1.0);
        true
    }

    /// Vanilla `pickUpItem`: swallows one block from a stack on the ground.
    pub(super) fn swallow_item_entity(&self, world: &Arc<World>, item_entity: &ItemEntity) {
        let mut stack = item_entity.get_item();
        if !self.can_hold_item(&stack) || self.state.lock().pickup_timer > 0 {
            return;
        }
        // TODO(advancements): vanilla `onItemPickup` fires THROWN_ITEM_PICKED_UP_BY_ENTITY.
        let swallowed = stack.split(1);
        self.set_item_slot(EquipmentSlot::Body, swallowed);
        self.play_sound(&sound_events::ENTITY_SULFUR_CUBE_ABSORB, 1.0, 1.0);
        self.set_guaranteed_drop(EquipmentSlot::Body);
        world.broadcast_to_nearby(
            ChunkPos::from_entity_pos(item_entity.position()),
            CTakeItemEntity::new(item_entity.id(), self.id(), 1),
            None,
        );
        if stack.is_empty() {
            item_entity.set_removed(RemovalReason::Discarded);
        } else {
            item_entity.set_item(stack);
        }
    }

    /// Vanilla `applyContactDamage`: what the swallowed block does to whatever
    /// touches the cube.
    pub(super) fn apply_contact_damage(&self, target: &dyn Entity) {
        let damages = self.state.lock().contact_damages.clone();
        if damages.is_empty() {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };
        let attacker = world.get_entity_by_id(self.id());
        let mut random = LegacyRandom::from_seed(rand::random());
        for damage in damages {
            let Some(damage_type) = REGISTRY.damage_types.by_key(&damage.damage_type) else {
                continue;
            };
            let attribute = damage.attribute_to_source || target.as_player().is_some();
            let source = match (&attacker, attribute) {
                (Some(attacker), true) => DamageSource::direct(damage_type, Arc::clone(attacker)),
                _ => DamageSource::environment(damage_type),
            };
            target.hurt(&world, &source, damage.amount.sample(&mut random));
        }
    }

    /// Vanilla `playerPush`: a player walking into a cube holding a block
    /// kicks it along.
    pub(super) fn player_push(&self, player: &Arc<Player>) {
        if !self.has_body_item() {
            return;
        }
        let pusher: SharedEntity = if player.is_passenger() {
            player
                .root_vehicle()
                .unwrap_or_else(|| Arc::clone(player) as SharedEntity)
        } else {
            Arc::clone(player) as SharedEntity
        };
        let to_pusher = self.position() - pusher.position();
        let pusher_feet = pusher.position().y;
        let cube_bottom = self.position().y;
        let cube_top = cube_bottom + f64::from(self.base.dimensions().height);
        let pusher_top = pusher_feet + f64::from(pusher.base().dimensions().height);
        let horizontal_distance = (to_pusher.x * to_pusher.x + to_pusher.z * to_pusher.z).sqrt();
        if horizontal_distance >= PUSH_DISTANCE_THRESHOLD
            || pusher_feet > cube_top
            || pusher_top <= cube_bottom
        {
            return;
        }
        let resistance = self
            .attributes()
            .lock()
            .get_value(vanilla_attributes::KNOCKBACK_RESISTANCE)
            .unwrap_or(0.0);
        let knockback = (1.0 - resistance).max(0.0);
        let direction = DVec3::new(to_pusher.x, 0.0, to_pusher.z).normalize_or_zero() * knockback;
        let scale = if player.is_passenger() {
            VEHICLE_PUSH_SPEED_SCALE
        } else {
            PLAYER_PUSH_SPEED_SCALE
        };
        let player_speed = (player.known_speed().length() * 2.0 * f64::from(scale))
            .clamp(0.0, MAX_PLAYER_PUSH_SPEED);
        let vertical = if self.on_ground() {
            knockback * f64::from(VERTICAL_PUSH_MULTIPLIER)
        } else {
            0.0
        };
        let push = DVec3::new(direction.x, vertical, direction.z) * player_speed;
        self.mark_velocity_sync();
        self.play_push_sound(push);
        self.set_velocity(self.velocity() + push);
        self.apply_contact_damage(player.as_ref());
    }

    fn play_push_sound(&self, push: DVec3) {
        let sound = {
            let mut state = self.state.lock();
            let threshold = f64::from(state.sound_settings.push_sound_impulse_threshold);
            if push.length_squared() <= threshold * threshold || state.push_sound_cooldown > 0 {
                return;
            }
            state.push_sound_cooldown = (state.sound_settings.push_sound_cooldown * 20.0) as i32;
            state.sound_settings.push_sound.clone()
        };
        self.play_archetype_sound(&sound);
    }
}
