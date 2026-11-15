//! Experience orbs lying near each other become one orb worth all of them (the Fabric
//! `XpOrbs`).
//!
//! Vanilla only stacks orbs of the same value (the orb's count), and a player takes one orb
//! of a stack every 2 ticks, so the pile of a farm takes seconds to pick up. Here a stack and
//! its neighbors of any value fold into a single orb with a count of 1, which is taken in
//! one touch. The total experience never changes: the kept orb is worth the sum. An orb is
//! saved with a short value, so a sum above [`MAX_VALUE`] is not merged.

use std::sync::Arc;

use steel_core::{
    entity::{
        Entity as _, RemovalReason,
        entities::{ExperienceOrbEntity, objects::items::ExperienceOrbObserver},
    },
    world::World,
};
use steel_utils::Downcast as _;

/// An orb saves its value as a short.
const MAX_VALUE: i64 = i16::MAX as i64;
/// How far apart two orbs merge, in blocks (the Fabric `xp_orb_merge.radius`).
const RADIUS: f64 = 2.0;
/// Every how many ticks an orb looks for neighbors (the Fabric `xp_orb_merge.period_ticks`).
const PERIOD: i32 = 10;

/// All the experience `orb` gives: its value times its count.
fn total(orb: &ExperienceOrbEntity) -> i64 {
    i64::from(orb.value()) * i64::from(orb.count())
}

/// A vanilla stack of equal orbs becomes one orb of the same worth, if it fits.
fn flatten(orb: &ExperienceOrbEntity) {
    let worth = total(orb);
    if orb.count() > 1
        && worth <= MAX_VALUE
        && let Ok(value) = i32::try_from(worth)
    {
        orb.set_value(value);
        orb.set_count(1);
    }
}

/// `into` takes all the experience of `orb`, which is removed. Does nothing if the sum does
/// not fit one orb. Returns whether `orb` was taken.
fn absorb(into: &ExperienceOrbEntity, orb: &ExperienceOrbEntity) -> bool {
    let sum = total(into) + total(orb);
    if sum > MAX_VALUE {
        return false;
    }
    let Ok(value) = i32::try_from(sum) else {
        return false;
    };
    into.set_value(value);
    into.set_count(1);
    into.set_age(into.age().min(orb.age()));
    orb.set_removed(RemovalReason::Discarded);
    true
}

/// Folds the orbs around `orb` into it.
fn merge(orb: &ExperienceOrbEntity, world: &Arc<World>) {
    flatten(orb);
    let position = orb.position();
    let search_box = orb.bounding_box().inflate(RADIUS);
    for entity in world.get_entities_in_aabb(&search_box) {
        if entity.id() == orb.id() || entity.is_removed() {
            continue;
        }
        let Some(other) = entity.downcast_ref::<ExperienceOrbEntity>() else {
            continue;
        };
        if other.position().distance_squared(position) <= RADIUS * RADIUS {
            absorb(orb, other);
        }
    }
}

/// Merges experience orbs.
pub(super) struct XpOrbs;

/// The one observer of experience orbs.
pub(super) static XP_ORBS: XpOrbs = XpOrbs;

impl ExperienceOrbObserver for XpOrbs {
    fn tick(&self, orb: &ExperienceOrbEntity, world: &Arc<World>) {
        if orb.tick_count() % PERIOD == 0 {
            merge(orb, world);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::*;

    fn orb(id: i32, value: i32, count: i32, age: i32) -> ExperienceOrbEntity {
        let orb = ExperienceOrbEntity::new(
            &vanilla_entities::EXPERIENCE_ORB,
            id,
            DVec3::ZERO,
            Weak::new(),
        );
        orb.set_value(value);
        orb.set_count(count);
        orb.set_age(age);
        orb
    }

    #[test]
    fn a_stack_becomes_one_orb_of_the_same_worth() {
        init_vanilla_registry();
        let stack = orb(1, 7, 5, 0);
        flatten(&stack);
        assert_eq!((stack.value(), stack.count()), (35, 1));

        // Too much for one orb: the stack stays a stack.
        let big = orb(2, 20_000, 2, 0);
        flatten(&big);
        assert_eq!((big.value(), big.count()), (20_000, 2));
    }

    #[test]
    fn an_orb_takes_a_neighbor_of_any_value_and_the_younger_age() {
        init_vanilla_registry();
        let keeper = orb(1, 3, 1, 50);
        let other = orb(2, 11, 4, 12);
        assert!(absorb(&keeper, &other));
        assert_eq!((keeper.value(), keeper.count()), (47, 1));
        assert_eq!(keeper.age(), 12);
        assert!(other.is_removed());
    }

    #[test]
    fn a_sum_that_does_not_fit_an_orb_is_not_merged() {
        init_vanilla_registry();
        let keeper = orb(1, 30_000, 1, 0);
        let other = orb(2, 2768, 1, 0);
        assert!(!absorb(&keeper, &other));
        assert_eq!(keeper.value(), 30_000);
        assert!(!other.is_removed());
        // One less fits exactly.
        let other = orb(3, 2767, 1, 0);
        assert!(absorb(&keeper, &other));
        assert_eq!(keeper.value(), 32_767);
    }
}
