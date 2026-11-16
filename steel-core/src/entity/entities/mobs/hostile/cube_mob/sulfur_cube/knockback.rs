//! Damage and knockback while the cube holds a block: kicking the cube.

use glam::{DVec3, Vec2};
use steel_math::trig::{cos, sin};
use steel_registry::vanilla_attributes;
use steel_registry::vanilla_damage_type_tags::DamageTypeTag;

use super::SulfurCubeEntity;
use crate::entity::damage::DamageSource;
use crate::entity::{Entity, KnockbackCause, LivingEntity};

/// Vanilla `HORIZONTAL_HIT_ANGLE_SCALE`.
const HORIZONTAL_HIT_ANGLE_SCALE: f32 = 1.6;
/// Vanilla `VERTICAL_HIT_ANGLE_SCALE`.
const VERTICAL_HIT_ANGLE_SCALE: f32 = 0.5;
/// Vanilla `VERTICAL_POSITION_ANGLE_SCALE`.
const VERTICAL_POSITION_ANGLE_SCALE: f32 = 0.8;
/// Vanilla `EXTRA_KNOCKBACK_DAMPENING`, applied to effect-driven knockback.
const EXTRA_KNOCKBACK_DAMPENING: f32 = 0.25;
/// Horizontal knockback scale after the power is computed.
const HORIZONTAL_KNOCKBACK_SCALE: f32 = 0.4;
/// Vertical knockback scale applied to the velocity.
const VERTICAL_KNOCKBACK_SCALE: f64 = 1.2;
/// Knockback power clamp.
const MAX_KNOCKBACK_POWER: f32 = 128.0;

/// Vanilla `Vec2.rotate`, with Minecraft's table sine and cosine.
fn rotate(vector: Vec2, angle_radians: f64) -> Vec2 {
    let cosine = cos(angle_radians);
    let sine = sin(angle_radians);
    Vec2::new(
        vector.x * cosine - vector.y * sine,
        vector.y * cosine + vector.x * sine,
    )
}

/// Vanilla `Mth.clampedMap` for doubles.
fn clamped_map(value: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    let factor = (value - from_min) / (from_max - from_min);
    if factor < 0.0 {
        to_min
    } else if factor > 1.0 {
        to_max
    } else {
        to_min + factor * (to_max - to_min)
    }
}

/// Vanilla `applyHorizontalHitAngleScale`: an off-center hit curves the push.
fn apply_horizontal_hit_angle_scale(
    original_angle: Vec2,
    attacker_position: DVec3,
    attacker_aim: DVec3,
    target_center: DVec3,
) -> Vec2 {
    let to_target = (target_center - attacker_position).normalize_or_zero();
    let angle_diff = (attacker_aim.x * to_target.z - attacker_aim.z * to_target.x)
        .atan2(attacker_aim.x * to_target.x + attacker_aim.z * to_target.z)
        as f32;
    rotate(
        original_angle,
        f64::from(angle_diff * HORIZONTAL_HIT_ANGLE_SCALE),
    )
}

/// Vanilla `applyVerticalHitAnglePowerTransfer`: hitting the cube low lifts it,
/// hitting it high flattens the push.
fn apply_vertical_hit_angle_power_transfer(
    power: Vec2,
    attacker_position: DVec3,
    attacker_aim: DVec3,
    target_center: DVec3,
    target_height: f32,
) -> Vec2 {
    let half_height = f64::from(0.5 * target_height);
    let to_top =
        (target_center + DVec3::new(0.0, half_height, 0.0) - attacker_position).normalize_or_zero();
    let to_bottom =
        (target_center - DVec3::new(0.0, half_height, 0.0) - attacker_position).normalize_or_zero();
    let factor = clamped_map(attacker_aim.y, to_top.y, to_bottom.y, -1.0, 1.0) as f32;
    let mut ratio = (factor * VERTICAL_HIT_ANGLE_SCALE).abs();
    if factor < 0.0 {
        ratio = -ratio;
    }
    Vec2::new(power.x * (1.0 - ratio), power.y * (1.0 + ratio))
}

/// Vanilla `applyVerticalPositionAnglePowerRotation`: an attacker above or
/// below the cube tilts the push, never past the archetype's powers.
fn apply_vertical_position_angle_power_rotation(
    power: Vec2,
    original_power: Vec2,
    attacker_feet: DVec3,
    target_feet: DVec3,
) -> Vec2 {
    let delta = target_feet - attacker_feet;
    let horizontal_distance = (delta.x * delta.x + delta.z * delta.z).sqrt();
    let angle = (-delta.y).atan2(horizontal_distance) as f32;
    let mut rotated = rotate(power, f64::from(-angle * VERTICAL_POSITION_ANGLE_SCALE));
    let horizontal_ratio = if original_power.x > 0.0 {
        rotated.x.abs() / original_power.x
    } else {
        0.0
    };
    let vertical_ratio = if original_power.y > 0.0 {
        rotated.y.abs() / original_power.y
    } else {
        0.0
    };
    let max_ratio = horizontal_ratio.max(vertical_ratio);
    if max_ratio > 1.0 {
        rotated *= 1.0 / max_ratio;
    }
    rotated
}

