//! Vanilla `TimerQueue`, the events `/schedule` sets to run functions at a
//! later game time.
//!
//! Vanilla keeps one queue for the server, saved with the overworld. Steel keeps
//! one for each domain, saved through the domain's default world and fired by
//! that domain's game time.

use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use steel_utils::Identifier;
use steel_utils::locks::{AsyncMutex, SyncMutex};
use steel_utils::saved_data::names as saved_data_names;

use crate::server::worlds::WorldMap;
use crate::world::World;

/// Vanilla `TimerCallback`: what a due event runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TimerCallback {
    /// Vanilla `FunctionCallback`: runs the function, if it still exists.
    Function(Identifier),
    /// Vanilla `FunctionTagCallback`: runs every function in the tag.
    FunctionTag(Identifier),
}

/// Vanilla `TimerQueue.Event`, without its trigger time and sequential id,
/// which are its key in the queue.
struct Event {
    id: String,
    callback: TimerCallback,
}

#[derive(Default)]
struct State {
    /// Vanilla's `PriorityQueue`, ordered by trigger time, then sequential id.
    queue: BTreeMap<(i64, u64), Event>,
    /// Vanilla's `Table<String, Long, Event>`: the sequential id of each event
    /// by its id and trigger time.
    events: BTreeMap<String, BTreeMap<i64, u64>>,
    sequential_id: u64,
}

impl State {
    fn schedule(&mut self, id: String, time: i64, callback: TimerCallback) -> bool {
        let row = self.events.entry(id.clone()).or_default();
        if row.contains_key(&time) {
            return false;
        }
        self.sequential_id += 1;
        row.insert(time, self.sequential_id);
        self.queue
            .insert((time, self.sequential_id), Event { id, callback });
        true
    }
}

/// Vanilla `TimerQueue<MinecraftServer>` for one domain.
pub(crate) struct TimerQueue {
    state: SyncMutex<State>,
    revision: AtomicU64,
    saved_revision: AtomicU64,
}

impl Default for TimerQueue {
    fn default() -> Self {
        Self {
            state: SyncMutex::new(State::default()),
            revision: AtomicU64::new(0),
            saved_revision: AtomicU64::new(0),
        }
    }
}

impl TimerQueue {
    /// Vanilla `schedule`: an event already set for the same id and time is
    /// kept, and the new one dropped.
    pub(crate) fn schedule(&self, id: String, time: i64, callback: TimerCallback) {
        if self.state.lock().schedule(id, time, callback) {
            self.mark_dirty();
        }
    }

    /// Vanilla `remove`: drops every event with `id` and returns how many it
    /// had in its table, which can count events that already ran (see
    /// [`Self::pop_due`]).
    pub(crate) fn remove(&self, id: &str) -> usize {
        let removed = {
            let mut state = self.state.lock();
            let row = state.events.remove(id).unwrap_or_default();
            for (time, sequential_id) in &row {
                state.queue.remove(&(*time, *sequential_id));
            }
            row.len()
        };
        self.mark_dirty();
        removed
    }

    /// Vanilla `getEventsIds`.
    pub(crate) fn event_ids(&self) -> Vec<String> {
        self.state.lock().events.keys().cloned().collect()
    }

    /// One step of vanilla `tick`: takes the earliest event due at
    /// `current_tick`, if any.
    ///
    /// Like vanilla, the event leaves the id table only at `current_tick`, so
    /// one that was due earlier stays there: it still counts for `remove` and
    /// is still suggested by `/schedule clear`, though it never runs again.
    pub(crate) fn pop_due(&self, current_tick: i64) -> Option<TimerCallback> {
        let event = {
            let mut state = self.state.lock();
            let entry = state.queue.first_entry()?;
            if entry.key().0 > current_tick {
                return None;
            }
            let event = entry.remove();
            if let Some(row) = state.events.get_mut(&event.id) {
                row.remove(&current_tick);
                if row.is_empty() {
                    state.events.remove(&event.id);
                }
            }
            event
        };
        self.mark_dirty();
        Some(event.callback)
    }

    fn mark_dirty(&self) {
        self.revision.fetch_add(1, Ordering::Release);
    }

