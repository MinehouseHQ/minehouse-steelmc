//! Vanilla `WardenSpawnTracker`: how close a player is to a warden answering
//! the sculk shriekers they set off.

use std::sync::Arc;

use glam::DVec3;
use simdnbt::borrow::NbtCompound as NbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_registry::vanilla_entities;
use steel_utils::{BlockPos, WorldAabb};

use super::Player;
use crate::entity::Entity as _;
use crate::world::World;

const TICKS_SINCE_LAST_WARNING_TAG: &str = "ticks_since_last_warning";
const WARNING_LEVEL_TAG: &str = "warning_level";
const COOLDOWN_TICKS_TAG: &str = "cooldown_ticks";

/// Vanilla `MAX_WARNING_LEVEL`.
pub const MAX_WARNING_LEVEL: i32 = 4;
/// Vanilla `PLAYER_SEARCH_RADIUS`.
const PLAYER_SEARCH_RADIUS: f64 = 16.0;
/// Vanilla `WARNING_CHECK_DIAMETER`.
const WARNING_CHECK_DIAMETER: f64 = 48.0;
/// Vanilla `DECREASE_WARNING_LEVEL_EVERY_INTERVAL`.
const DECREASE_WARNING_LEVEL_EVERY_INTERVAL: i32 = 12_000;
/// Vanilla `WARNING_LEVEL_INCREASE_COOLDOWN`.
const WARNING_LEVEL_INCREASE_COOLDOWN: i32 = 200;

/// Vanilla `WardenSpawnTracker`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WardenSpawnTracker {
    ticks_since_last_warning: i32,
    warning_level: i32,
    cooldown_ticks: i32,
}

impl WardenSpawnTracker {
    /// Creates a tracker from its saved fields.
    #[must_use]
    pub const fn new(
        ticks_since_last_warning: i32,
        warning_level: i32,
        cooldown_ticks: i32,
    ) -> Self {
        Self {
            ticks_since_last_warning,
            warning_level,
            cooldown_ticks,
        }
    }

    /// Vanilla `tick`: the warning level drops by one every ten minutes
    /// without a warning.
    pub const fn tick(&mut self) {
        if self.ticks_since_last_warning >= DECREASE_WARNING_LEVEL_EVERY_INTERVAL {
            self.set_warning_level(self.warning_level - 1);
            self.ticks_since_last_warning = 0;
        } else {
            self.ticks_since_last_warning += 1;
        }
        if self.cooldown_ticks > 0 {
            self.cooldown_ticks -= 1;
        }
    }

    /// Vanilla `reset`.
    pub const fn reset(&mut self) {
        *self = Self::new(0, 0, 0);
    }

    /// Vanilla `getWarningLevel`.
    #[must_use]
    pub const fn warning_level(&self) -> i32 {
        self.warning_level
    }

    /// Vanilla `setWarningLevel`: clamped to 0 to 4.
    pub const fn set_warning_level(&mut self, warning_level: i32) {
        self.warning_level = if warning_level < 0 {
            0
        } else if warning_level > MAX_WARNING_LEVEL {
            MAX_WARNING_LEVEL
        } else {
            warning_level
        };
    }

    /// The ticks since the last warning.
    #[must_use]
    pub const fn ticks_since_last_warning(&self) -> i32 {
        self.ticks_since_last_warning
    }

    /// The ticks until the warning level can rise again.
    #[must_use]
    pub const fn cooldown_ticks(&self) -> i32 {
        self.cooldown_ticks
    }

    const fn on_cooldown(&self) -> bool {
        self.cooldown_ticks > 0
    }

    /// Vanilla `increaseWarningLevel`.
    const fn increase_warning_level(&mut self) {
        if !self.on_cooldown() {
            self.ticks_since_last_warning = 0;
            self.cooldown_ticks = WARNING_LEVEL_INCREASE_COOLDOWN;
            self.set_warning_level(self.warning_level + 1);
        }
    }

    /// Vanilla `tryWarn`: raises the warning level shared by every player
    /// near `pos` and `trigger`, unless a warden is already close or any of
    /// them warned recently. Returns the new warning level.
    #[must_use]
    pub fn try_warn(world: &World, pos: BlockPos, trigger: &Arc<Player>) -> Option<i32> {
        if has_nearby_warden(world, pos) {
            return None;
        }
        let mut players = nearby_players(world, pos);
        if !players.iter().any(|player| Arc::ptr_eq(player, trigger)) {
            players.push(Arc::clone(trigger));
        }
        if players
            .iter()
            .any(|player| player.warden_spawn_tracker().on_cooldown())
        {
            return None;
        }
        // Vanilla `Stream.max` keeps the last of equal warning levels.
        let mut highest = players
            .iter()
            .map(|player| player.warden_spawn_tracker())
            .reduce(|best, tracker| {
                if tracker.warning_level >= best.warning_level {
                    tracker
                } else {
                    best
                }
            })?;
        highest.increase_warning_level();
        for player in &players {
            player.set_warden_spawn_tracker(highest);
        }
        Some(highest.warning_level)
    }