impl SulfurCubeEntity {
    /// Vanilla `hurtServer` while holding a block: fire or explosions light
    /// an explosive block, and the immune damage types only push the cube.
    /// Returns `true` when the hit was absorbed here.
    pub(super) fn hurt_with_body_item(&self, source: &DamageSource, damage: f32) -> bool {
        if !self.has_body_item() {
            return false;
        }
        if self.can_explode() && !self.is_primed() {
            let burning_arrow = source
                .direct_entity()
                .is_some_and(|entity| entity.is_abstract_arrow() && entity.is_on_fire());
            if source.is(&DamageTypeTag::IS_FIRE) || burning_arrow {
                self.prime_time(false);
            } else if source.is(&DamageTypeTag::IS_EXPLOSION) {
                self.prime_time(true);
            }
        }
        if source.is(&DamageTypeTag::SULFUR_CUBE_WITH_BLOCK_IMMUNE_TO) {
            self.apply_damage_knockback(source, damage, true);
            return true;
        }
        false
    }

    /// Vanilla `knockback` while holding a block and hit by an entity: the
    /// archetype's powers, steered by where and how the cube was hit.
    /// Returns `false` when vanilla falls back to the normal knockback.
    pub(super) fn knockback_with_body_item(
        &self,
        power: f64,
        xd: f64,
        zd: f64,
        cause: &KnockbackCause<'_>,
    ) -> bool {
        let Some(attacker) = cause.source.causing_entity() else {
            return false;
        };
        if !self.has_body_item() {
            return false;
        }
        let (modifiers, hit_sound) = {
            let state = self.state.lock();
            (
                state.knockback_modifiers,
                state.sound_settings.hit_sound.clone(),
            )
        };
        let original_power = Vec2::new(modifiers.horizontal_power, modifiers.vertical_power);
        let attacker_position = attacker.position();
        let eye_position = DVec3::new(
            attacker_position.x,
            attacker.get_eye_y(),
            attacker_position.z,
        );
        let aim = attacker.look_angle().normalize_or_zero();
        let center = self.bounding_box().center();

        let angle = apply_horizontal_hit_angle_scale(
            Vec2::new(xd as f32, zd as f32),
            eye_position,
            aim,
            center,
        );
        let transferred = apply_vertical_hit_angle_power_transfer(
            original_power,
            eye_position,
            aim,
            center,
            self.base.dimensions().height,
        );
        let mut new_power = apply_vertical_position_angle_power_rotation(
            transferred,
            original_power,
            attacker_position,
            self.position(),
        );

        let multiplier = cause.damage.sqrt()
            * if cause.comes_from_effect {
                power as f32 * EXTRA_KNOCKBACK_DAMPENING
            } else {
                1.0
            };
        new_power *= multiplier;
        let resistance = self
            .attributes()
            .lock()
            .get_value(vanilla_attributes::KNOCKBACK_RESISTANCE)
            .unwrap_or(0.0);
        new_power *= (1.0 - resistance) as f32;
        let horizontal = (new_power.x * HORIZONTAL_KNOCKBACK_SCALE)
            .clamp(-MAX_KNOCKBACK_POWER, MAX_KNOCKBACK_POWER);
        let vertical = new_power.y.clamp(-MAX_KNOCKBACK_POWER, MAX_KNOCKBACK_POWER);
        let push = DVec3::new(f64::from(angle.x), 0.0, f64::from(angle.y)).normalize_or_zero()
            * f64::from(horizontal);
        let velocity = self.velocity();
        self.set_velocity(DVec3::new(
            velocity.x - push.x,
            velocity.y + f64::from(vertical) * VERTICAL_KNOCKBACK_SCALE,
            velocity.z - push.z,
        ));
        self.mark_velocity_sync();
        self.play_archetype_sound(&hit_sound);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::FRAC_PI_2;

    use glam::{DVec3, Vec2};

    use super::{apply_vertical_position_angle_power_rotation, clamped_map, rotate};

    #[test]
    fn clamped_map_clamps_both_ends() {
        assert!((clamped_map(0.5, 0.0, 1.0, -1.0, 1.0)).abs() < 1e-9);
        assert!((clamped_map(-3.0, 0.0, 1.0, -1.0, 1.0) + 1.0).abs() < 1e-9);
        assert!((clamped_map(3.0, 0.0, 1.0, -1.0, 1.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn rotate_quarter_turn() {
        let rotated = rotate(Vec2::new(1.0, 0.0), FRAC_PI_2);
        assert!(rotated.x.abs() < 1e-3);
        assert!((rotated.y - 1.0).abs() < 1e-3);
    }

    #[test]
    fn level_attacker_keeps_archetype_power() {
        let power = Vec2::new(0.33, 0.06);
        let rotated = apply_vertical_position_angle_power_rotation(
            power,
            power,
            DVec3::new(-2.0, 0.0, 0.0),
            DVec3::ZERO,
        );
        assert!((rotated - power).length() < 1e-6);
    }
}