    /// Vanilla's codec reads each event back through `schedule`, which starts
    /// the sequential ids over.
    fn from_persistent(persistent: PersistentTimerQueue) -> io::Result<Self> {
        let queue = Self::default();
        {
            let mut state = queue.state.lock();
            for event in persistent.events {
                let callback = event.callback.into_callback()?;
                state.schedule(event.id, event.trigger_time, callback);
            }
        }
        Ok(queue)
    }

    /// Vanilla `pack`: the queued events in order.
    fn pending_save(&self) -> Option<(u64, PersistentTimerQueue)> {
        let state = self.state.lock();
        let revision = self.revision.load(Ordering::Acquire);
        if revision == self.saved_revision.load(Ordering::Acquire) {
            return None;
        }
        let events = state
            .queue
            .iter()
            .map(|(&(trigger_time, _), event)| PersistentEvent {
                trigger_time,
                id: event.id.clone(),
                callback: PersistentCallback::from(&event.callback),
            })
            .collect();
        Some((revision, PersistentTimerQueue { events }))
    }

    fn mark_saved(&self, revision: u64) {
        self.saved_revision.fetch_max(revision, Ordering::Release);
    }
}

/// Vanilla `TimerQueue.Packed`.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct PersistentTimerQueue {
    events: Vec<PersistentEvent>,
}

/// Vanilla `TimerQueue.Event.Packed`.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PersistentEvent {
    trigger_time: i64,
    id: String,
    callback: PersistentCallback,
}

/// Vanilla `TimerCallbacks.SERVER_CALLBACKS`' dispatch codec.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
enum PersistentCallback {
    #[serde(rename = "minecraft:function")]
    Function { id: String },
    #[serde(rename = "minecraft:function_tag")]
    FunctionTag { id: String },
}

impl From<&TimerCallback> for PersistentCallback {
    fn from(callback: &TimerCallback) -> Self {
        match callback {
            TimerCallback::Function(id) => Self::Function { id: id.to_string() },
            TimerCallback::FunctionTag(id) => Self::FunctionTag { id: id.to_string() },
        }
    }
}

impl PersistentCallback {
    fn into_callback(self) -> io::Result<TimerCallback> {
        let (id, tag) = match self {
            Self::Function { id } => (id, false),
            Self::FunctionTag { id } => (id, true),
        };
        let id = id.parse::<Identifier>().map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid scheduled function id '{id}': {error}"),
            )
        })?;
        Ok(if tag {
            TimerCallback::FunctionTag(id)
        } else {
            TimerCallback::Function(id)
        })
    }
}

/// Every domain's [`TimerQueue`].
pub(crate) struct DomainTimerQueues {
    queues: BTreeMap<String, TimerQueue>,
    save_lock: AsyncMutex<()>,
}

impl DomainTimerQueues {
    /// Loads each domain's queue through its default world.
    pub(crate) async fn load(worlds: &WorldMap) -> io::Result<Self> {
        let mut queues = BTreeMap::new();
        for domain in worlds.domain_names() {
            let world = domain_default_world(worlds, domain)?;
            let persistent: PersistentTimerQueue = world
                .saved_data
                .load_or_default(saved_data_names::SCHEDULED_EVENTS)
                .await
                .map_err(|error| queue_io_error(domain, &error))?;
            let queue = TimerQueue::from_persistent(persistent)
                .map_err(|error| queue_io_error(domain, &error))?;
            queues.insert(domain.to_owned(), queue);
        }
        Ok(Self {
            queues,
            save_lock: AsyncMutex::new(()),
        })
    }

    /// The queue of `domain`.
    pub(crate) fn get(&self, domain: &str) -> Option<&TimerQueue> {
        self.queues.get(domain)
    }

