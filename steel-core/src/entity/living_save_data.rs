//! Vanilla `LivingEntity.readAdditionalSaveData` for living state stored with
//! chunk entities and `/summon` data.

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtTag;
use steel_registry::MobEffectInstance as RegistryMobEffectInstance;
use steel_registry::entity_data::EntityPose;
use steel_utils::{BlockPos, UuidExt as _};
use uuid::Uuid;

use super::{ActiveMobEffect, LivingEntity};

/// Reads the common living state saved by
/// [`LivingEntity::save_command_nbt`], in vanilla order: absorption,
/// attributes, effects, health, hurt and death animation, elytra flight,
/// sleeping position, the player-kill memory and the impulse context.
///
/// Equipment and the locator bar icon are read by the mob and armor stand
/// loaders themselves.
pub(crate) fn load_living_save_data(
    living: &dyn LivingEntity,
    nbt: BorrowedNbtCompoundView<'_, '_>,
) {
    if let Some(attributes) = nbt.list("attributes") {
        living.attributes().lock().apply_vanilla_nbt(attributes);
    }

    living.living_base().clear_mob_effects_no_update();
    for effect in nbt
        .list("active_effects")
        .and_then(|effects| effects.compounds())
        .into_iter()
        .flatten()
        .filter_map(|effect| {
            RegistryMobEffectInstance::from_owned_nbt(&NbtTag::Compound(effect.to_owned()))
        })
    {
        living
            .living_base()
            .add_mob_effect(ActiveMobEffect::from_registry_instance(&effect));
    }
    // Vanilla reads the absorption first; Steel clamps it to the
    // `max_absorption` attribute, so it is set once the attributes and
    // effects that raise that maximum are back.
    living.set_absorption_amount(nbt.float("AbsorptionAmount").unwrap_or(0.0));

    living.set_health(
        nbt.float("Health")
            .unwrap_or_else(|| living.get_max_health()),
    );
    living
        .living_base()
        .set_hurt_time(i32::from(nbt.short("HurtTime").unwrap_or(0)));
    living
        .living_base()
        .set_death_time(i32::from(nbt.short("DeathTime").unwrap_or(0)));
    if nbt.byte("FallFlying").is_some_and(|flying| flying != 0) {
        living.set_fall_flying(true);
    }

    match nbt.int_array("sleeping_pos").as_deref() {
        Some(&[x, y, z]) => {
            living.set_sleeping_pos(BlockPos::new(x, y, z));
            living.set_pose(EntityPose::Sleeping);
        }
        _ => living.clear_sleeping_pos(),
    }

    if let Some(uuid) = nbt
        .int_array("last_hurt_by_player")
        .and_then(|ints| Uuid::from_int_array(&ints))
    {
        living.set_last_hurt_by_player(
            uuid,
            nbt.int("last_hurt_by_player_memory_time").unwrap_or(0),
        );
    }

    let impact = nbt
        .list("current_explosion_impact_pos")
        .and_then(|pos| pos.doubles())
        .and_then(|pos| match pos.as_slice() {
            [x, y, z] => Some(DVec3::new(*x, *y, *z)),
            _ => None,
        });
    living.living_base().restore_impulse_context(
        nbt.int("current_impulse_context_reset_grace_time")
            .unwrap_or(0),
        impact,
    );
}
