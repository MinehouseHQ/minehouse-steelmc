//! Trident item behavior (`TridentItem`).

use std::f32::consts::PI;
use std::sync::Arc;

use glam::DVec3;
use steel_macros::item_behavior;
use steel_math::trig;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::properties::Direction;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_events;
use steel_registry::stat::vanilla_stat_types;

use super::projectile_weapon::spawn_arrow;
use crate::behavior::item::ItemBehavior;
use crate::behavior::projectile_item::ProjectileItem;
use crate::behavior::{InteractionResult, ItemUseAnimation, UseItemContext};
use crate::enchantment_helper;
use crate::entity::entities::ThrownTridentEntity;
use crate::entity::{
    AbstractArrow as _, ArrowPickup, Entity, LivingEntity, Projectile as _, SharedEntity,
};
use crate::physics::MoverType;
use crate::world::World;

/// Vanilla `TridentItem.getUseDuration`.
const USE_DURATION: i32 = 72000;
/// Vanilla `TridentItem.THROW_THRESHOLD_TIME`: ticks to charge a throw.
const THROW_THRESHOLD_TIME: i32 = 10;
/// Vanilla `TridentItem.PROJECTILE_SHOOT_POWER`.
const PROJECTILE_SHOOT_POWER: f32 = 2.5;
/// Vanilla riptide spin: ticks and damage of `startAutoSpinAttack`.
const SPIN_ATTACK_TICKS: i32 = 20;
const SPIN_ATTACK_DAMAGE: f32 = 8.0;
/// Vanilla riptide lift when launched from the ground.
const SPIN_GROUND_LIFT: f64 = 1.199_999_9;
/// Vanilla `(float) (Math.PI / 180.0)`.
const DEGREES_TO_RADIANS: f32 = PI / 180.0;

/// Behavior for the trident item.
#[item_behavior(class = "TridentItem")]
pub struct TridentItem;

impl ItemBehavior for TridentItem {
    fn as_projectile_item(&self) -> Option<&dyn ProjectileItem> {
        Some(self)
    }

    /// Vanilla `TridentItem.use`: charge unless the next use would break it,
    /// or a riptide trident is out of water and rain.
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        let (breaks, riptide) = context.inv.with_item(|item| {
            (
                item.next_damage_will_break(),
                enchantment_helper::trident_spin_attack_strength(item) > 0.0,
            )
        });
        if breaks || (riptide && !context.player.is_in_water_or_rain()) {
            return InteractionResult::Fail;
        }
        context.player.start_using_item(context.hand);
        InteractionResult::Consume
    }

    fn get_use_animation(&self, _stack: &ItemStack) -> ItemUseAnimation {
        ItemUseAnimation::Trident
    }

    fn get_use_duration(&self, _stack: &ItemStack, _user: &dyn LivingEntity) -> i32 {
        USE_DURATION
    }

    /// Vanilla `TridentItem.releaseUsing`: throws the trident, or with
    /// Riptide in water or rain launches the player into a spin.
    fn release_using(
        &self,
        stack: &mut ItemStack,
        world: &Arc<World>,
        user: &dyn LivingEntity,
        time_left: i32,
    ) -> bool {
        let Some(player) = world.players.get_by_entity_id(user.id()) else {
            return false;
        };
        if self.get_use_duration(stack, user) - time_left < THROW_THRESHOLD_TIME {
            return false;
        }
        let riptide_strength = enchantment_helper::trident_spin_attack_strength(stack);
        if riptide_strength > 0.0 && (!player.is_in_water_or_rain() || player.is_passenger()) {
            return false;
        }
        if stack.next_damage_will_break() {
            return false;
        }
        let sound = enchantment_helper::pick_trident_sound(stack)
            .unwrap_or(&sound_events::ITEM_TRIDENT_THROW);
        player.award_stat(&vanilla_stat_types::ITEM_USED, stack.item());
        let has_infinite_materials = player.has_infinite_materials();
        stack.hurt_without_breaking(1, has_infinite_materials);

        if riptide_strength == 0.0 {
            let thrown = stack.consume_and_return(1, has_infinite_materials);
            let owner: SharedEntity = Arc::clone(&player) as SharedEntity;
            let trident = ThrownTridentEntity::thrown_by(world, &owner, &thrown);
            let (y_rot, x_rot) = player.rotation();
            trident.shoot_from_rotation(
                player.as_ref(),
                x_rot,
                y_rot,
                0.0,
                PROJECTILE_SHOOT_POWER,
                1.0,
            );
            if has_infinite_materials {
                trident.set_pickup(ArrowPickup::CreativeOnly);
            }
            let position = trident.position();
            let trident: SharedEntity = Arc::new(trident);
            spawn_arrow(world, &trident, &thrown, Some(&owner));
            world.play_sound_at(sound, SoundSource::Players, position, 1.0, 1.0, None);
            return true;
        }

        // Vanilla computes the launch with `Mth`'s float lookup tables.
        let (y_rot, x_rot) = player.rotation();
        let (y_rot, x_rot) = (y_rot * DEGREES_TO_RADIANS, x_rot * DEGREES_TO_RADIANS);
        let xd = -trig::sin(y_rot) * trig::cos(x_rot);
        let yd = -trig::sin(x_rot);
        let zd = trig::cos(y_rot) * trig::cos(x_rot);
        let scale = riptide_strength / (xd * xd + yd * yd + zd * zd).sqrt();
        player.push_impulse(DVec3::new(
            f64::from(xd * scale),
            f64::from(yd * scale),
            f64::from(zd * scale),
        ));
        player.start_auto_spin_attack(SPIN_ATTACK_TICKS, SPIN_ATTACK_DAMAGE, stack.clone());
        if player.on_ground() {
            let _ = Arc::clone(&player).move_entity(
                MoverType::SelfMovement,
                DVec3::new(0.0, SPIN_GROUND_LIFT, 0.0),
            );
        }
        world.play_sound_at(
            sound,
            SoundSource::Players,
            player.position(),
            1.0,
            1.0,
            None,
        );
        true
    }
}

impl ProjectileItem for TridentItem {
    fn as_projectile(
        &self,
        world: &Arc<World>,
        position: DVec3,
        stack: &ItemStack,
        _direction: Direction,
    ) -> SharedEntity {
        let trident = ThrownTridentEntity::at_position(world, position, &stack.copy_with_count(1));
        trident.set_pickup(ArrowPickup::Allowed);
        Arc::new(trident)
    }
}
