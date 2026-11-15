//! Delta-tracking state for `CSetHealth` packet deduplication.
//!
//! Tracks the last health/food/saturation values sent to the client so we only
//! send `CSetHealth` when something actually changes.

use crate::entity::{Entity as _, LivingEntity as _};
use crate::player::Player;
use crate::scoreboard::{CriteriaUpdate, CustomCriteria, ObjectiveCriteria};

/// Vanilla `Mth.ceil`, saturating like Java's float-to-int cast.
#[expect(
    clippy::cast_possible_truncation,
    reason = "Java's float-to-int cast saturates like Rust's `as`"
)]
const fn java_ceil(value: f32) -> i32 {
    value.ceil() as i32
}

/// Tracks the last health/food/saturation values sent to the client.
pub struct HealthSyncState {
    /// Last health value sent to the client.
    pub last_health: f32,
    /// Last food level sent to the client.
    pub last_food: i32,
    /// Whether saturation was zero last time we sent health.
    pub saturation_zero: bool,
}

impl HealthSyncState {
    /// Creates a new state that will trigger a send on the first tick.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            last_health: -1.0,
            last_food: -1,
            saturation_zero: true,
        }
    }

    /// Returns true if the given values differ from what was last sent.
    #[expect(
        clippy::float_cmp,
        reason = "intentional exact comparison: we only want to send updates when the value changes from what we last sent"
    )]
    #[must_use]
    pub fn needs_update(&self, health: f32, food: i32, saturation_zero: bool) -> bool {
        self.last_health != health
            || self.last_food != food
            || self.saturation_zero != saturation_zero
    }

    /// Records that we just sent the given values to the client.
    pub const fn record_sent(&mut self, health: f32, food: i32, saturation_zero: bool) {
        self.last_health = health;
        self.last_food = food;
        self.saturation_zero = saturation_zero;
    }

    /// Invalidates the state so the next tick will re-send.
    pub const fn invalidate(&mut self) {
        self.last_health = -1.0e8;
    }
}

/// Vanilla `ServerPlayer`'s `lastRecorded*` fields: the values last written
/// to the objectives of the `health`, `food`, `air`, `armor`, `xp` and
/// `level` criteria. A respawned player is a new [`Player`], so like vanilla
/// it starts over and writes them again.
pub struct RecordedCriteria {
    health_and_absorption: f32,
    food: i32,
    air: i32,
    armor: i32,
    experience: i32,
    level: i32,
}

impl RecordedCriteria {
    /// Vanilla's initial values, which no real value matches.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            // Java `Float.MIN_VALUE`, the smallest positive float.
            health_and_absorption: f32::from_bits(1),
            food: i32::MIN,
            air: i32::MIN,
            armor: i32::MIN,
            experience: i32::MIN,
            level: i32::MIN,
        }
    }
}

impl Default for RecordedCriteria {
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    /// The part of vanilla `ServerPlayer.doTick` that writes the read-only
    /// criteria's scores when their values change.
    #[expect(
        clippy::float_cmp,
        reason = "vanilla compares the recorded health exactly"
    )]
    pub(crate) fn update_recorded_criteria(&self) {
        let health_and_absorption = self.get_health() + self.get_absorption_amount();
        let food = self.food_data.lock().food_level;
        let air = self.air_supply();
        let armor = self.get_armor_value();
        let (experience, level) = {
            let experience = self.experience.lock();
            (experience.total_points(), experience.level())
        };
        let mut updates = Vec::new();
        {
            let mut guard = self.recorded_criteria.lock();
            let recorded = &mut *guard;
            if health_and_absorption != recorded.health_and_absorption {
                recorded.health_and_absorption = health_and_absorption;
                updates.push((CustomCriteria::Health, java_ceil(health_and_absorption)));
            }
            for (criteria, value, last) in [
                (CustomCriteria::Food, food, &mut recorded.food),
                (CustomCriteria::Air, air, &mut recorded.air),
                (CustomCriteria::Armor, armor, &mut recorded.armor),
                (
                    CustomCriteria::Experience,
                    experience,
                    &mut recorded.experience,
                ),
                (CustomCriteria::Level, level, &mut recorded.level),
            ] {
                if value != *last {
                    *last = value;
                    updates.push((criteria, value));
                }
            }
        }
        for (criteria, value) in updates {
            self.update_scores_for_criteria(
                &ObjectiveCriteria::Custom(criteria),
                CriteriaUpdate::Set(value),
            );
        }
    }

    /// Invalidates the delta-tracking state so that the next `tick()` will send
    /// `CSetHealth` to the client (vanilla: `resetSentInfo`).
    pub fn reset_sent_info(&self) {
        self.health_sync.lock().invalidate();
    }
}
