//! Fast leaf decay (the Fabric `LeafDecay`): once leaves find no log within 6 blocks, they
//! get a tick a fraction of a second later and fall then, instead of waiting for a random
//! tick minutes away. The random delay makes a tree fall apart gradually rather than vanish
//! at once.
//!
//! Vanilla's scheduled tick of leaves only recomputes the distance to the nearest log. When
//! that comes out as "no log", the leaves wait one short random delay and fall on the next
//! tick, if they still have no log.

use std::sync::Arc;

use rustc_hash::FxHashSet;
use steel_core::{
    behavior::blocks::vegetation::{LeafDecayObserver, LeafTick},
    world::World,
};
use steel_utils::{BlockPos, locks::SyncMutex};

const MIN_DELAY: i32 = 4;
const MAX_DELAY: i32 = 14;

/// A leaves block: the key of its world and its place.
type Key = (String, i32, i32, i32);

/// Leaves that already waited one delay and fall on their next tick.
static WAITING: SyncMutex<Option<FxHashSet<Key>>> = SyncMutex::new(None);

/// What the leaves at `key` do, given whether they are decaying; remembers who waits.
fn decide(waiting: &mut FxHashSet<Key>, key: Key, decaying: bool, delay: i32) -> LeafTick {
    if !decaying {
        // They got a log back, or were never decaying: forget any wait.
        waiting.remove(&key);
        return LeafTick::Vanilla;
    }
    if waiting.remove(&key) {
        return LeafTick::Fall;
    }
    waiting.insert(key);
    LeafTick::Wait(delay)
}

/// The observer that makes leaves fall soon.
pub struct LeafDecay;

/// The one instance, installed at start.
pub static LEAF_DECAY: LeafDecay = LeafDecay;

impl LeafDecayObserver for LeafDecay {
    fn tick(&self, world: &Arc<World>, pos: BlockPos, decaying: bool) -> LeafTick {
        let key = (world.key.to_string(), pos.x(), pos.y(), pos.z());
        let delay = rand::random_range(MIN_DELAY..=MAX_DELAY);
        decide(
            WAITING.lock().get_or_insert_with(FxHashSet::default),
            key,
            decaying,
            delay,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(x: i32) -> Key {
        ("minecraft:overworld".to_owned(), x, 70, 0)
    }

    #[test]
    fn leaves_without_a_log_wait_once_and_then_fall() {
        let mut waiting = FxHashSet::default();
        assert_eq!(decide(&mut waiting, key(0), true, 7), LeafTick::Wait(7));
        assert_eq!(decide(&mut waiting, key(0), true, 9), LeafTick::Fall);
        assert!(waiting.is_empty());
    }

    #[test]
    fn a_log_put_back_cancels_the_wait() {
        let mut waiting = FxHashSet::default();
        assert_eq!(decide(&mut waiting, key(0), true, 7), LeafTick::Wait(7));
        assert_eq!(decide(&mut waiting, key(0), false, 7), LeafTick::Vanilla);
        assert_eq!(decide(&mut waiting, key(0), true, 5), LeafTick::Wait(5));
    }

    #[test]
    fn each_block_waits_on_its_own() {
        let mut waiting = FxHashSet::default();
        assert_eq!(decide(&mut waiting, key(0), true, 4), LeafTick::Wait(4));
        assert_eq!(decide(&mut waiting, key(1), true, 4), LeafTick::Wait(4));
        assert_eq!(decide(&mut waiting, key(1), true, 4), LeafTick::Fall);
        assert_eq!(decide(&mut waiting, key(0), true, 4), LeafTick::Fall);
    }
}