    /// Vanilla `CODEC` decode: each field defaults to 0 and must not be
    /// negative.
    #[must_use]
    pub fn load(nbt: &NbtCompoundView<'_, '_>) -> Option<Self> {
        Self::decode(|key| nbt.int(key))
    }

    /// [`Self::load`] from an owned compound.
    #[must_use]
    pub fn load_owned(nbt: &NbtCompound) -> Option<Self> {
        Self::decode(|key| nbt.int(key))
    }

    fn decode(int: impl Fn(&str) -> Option<i32>) -> Option<Self> {
        let field = |key| match int(key) {
            None => Some(0),
            Some(value) if value >= 0 => Some(value),
            Some(_) => None,
        };
        Some(Self::new(
            field(TICKS_SINCE_LAST_WARNING_TAG)?,
            field(WARNING_LEVEL_TAG)?,
            field(COOLDOWN_TICKS_TAG)?,
        ))
    }

    /// Vanilla `CODEC` encode: every field is written.
    #[must_use]
    pub fn save(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        nbt.insert(TICKS_SINCE_LAST_WARNING_TAG, self.ticks_since_last_warning);
        nbt.insert(WARNING_LEVEL_TAG, self.warning_level);
        nbt.insert(COOLDOWN_TICKS_TAG, self.cooldown_ticks);
        nbt
    }
}

/// Vanilla `hasNearbyWarden`: a warden within a 48-block cube.
fn has_nearby_warden(world: &World, pos: BlockPos) -> bool {
    let half = WARNING_CHECK_DIAMETER / 2.0;
    let center = block_center(pos);
    let area = WorldAabb::new(
        center.x - half,
        center.y - half,
        center.z - half,
        center.x + half,
        center.y + half,
        center.z + half,
    );
    !world
        .get_entities_in_aabb_matching(&area, |entity| {
            entity.entity_type().key == vanilla_entities::WARDEN.key
        })
        .is_empty()
}

/// Vanilla `getNearbyPlayers`: living non-spectators within 16 blocks.
fn nearby_players(world: &World, pos: BlockPos) -> Vec<Arc<Player>> {
    let origin = block_center(pos);
    let mut players = Vec::new();
    world.players.iter_players(|_, player| {
        if !player.is_spectator()
            && player.position().distance_squared(origin)
                < PLAYER_SEARCH_RADIUS * PLAYER_SEARCH_RADIUS
            && player.is_alive()
        {
            players.push(Arc::clone(player));
        }
        true
    });
    players
}

fn block_center(pos: BlockPos) -> DVec3 {
    let (x, y, z) = pos.get_center();
    DVec3::new(x, y, z)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use simdnbt::borrow::read_compound;

    use super::*;

    #[test]
    fn the_warning_level_drops_every_ten_minutes_and_the_cooldown_runs_out() {
        let mut tracker = WardenSpawnTracker::new(0, 2, 3);
        for _ in 0..DECREASE_WARNING_LEVEL_EVERY_INTERVAL {
            tracker.tick();
        }
        assert_eq!(tracker, WardenSpawnTracker::new(12_000, 2, 0));
        tracker.tick();
        assert_eq!(tracker, WardenSpawnTracker::new(0, 1, 0));
    }

    #[test]
    fn warnings_rise_once_per_cooldown_and_stop_at_four() {
        let mut tracker = WardenSpawnTracker::new(50, 3, 0);
        tracker.increase_warning_level();
        assert_eq!(tracker, WardenSpawnTracker::new(0, 4, 200));
        tracker.increase_warning_level();
        assert_eq!(tracker.warning_level(), 4);
        tracker.cooldown_ticks = 0;
        tracker.increase_warning_level();
        assert_eq!(tracker.warning_level(), MAX_WARNING_LEVEL);
    }

    fn read(nbt: &NbtCompound) -> Option<WardenSpawnTracker> {
        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let borrowed = read_compound(&mut Cursor::new(bytes.as_slice())).expect("valid nbt");
        let view: NbtCompoundView<'_, '_> = (&borrowed).into();
        WardenSpawnTracker::load(&view)
    }

    #[test]
    fn the_codec_round_trips_and_rejects_negative_fields() {
        let tracker = WardenSpawnTracker::new(7, 2, 100);
        assert_eq!(read(&tracker.save()), Some(tracker));
        assert_eq!(
            WardenSpawnTracker::load_owned(&tracker.save()),
            Some(tracker)
        );
        assert_eq!(
            read(&NbtCompound::new()),
            Some(WardenSpawnTracker::default())
        );

        let mut negative = NbtCompound::new();
        negative.insert(WARNING_LEVEL_TAG, -1);
        assert_eq!(read(&negative), None);
    }
}
