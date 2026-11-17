//! Vanilla `ProjectileUtil.getHitEntitiesAlong` and
//! `PiercingWeapon.canHitEntity`: what a piercing or kinetic weapon reaches
//! along the attacker's head look.

use std::mem::swap;

use glam::DVec3;
use steel_registry::item_stack::ItemStack;
use steel_registry::{vanilla_attributes, vanilla_entities};
use steel_utils::WorldAabb;
use steel_utils::types::GameType;

use crate::entity::{Entity, LivingEntity, SharedEntity};
use crate::world::{ClipBlockShape, ClipFluid, World};

/// Vanilla `PiercingWeapon.canHitEntity(jabber, target)`.
pub(crate) fn can_piercing_hit_entity(jabber: &dyn Entity, target: &dyn Entity) -> bool {
    if target.id() == jabber.id()
        || target.is_invulnerable_to_piercing_weapon()
        || !target.is_alive()
    {
        return false;
    }
    if target.entity_type() == &vanilla_entities::INTERACTION {
        return true;
    }
    if !target.can_be_hit_by_projectile() {
        return false;
    }
    if let (Some(target_player), Some(jabbing_player)) = (target.as_player(), jabber.as_player())
        && !jabbing_player.can_harm_player(target_player)
    {
        return false;
    }
    !jabber.is_passenger_of_same_vehicle(target)
}

/// Vanilla `LivingEntity.getAttackRangeWith(stack)` resolved through
/// `AttackRange.effectiveMinRange`/`effectiveMaxRange`: the reach a `stack`
/// gives `attacker` and its hitbox margin.
pub(crate) fn effective_attack_range(
    attacker: &dyn LivingEntity,
    stack: &ItemStack,
) -> (f64, f64, f64) {
    let Some(range) = stack.get_attack_range() else {
        // Vanilla `AttackRange.defaultFor`: the entity interaction range.
        let reach = attacker
            .attributes()
            .lock()
            .get_value(vanilla_attributes::ENTITY_INTERACTION_RANGE)
            .unwrap_or_default();
        return (0.0, reach, 0.0);
    };
    let (min_reach, max_reach) = match attacker.as_player() {
        Some(player) if player.game_mode() == GameType::Creative => {
            (range.min_creative_reach, range.max_creative_reach)
        }
        Some(_) => (range.min_reach, range.max_reach),
        None => (
            range.min_reach * range.mob_factor,
            range.max_reach * range.mob_factor,
        ),
    };
    (
        f64::from(min_reach),
        f64::from(max_reach),
        f64::from(range.hitbox_margin),
    )
}

/// Vanilla `ProjectileUtil.getHitEntitiesAlong(attacker, attackRange,
/// PiercingWeapon::canHitEntity, COLLIDER)`: the entities `stack` reaches along
/// the attacker's head look, nearest first; none when a block is closer.
pub(crate) fn hit_entities_along(
    attacker: &dyn LivingEntity,
    stack: &ItemStack,
    world: &World,
) -> Vec<SharedEntity> {
    let (yaw, pitch) = (attacker.y_head_rot(), attacker.rotation().1);
    let look = attacker.calculate_view_vector(pitch, yaw);
    if look.length_squared() <= f64::EPSILON {
        return Vec::new();
    }
    let (min_reach, max_reach, hitbox_margin) = effective_attack_range(attacker, stack);
    let position = attacker.position();
    let eye_position = DVec3::new(position.x, attacker.get_eye_y(), position.z);
    let from = eye_position + look * min_reach;
    let movement_extension = attacker.known_movement().dot(look).max(0.0);
    let mut to = eye_position + look * (max_reach + movement_extension);

    let block_hit = world.clip(eye_position, to, ClipBlockShape::Collider, ClipFluid::None);
    if !block_hit.is_miss() {
        to = block_hit.location;
        if eye_position.distance_squared(to) < eye_position.distance_squared(from) {
            return Vec::new();
        }
    }

    let search_area = WorldAabb::new(from.x, from.y, from.z, from.x, from.y, from.z)
        .inflate_xyz(hitbox_margin, hitbox_margin, hitbox_margin)
        .expand_towards(to - from)
        .inflate(1.0);
    let mut hits = world
        .get_entities_in_aabb_matching(&search_area, |entity| {
            can_piercing_hit_entity(attacker, entity)
        })
        .into_iter()
        .filter_map(|entity| {
            piercing_ray_hit_t(world, entity.bounding_box(), from, to, hitbox_margin)
                .map(|hit_t| (hit_t, entity))
        })
        .collect::<Vec<_>>();
    hits.sort_by(|(left, _), (right, _)| left.total_cmp(right));
    hits.into_iter().map(|(_, entity)| entity).collect()
}

/// Vanilla `ProjectileUtil.getManyEntityHitResult`'s per-entity test: where
/// along `from`..`to` the ray meets `bounding_box` widened by `entity_margin`.
pub(crate) fn piercing_ray_hit_t(
    world: &World,
    bounding_box: WorldAabb,
    from: DVec3,
    to: DVec3,
    entity_margin: f64,
) -> Option<f64> {
    if let Some(hit_t) = ray_aabb_hit_t(bounding_box, from, to) {
        return Some(hit_t);
    }
    if entity_margin <= 0.0 {
        return None;
    }

    let outside_hit_t = ray_aabb_hit_t(bounding_box.inflate(entity_margin), from, to)?;
    let outside_hit = from + (to - from) * outside_hit_t;
    let mut towards_target = DVec3::new(
        f64::midpoint(bounding_box.min_x(), bounding_box.max_x()),
        f64::midpoint(bounding_box.min_y(), bounding_box.max_y()),
        f64::midpoint(bounding_box.min_z(), bounding_box.max_z()),
    );
    let block_hit = world.clip(
        outside_hit,
        towards_target,
        ClipBlockShape::Collider,
        ClipFluid::None,
    );
    if !block_hit.is_miss() {
        towards_target = block_hit.location;
    }
    ray_aabb_hit_t(bounding_box, outside_hit, towards_target).map(|_| outside_hit_t)
}

fn ray_aabb_hit_t(aabb: WorldAabb, from: DVec3, to: DVec3) -> Option<f64> {
    if aabb.contains(from) {
        return Some(0.0);
    }

    let delta = to - from;
    let mut t_min = 0.0_f64;
    let mut t_max = 1.0_f64;
    if !update_ray_axis(
        from.x,
        delta.x,
        aabb.min_x(),
        aabb.max_x(),
        &mut t_min,
        &mut t_max,
    ) {
        return None;
    }
    if !update_ray_axis(
        from.y,
        delta.y,
        aabb.min_y(),
        aabb.max_y(),
        &mut t_min,
        &mut t_max,
    ) {
        return None;
    }
    if !update_ray_axis(
        from.z,
        delta.z,
        aabb.min_z(),
        aabb.max_z(),
        &mut t_min,
        &mut t_max,
    ) {
        return None;
    }

    Some(t_min)
}

fn update_ray_axis(
    start: f64,
    delta: f64,
    min: f64,
    max: f64,
    t_min: &mut f64,
    t_max: &mut f64,
) -> bool {
    if delta.abs() < f64::EPSILON {
        return start >= min && start <= max;
    }

    let inverse_delta = 1.0 / delta;
    let mut enter = (min - start) * inverse_delta;
    let mut exit = (max - start) * inverse_delta;
    if enter > exit {
        swap(&mut enter, &mut exit);
    }

    *t_min = (*t_min).max(enter);
    *t_max = (*t_max).min(exit);
    *t_min <= *t_max
}
