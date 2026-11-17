//! Vanilla `KineticWeapon.damageEntities` and `LivingEntity.stabAttack`: a
//! spear in use stabs what it runs into.

use glam::DVec3;
use steel_registry::data_components::components::{KineticWeapon, KineticWeaponCondition};
use steel_registry::item_stack::ItemStack;
use steel_registry::{REGISTRY, TaggedRegistryExt as _, vanilla_attributes};
use steel_utils::Downcast as _;
use steel_utils::entity_events::EntityStatus;
use steel_utils::types::InteractionHand;

use crate::behavior::ITEM_BEHAVIORS;
use crate::enchantment_helper::{self, EnchantmentDamageContext, EnchantmentPostAttackContext};
use crate::entity::KnockbackCause;
use crate::entity::entities::EnderDragonPartEntity;
use crate::entity::piercing::hit_entities_along;
use crate::entity::{Entity, LivingEntity, Mob, SharedEntity};
use crate::inventory::equipment::EquipmentSlot;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;

/// Vanilla `KineticWeapon.getMotion`: the speed, in blocks per second, an
/// entity charges with. A mob passenger moves with its root vehicle.
pub(crate) fn kinetic_motion(entity: &dyn Entity) -> DVec3 {
    if entity.as_player().is_none()
        && entity.is_passenger()
        && let Some(root) = entity.root_vehicle()
    {
        return root.known_speed() * 20.0;
    }
    entity.known_speed() * 20.0
}

/// Vanilla `KineticWeapon.Condition.test`.
pub(crate) fn condition_holds(
    condition: Option<&KineticWeaponCondition>,
    ticks_used: i32,
    attacker_speed: f64,
    relative_speed: f64,
    entity_factor: f64,
) -> bool {
    condition.is_some_and(|condition| {
        ticks_used <= condition.max_duration_ticks()
            && attacker_speed >= f64::from(condition.min_speed()) * entity_factor
            && relative_speed >= f64::from(condition.min_relative_speed()) * entity_factor
    })
}

/// Vanilla `EnderDragonPart.parentMob`: a stab on a dragon part lands on the
/// dragon.
fn stab_target(entity: SharedEntity) -> SharedEntity {
    let parent = entity
        .downcast_ref::<EnderDragonPartEntity>()
        .and_then(|part| {
            entity
                .level()
                .and_then(|world| world.get_entity_by_id(part.parent_id()))
        });
    parent.unwrap_or(entity)
}

/// What one stab does: the damage it carries and whether it hurts, knocks
/// back or dismounts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Stab {
    /// The damage before enchantments.
    pub damage: f32,
    /// Whether the stab deals its damage.
    pub deals_damage: bool,
    /// Whether the stab knocks the target back.
    pub deals_knockback: bool,
    /// Whether the stab throws the target off its mount.
    pub dismounts: bool,
}

/// Vanilla `KineticWeapon.damageEntities` for `stack` used by `user`:
/// `stab_attack` is the user's `stabAttack`. Returns whether a stab landed,
/// after broadcasting the hit.
pub(crate) fn damage_entities(
    user: &dyn LivingEntity,
    kinetic_weapon: &KineticWeapon,
    stack: &ItemStack,
    ticks_remaining: i32,
    mut stab_attack: impl FnMut(&SharedEntity, Stab) -> bool,
) -> bool {
    let Some(world) = user.level() else {
        return false;
    };
    let use_duration = ITEM_BEHAVIORS
        .get_behavior(stack.item())
        .get_use_duration(stack, user);
    let ticks_used = use_duration - ticks_remaining;
    if ticks_used < kinetic_weapon.delay_ticks() {
        return false;
    }
    let ticks_used = ticks_used - kinetic_weapon.delay_ticks();
    let look = user.look_angle();
    let attacker_speed = look.dot(kinetic_motion(user));
    let action_factor = if user.as_player().is_some() { 1.0 } else { 0.2 };
    let base_damage = user
        .attributes()
        .lock()
        .get_base_value(vanilla_attributes::ATTACK_DAMAGE)
        .unwrap_or_default();
    let game_time = world.game_time();
    let mut affected = false;
    for target in hit_entities_along(user, stack, &world) {
        let target = stab_target(target);
        let living = user.living_base();
        if living.was_recently_stabbed(
            target.id(),
            kinetic_weapon.contact_cooldown_ticks(),
            game_time,
        ) {
            continue;
        }
        living.remember_stabbed_entity(target.id(), target.as_living_entity().is_some(), game_time);
        let target_speed = look.dot(kinetic_motion(target.as_ref()));
        let relative_speed = (attacker_speed - target_speed).max(0.0);
        let holds = |condition| {
            condition_holds(
                condition,
                ticks_used,
                attacker_speed,
                relative_speed,
                action_factor,
            )
        };
        let stab = Stab {
            damage: base_damage as f32
                + (relative_speed * f64::from(kinetic_weapon.damage_multiplier())).floor() as f32,
            deals_damage: holds(kinetic_weapon.damage_conditions()),
            deals_knockback: holds(kinetic_weapon.knockback_conditions()),
            dismounts: holds(kinetic_weapon.dismount_conditions()),
        };
        if stab.deals_damage || stab.deals_knockback || stab.dismounts {
            affected |= stab_attack(&target, stab);
        }
    }
    if affected {
        user.broadcast_entity_event(EntityStatus::KineticHit);
    }
    affected
}

