//! Vanilla `Stopwatches`, the real-time stopwatches of `/stopwatch`.
//!
//! Vanilla keeps one set for the server. Steel keeps one for each domain, saved
//! through the domain's default world.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::io;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use steel_utils::Identifier;
use steel_utils::locks::{AsyncMutex, SyncMutex};
use steel_utils::saved_data::names as saved_data_names;

use crate::server::worlds::WorldMap;
use crate::world::World;

/// The instant vanilla's `Util.getMillis` counts from.
static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Vanilla `Stopwatches.currentTime`: monotonic milliseconds.
fn current_time() -> i64 {
    i64::try_from(EPOCH.elapsed().as_millis()).unwrap_or(i64::MAX)
}

/// Vanilla `Stopwatch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stopwatch {
    creation_time: i64,
    accumulated_elapsed_time: i64,
}

impl Stopwatch {
    const fn new(creation_time: i64) -> Self {
        Self {
            creation_time,
            accumulated_elapsed_time: 0,
        }
    }

    const fn elapsed_milliseconds(self, current_time: i64) -> i64 {
        self.accumulated_elapsed_time + (current_time - self.creation_time)
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "vanilla divides the long milliseconds as a double"
    )]
    fn elapsed_seconds(self, current_time: i64) -> f64 {
        self.elapsed_milliseconds(current_time) as f64 / 1000.0
    }
}

/// Vanilla `Stopwatches` for one domain.
#[derive(Default)]
pub(crate) struct Stopwatches {
    stopwatches: SyncMutex<BTreeMap<Identifier, Stopwatch>>,
    revision: AtomicU64,
    saved_revision: AtomicU64,
}

impl Stopwatches {
    /// Vanilla `add`: starts a stopwatch at zero, unless `id` is taken.
    pub(crate) fn add(&self, id: Identifier) -> bool {
        let added = match self.stopwatches.lock().entry(id) {
            Entry::Occupied(_) => false,
            Entry::Vacant(entry) => {
                entry.insert(Stopwatch::new(current_time()));
                true
            }
        };
        if added {
            self.mark_dirty();
        }
        added
    }

    /// Vanilla `get` and `Stopwatch.elapsedSeconds` at the current time.
    pub(crate) fn elapsed_seconds(&self, id: &Identifier) -> Option<f64> {
        let stopwatch = *self.stopwatches.lock().get(id)?;
        Some(stopwatch.elapsed_seconds(current_time()))
    }

    /// Vanilla `update` with a new stopwatch: starts `id` again from zero.
    pub(crate) fn restart(&self, id: &Identifier) -> bool {
        let restarted = self
            .stopwatches
            .lock()
            .get_mut(id)
            .map(|stopwatch| *stopwatch = Stopwatch::new(current_time()))
            .is_some();
        if restarted {
            self.mark_dirty();
        }
        restarted
    }

    /// Vanilla `remove`.
    pub(crate) fn remove(&self, id: &Identifier) -> bool {
        let removed = self.stopwatches.lock().remove(id).is_some();
        if removed {
            self.mark_dirty();
        }
        removed
    }

    /// Vanilla `ids`.
    pub(crate) fn ids(&self) -> Vec<Identifier> {
        self.stopwatches.lock().keys().cloned().collect()
    }

    fn mark_dirty(&self) {
        self.revision.fetch_add(1, Ordering::Release);
    }

    /// Vanilla `unpack`: each stopwatch keeps the time it had and runs on
    /// from now, so time the server was down does not count.
    fn from_persistent(persistent: PersistentStopwatches) -> io::Result<Self> {
        let now = current_time();
        let mut stopwatches = BTreeMap::new();
        for (id, accumulated_elapsed_time) in persistent.stopwatches {
            let id = id.parse::<Identifier>().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid stopwatch id '{id}': {error}"),
                )
            })?;
            stopwatches.insert(
                id,
                Stopwatch {
                    creation_time: now,
                    accumulated_elapsed_time,
                },
            );
        }
        Ok(Self {
            stopwatches: SyncMutex::new(stopwatches),
            ..Self::default()
        })
    }

    /// Vanilla `pack`. Like vanilla's `isDirty`, a set with any stopwatch is
    /// saved every time, since their elapsed times keep moving.
    fn pending_save(&self) -> Option<(u64, PersistentStopwatches)> {
        let stopwatches = self.stopwatches.lock();
        let revision = self.revision.load(Ordering::Acquire);
        if stopwatches.is_empty() && revision == self.saved_revision.load(Ordering::Acquire) {
            return None;
        }
        let now = current_time();
        let stopwatches = stopwatches
            .iter()
            .map(|(id, stopwatch)| (id.to_string(), stopwatch.elapsed_milliseconds(now)))
            .collect();
        Some((revision, PersistentStopwatches { stopwatches }))
    }

    fn mark_saved(&self, revision: u64) {
        self.saved_revision.fetch_max(revision, Ordering::Release);
    }
}

