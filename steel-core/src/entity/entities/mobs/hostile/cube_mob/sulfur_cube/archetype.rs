//! Applying the swallowed block's archetypes (vanilla `collectEquipmentChanges`).

use steel_math::trig::sin;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventHolder;
use steel_registry::sulfur_cube_archetype::{
    ArchetypeSound, DEFAULT_KNOCKBACK_MODIFIERS, SoundSettings, SulfurCubeArchetypeRef,
};
use steel_registry::{REGISTRY, RegistryExt as _};

use super::SulfurCubeEntity;
use crate::entity::attribute::AttributeModifier;
use crate::entity::{Entity, LivingEntity, Mob, PathfinderMob as _};

/// Vanilla `matchingArchetypes`: every archetype whose items include `stack`,
/// in registry order.
pub(super) fn matching_archetypes(stack: &ItemStack) -> Vec<SulfurCubeArchetypeRef> {
    if stack.is_empty() {
        return Vec::new();
    }
    REGISTRY
        .sulfur_cube_archetypes
        .iter()
        .map(|(_, archetype)| archetype)
        .filter(|archetype| archetype.matches_item(stack.item()))
        .collect()
}

/// Resolves an archetype sound: a registered sound by id, otherwise inline.
pub(super) fn sound_holder(sound: &ArchetypeSound) -> SoundEventHolder {
    if sound.fixed_range.is_none()
        && let Some(registered) = REGISTRY.sound_events.by_key(&sound.sound_id)
    {
        return SoundEventHolder::Registry(registered);
    }
    SoundEventHolder::Direct {
        sound_id: sound.sound_id.clone(),
        fixed_range: sound.fixed_range,
    }
}

impl SulfurCubeEntity {
    /// Vanilla `Entity.playSound(Holder)`: full volume, normal pitch.
    pub(super) fn play_archetype_sound(&self, sound: &ArchetypeSound) {
        if self.is_silent() {
            return;
        }
        if let Some(world) = self.level() {
            world.play_sound_holder_at(
                &sound_holder(sound),
                self.sound_source(),
                self.position(),
                1.0,
                1.0,
                None,
            );
        }
    }

    /// Vanilla `collectEquipmentChanges` for the body slot: swap goals and
    /// replace the old block's archetypes with the new block's.
    pub(super) fn apply_body_item_change(&self, previous: &ItemStack, current: &ItemStack) {
        if current.is_empty() {
            Self::register_goals(&self.mob_base);
        } else {
            self.remove_all_goals(|_| true);
            self.set_mob_speed(0.0);
        }

        for archetype in matching_archetypes(previous) {
            for modifier in &archetype.data.attribute_modifiers {
                if let Some(attribute) = REGISTRY.attributes.by_key(&modifier.attribute) {
                    self.attributes()
                        .lock()
                        .remove_modifier(attribute, &modifier.id);
                }
            }
        }

        let mut state = self.state.lock();
        state.floats_in_liquids = false;
        state.explosion = None;
        state.contact_damages.clear();
        state.knockback_modifiers = DEFAULT_KNOCKBACK_MODIFIERS;
        state.sound_settings = SoundSettings::vanilla_default();

        for archetype in matching_archetypes(current) {
            let data = &archetype.data;
            if data.buoyant {
                state.floats_in_liquids = true;
            }
            if data.explosion.is_some() {
                state.explosion = data.explosion;
            }
            if let Some(contact_damage) = &data.contact_damage {
                state.contact_damages.push(contact_damage.clone());
            }
            state.knockback_modifiers = data.knockback_modifiers;
            state.sound_settings = data.sound_settings.clone();
            for modifier in &data.attribute_modifiers {
                if let Some(attribute) = REGISTRY.attributes.by_key(&modifier.attribute) {
                    self.attributes().lock().set_modifier(
                        attribute,
                        AttributeModifier {
                            id: modifier.id.clone(),
                            amount: modifier.amount,
                            operation: modifier.operation,
                        },
                        false,
                    );
                }
            }
        }
    }

    /// Vanilla `travelInFluid` tail: a buoyant block bobs the cube up.
    pub(super) fn float_in_liquid(&self) {
        if !self.has_body_item() || !self.state.lock().floats_in_liquids {
            return;
        }
        let vibe_amount = 0.2 * sin(self.tick_count() as f32 * 0.4);
        let contact = self.fluid_contact();
        let fluid_height = if self.is_in_water() {
            contact.water_height()
        } else {
            contact.lava_height()
        };
        let immersion = fluid_height - self.get_fluid_jump_threshold() + f64::from(vibe_amount);
        if immersion > 0.0 {
            let velocity = self.velocity();
            self.set_velocity(
                velocity + glam::DVec3::new(0.0, immersion.min(1.0) * f64::from(0.04_f32), 0.0),
            );
        }
    }
}