/// Vanilla `LivingEntity.stabAttack` for a mob holding the weapon in `hand`.
pub(crate) fn mob_stab_attack(
    mob: &dyn Mob,
    entity: &SharedEntity,
    hand: InteractionHand,
    target: &SharedEntity,
    stab: Stab,
) -> bool {
    let Some(world) = mob.level() else {
        return false;
    };
    let Some(attacker) = mob.as_entity_event_source().as_living_entity() else {
        return false;
    };
    let slot = match hand {
        InteractionHand::MainHand => EquipmentSlot::MainHand,
        InteractionHand::OffHand => EquipmentSlot::OffHand,
    };
    let mut weapon = ItemStack::empty();
    mob.with_equipment_slot(slot, &mut |stack| weapon = stack.clone());
    let damage_source = mob.mob_attack_damage_source(&weapon, entity);
    let enchantment_context = EnchantmentDamageContext::new(
        target.entity_type(),
        Some(mob.entity_type()),
        Some(mob.entity_type()),
        &damage_source,
    );
    let damage = enchantment_helper::modify_damage(&weapon, &enchantment_context, stab.damage);
    let old_movement = target.velocity();
    let dealt_damage = stab.deals_damage && target.hurt(&world, &damage_source, damage);
    let mut affected = stab.deals_knockback | dealt_damage;
    if stab.deals_knockback {
        mob.cause_extra_knockback(
            target.as_ref(),
            0.4,
            old_movement,
            &KnockbackCause::new(&damage_source, damage, false),
        );
        mob.cause_extra_knockback(
            target.as_ref(),
            mob.get_attack_knockback(target.as_ref(), &weapon, &damage_source),
            old_movement,
            &KnockbackCause::new(&damage_source, damage, true),
        );
    }
    if stab.dismounts
        && target.is_passenger()
        && !REGISTRY.entity_types.is_in_tag(
            target.entity_type(),
            &EntityTypeTag::CANNOT_BE_DISMOUNTED_BY_ITEM_USAGE,
        )
    {
        affected = true;
        target.stop_riding();
    }
    if let Some(living_target) = target.as_living_entity() {
        mob.with_equipment_slot_mut(slot, &mut |stack| {
            if !stack.is_empty() {
                ITEM_BEHAVIORS.get_behavior(stack.item()).hurt_enemy(
                    stack,
                    living_target,
                    attacker,
                );
            }
        });
    }
    if dealt_damage {
        let context = EnchantmentPostAttackContext::new(target, &damage_source);
        enchantment_helper::do_post_attack_effects_with_item_source(
            &world,
            target.as_ref(),
            &weapon,
            &context,
        );
    }
    if !affected {
        return false;
    }
    mob.set_last_hurt_mob(Some(target));
    mob.play_attack_sound();
    true
}

#[cfg(test)]
mod tests {
    use steel_registry::data_components::components::KineticWeaponCondition;

    use super::condition_holds;

    #[test]
    fn conditions_need_the_speed_within_their_window() {
        let condition = KineticWeaponCondition::new(10, 4.6, 0.0).expect("a valid condition");
        assert!(condition_holds(Some(&condition), 10, 5.0, 0.0, 1.0));
        assert!(!condition_holds(Some(&condition), 11, 5.0, 0.0, 1.0));
        assert!(!condition_holds(Some(&condition), 0, 4.0, 0.0, 1.0));
        assert!(condition_holds(Some(&condition), 0, 1.0, 0.0, 0.2));
        assert!(!condition_holds(None, 0, 100.0, 100.0, 1.0));
    }
}