/// Vanilla `Stopwatches.CODEC`: elapsed milliseconds by id.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct PersistentStopwatches {
    stopwatches: BTreeMap<String, i64>,
}

/// Every domain's [`Stopwatches`].
pub(crate) struct DomainStopwatches {
    domains: BTreeMap<String, Stopwatches>,
    save_lock: AsyncMutex<()>,
}

impl DomainStopwatches {
    /// Loads each domain's stopwatches through its default world.
    pub(crate) async fn load(worlds: &WorldMap) -> io::Result<Self> {
        let mut domains = BTreeMap::new();
        for domain in worlds.domain_names() {
            let world = domain_default_world(worlds, domain)?;
            let persistent: PersistentStopwatches = world
                .saved_data
                .load_or_default(saved_data_names::STOPWATCHES)
                .await
                .map_err(|error| stopwatches_io_error(domain, &error))?;
            let stopwatches = Stopwatches::from_persistent(persistent)
                .map_err(|error| stopwatches_io_error(domain, &error))?;
            domains.insert(domain.to_owned(), stopwatches);
        }
        Ok(Self {
            domains,
            save_lock: AsyncMutex::new(()),
        })
    }

    /// The stopwatches of `domain`.
    pub(crate) fn get(&self, domain: &str) -> Option<&Stopwatches> {
        self.domains.get(domain)
    }

    /// Saves every domain with stopwatches or changes and returns the number
    /// written.
    pub(crate) async fn save(&self, worlds: &WorldMap) -> io::Result<usize> {
        let _save_guard = self.save_lock.lock().await;
        let mut saved = 0;
        for (domain, stopwatches) in &self.domains {
            let Some((revision, persistent)) = stopwatches.pending_save() else {
                continue;
            };
            let world = domain_default_world(worlds, domain)?;
            world
                .saved_data
                .save(saved_data_names::STOPWATCHES, &persistent)
                .await
                .map_err(|error| stopwatches_io_error(domain, &error))?;
            stopwatches.mark_saved(revision);
            saved += 1;
        }
        Ok(saved)
    }
}

fn domain_default_world<'a>(worlds: &'a WorldMap, domain: &str) -> io::Result<&'a World> {
    worlds
        .default_world(domain)
        .map(AsRef::as_ref)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("domain '{domain}' has no loaded default world"),
            )
        })
}

fn stopwatches_io_error(domain: &str, error: &io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("stopwatches I/O failed for domain '{domain}': {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(path: &str) -> Identifier {
        path.parse().expect("valid id")
    }

    #[test]
    fn stopwatches_are_added_once_restarted_and_removed() {
        let stopwatches = Stopwatches::default();
        assert!(stopwatches.add(id("demo:a")));
        assert!(!stopwatches.add(id("demo:a")));
        assert!(
            stopwatches
                .elapsed_seconds(&id("demo:a"))
                .is_some_and(|s| s >= 0.0)
        );
        assert_eq!(stopwatches.elapsed_seconds(&id("demo:b")), None);
        assert!(stopwatches.restart(&id("demo:a")));
        assert!(!stopwatches.restart(&id("demo:b")));
        assert_eq!(stopwatches.ids(), [id("demo:a")]);
        assert!(stopwatches.remove(&id("demo:a")));
        assert!(!stopwatches.remove(&id("demo:a")));
    }

    #[test]
    fn saving_keeps_elapsed_time_and_loading_runs_on_from_it() {
        let stopwatches = Stopwatches::default();
        assert!(stopwatches.pending_save().is_none());
        stopwatches.add(id("demo:a"));
        let (revision, _) = stopwatches.pending_save().expect("a stopwatch is saved");
        stopwatches.mark_saved(revision);
        // Running stopwatches are saved again even without a change.
        assert!(stopwatches.pending_save().is_some());

        let persistent = PersistentStopwatches {
            stopwatches: [("demo:a".to_owned(), 90_000)].into_iter().collect(),
        };
        let loaded = Stopwatches::from_persistent(persistent).expect("ids parse");
        let elapsed = loaded.elapsed_seconds(&id("demo:a")).expect("loaded");
        assert!((90.0..91.0).contains(&elapsed), "{elapsed}");
    }

    #[test]
    fn elapsed_seconds_divide_milliseconds() {
        let stopwatch = Stopwatch {
            creation_time: 1_000,
            accumulated_elapsed_time: 500,
        };
        assert_eq!(stopwatch.elapsed_milliseconds(3_250), 2_750);
        assert!((stopwatch.elapsed_seconds(3_250) - 2.75).abs() < f64::EPSILON);
    }
}