    /// Saves every changed domain queue and returns the number written.
    pub(crate) async fn save(&self, worlds: &WorldMap) -> io::Result<usize> {
        let _save_guard = self.save_lock.lock().await;
        let mut saved = 0;
        for (domain, queue) in &self.queues {
            let Some((revision, persistent)) = queue.pending_save() else {
                continue;
            };
            let world = domain_default_world(worlds, domain)?;
            world
                .saved_data
                .save(saved_data_names::SCHEDULED_EVENTS, &persistent)
                .await
                .map_err(|error| queue_io_error(domain, &error))?;
            queue.mark_saved(revision);
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

fn queue_io_error(domain: &str, error: &io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("scheduled events I/O failed for domain '{domain}': {error}"),
    )
}

#[cfg(test)]
mod tests {
    use std::env::temp_dir;
    use std::{fs, process};

    use steel_utils::saved_data::SavedDataManager;

    use super::*;

    fn function(id: &str) -> TimerCallback {
        TimerCallback::Function(id.parse().expect("valid id"))
    }

    fn drain(queue: &TimerQueue, tick: i64) -> Vec<TimerCallback> {
        let mut due = Vec::new();
        while let Some(callback) = queue.pop_due(tick) {
            due.push(callback);
        }
        due
    }

    #[test]
    fn due_events_run_in_time_then_schedule_order() {
        let queue = TimerQueue::default();
        queue.schedule("demo:b".to_owned(), 5, function("demo:b"));
        queue.schedule("demo:a".to_owned(), 5, function("demo:a"));
        queue.schedule("demo:c".to_owned(), 3, function("demo:c"));
        queue.schedule("demo:late".to_owned(), 6, function("demo:late"));

        assert_eq!(drain(&queue, 2), []);
        assert_eq!(drain(&queue, 3), [function("demo:c")]);
        assert_eq!(drain(&queue, 5), [function("demo:b"), function("demo:a")]);
        assert_eq!(queue.event_ids(), ["demo:late"]);
    }

    #[test]
    fn the_same_id_and_time_is_scheduled_once() {
        let queue = TimerQueue::default();
        queue.schedule("demo:a".to_owned(), 5, function("demo:a"));
        queue.schedule("demo:a".to_owned(), 5, function("demo:other"));
        queue.schedule("demo:a".to_owned(), 7, function("demo:a"));

        assert_eq!(queue.remove("demo:a"), 2);
        assert_eq!(queue.remove("demo:a"), 0);
        assert_eq!(drain(&queue, 10), []);
    }

    #[test]
    fn an_event_run_late_stays_in_the_id_table() {
        let queue = TimerQueue::default();
        queue.schedule("demo:a".to_owned(), 5, function("demo:a"));
        queue.schedule("demo:b".to_owned(), 8, function("demo:b"));

        // Vanilla removes the table entry at the current tick, not the
        // event's own trigger time.
        assert_eq!(drain(&queue, 8).len(), 2);
        assert_eq!(queue.event_ids(), ["demo:a"]);
        assert_eq!(queue.remove("demo:a"), 1);
        assert_eq!(queue.event_ids(), [] as [String; 0]);
    }

    #[tokio::test]
    async fn the_queue_saves_in_order_and_reloads() {
        let path = temp_dir().join(format!("steel-timer-queue-{}", process::id()));
        let manager = SavedDataManager::new(Some(&path));
        let queue = TimerQueue::default();
        queue.schedule("demo:b".to_owned(), 9, function("demo:b"));
        let tag = TimerCallback::FunctionTag("demo:all".parse().expect("valid id"));
        queue.schedule("#demo:all".to_owned(), 4, tag.clone());

        let (revision, persistent) = queue.pending_save().expect("queue should be dirty");
        assert_eq!(
            persistent
                .events
                .iter()
                .map(|event| event.trigger_time)
                .collect::<Vec<_>>(),
            [4, 9]
        );
        manager
            .save(saved_data_names::SCHEDULED_EVENTS, &persistent)
            .await
            .expect("queue should save");
        queue.mark_saved(revision);
        assert!(queue.pending_save().is_none());

        let loaded: PersistentTimerQueue = manager
            .load_or_default(saved_data_names::SCHEDULED_EVENTS)
            .await
            .expect("queue should load");
        assert_eq!(loaded, persistent);
        let restored = TimerQueue::from_persistent(loaded).expect("ids should parse");
        assert_eq!(restored.event_ids(), ["#demo:all", "demo:b"]);
        assert_eq!(drain(&restored, 9), [tag, function("demo:b")]);
        let _ = fs::remove_dir_all(path);
    }
}
