//! Vanilla `RandomSequences`: named, persistent random streams for `/random`.
//!
//! Like vanilla, only drawing from a sequence marks the data dirty;
//! `reset`, `clear` and new seed defaults are written with the next draw (or
//! never, if nothing draws before the server stops).

use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use steel_utils::Identifier;
use steel_utils::locks::{AsyncMutex, SyncMutex};
use steel_utils::random::xoroshiro::{Seed128, Xoroshiro};
use steel_utils::saved_data::names as saved_data_names;

use crate::server::worlds::WorldMap;
use crate::world::World;

/// Vanilla `RandomSequence.createSequence`.
fn create_sequence(seed: i64, key: Option<&Identifier>) -> Xoroshiro {
    let mut seed = Seed128::upgrade_unmixed(seed.cast_unsigned());
    if let Some(key) = key {
        seed = seed.xor(Seed128::from_hash_of(&key.to_string()));
    }
    Xoroshiro::from_seed_128(seed.mixed())
}

/// Vanilla's `salt`, `include_world_seed` and `include_sequence_id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeedDefaults {
    /// Mixed into every new sequence's seed.
    pub salt: i32,
    /// Whether the world seed is part of a new sequence's seed.
    pub include_world_seed: bool,
    /// Whether the sequence id is part of a new sequence's seed.
    pub include_sequence_id: bool,
}

impl Default for SeedDefaults {
    fn default() -> Self {
        Self {
            salt: 0,
            include_world_seed: true,
            include_sequence_id: true,
        }
    }
}

impl SeedDefaults {
    /// Vanilla `RandomSequences.createSequence` with these settings.
    fn create(self, key: &Identifier, world_seed: i64) -> Xoroshiro {
        let seed = (if self.include_world_seed {
            world_seed
        } else {
            0
        }) ^ i64::from(self.salt);
        create_sequence(seed, self.include_sequence_id.then_some(key))
    }
}

struct State {
    defaults: SeedDefaults,
    sequences: BTreeMap<Identifier, Xoroshiro>,
}

/// A domain's random sequences.
pub struct RandomSequences {
    state: SyncMutex<State>,
    revision: AtomicU64,
    saved_revision: AtomicU64,
}

impl Default for RandomSequences {
    fn default() -> Self {
        Self::new()
    }
}

impl RandomSequences {
    /// No sequences, vanilla's default seed settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: SyncMutex::new(State {
                defaults: SeedDefaults::default(),
                sequences: BTreeMap::new(),
            }),
            revision: AtomicU64::new(0),
            saved_revision: AtomicU64::new(0),
        }
    }

    /// Vanilla `get(key, worldSeed)` and one draw through its
    /// `DirtyMarkingRandomSource`: creates the sequence on first use.
    pub fn draw<R>(
        &self,
        key: &Identifier,
        world_seed: i64,
        draw: impl FnOnce(&mut Xoroshiro) -> R,
    ) -> R {
        let mut state = self.state.lock();
        let defaults = state.defaults;
        let random = state
            .sequences
            .entry(key.clone())
            .or_insert_with(|| defaults.create(key, world_seed));
        let result = draw(random);
        drop(state);
        self.revision.fetch_add(1, Ordering::Release);
        result
    }

    /// Vanilla `forAllSequences`, as the ids in order.
    #[must_use]
    pub fn ids(&self) -> Vec<Identifier> {
        self.state.lock().sequences.keys().cloned().collect()
    }

    /// The settings new sequences are seeded with.
    #[must_use]
    pub fn seed_defaults(&self) -> SeedDefaults {
        self.state.lock().defaults
    }

    /// Vanilla `setSeedDefaults`.
    pub fn set_seed_defaults(&self, defaults: SeedDefaults) {
        self.state.lock().defaults = defaults;
    }

    /// Vanilla `clear`: drops every sequence and returns how many there were.
    pub fn clear(&self) -> usize {
        let mut state = self.state.lock();
        let count = state.sequences.len();
        state.sequences.clear();
        count
    }

    /// Vanilla `reset(id, worldSeed)`: restarts a sequence from the defaults.
    pub fn reset(&self, id: &Identifier, world_seed: i64) {
        let mut state = self.state.lock();
        let random = state.defaults.create(id, world_seed);
        state.sequences.insert(id.clone(), random);
    }

    /// Vanilla `reset(id, worldSeed, salt, includeWorldSeed, includeSequenceId)`.
    pub fn reset_with(&self, id: &Identifier, world_seed: i64, settings: SeedDefaults) {
        self.state
            .lock()
            .sequences
            .insert(id.clone(), settings.create(id, world_seed));
    }

    fn from_persistent(persistent: PersistentRandomSequences) -> io::Result<Self> {
        let mut sequences = BTreeMap::new();
        for (raw_id, sequence) in persistent.sequences {
            let id = raw_id.parse::<Identifier>().map_err(|error| {
                invalid_data(format!("invalid random sequence id '{raw_id}': {error}"))
            })?;
            let [lo, hi] = sequence.source;
            let seed = Seed128 {
                lo: lo.cast_unsigned(),
                hi: hi.cast_unsigned(),
            };
            sequences.insert(id, Xoroshiro::from_seed_128(seed));
        }
        Ok(Self {
            state: SyncMutex::new(State {
                defaults: SeedDefaults {
                    salt: persistent.salt,
                    include_world_seed: persistent.include_world_seed,
                    include_sequence_id: persistent.include_sequence_id,
                },
                sequences,
            }),
            revision: AtomicU64::new(0),
            saved_revision: AtomicU64::new(0),
        })
    }

    fn pending_save(&self) -> Option<(u64, PersistentRandomSequences)> {
        let state = self.state.lock();
        let revision = self.revision.load(Ordering::Acquire);
        if revision == self.saved_revision.load(Ordering::Acquire) {
            return None;
        }
        let sequences = state
            .sequences
            .iter()
            .map(|(id, random)| {
                let seed = random.state();
                let source = [seed.lo.cast_signed(), seed.hi.cast_signed()];
                (id.to_string(), PersistentRandomSequence { source })
            })
            .collect();
        Some((
            revision,
            PersistentRandomSequences {
                salt: state.defaults.salt,
                include_world_seed: state.defaults.include_world_seed,
                include_sequence_id: state.defaults.include_sequence_id,
                sequences,
            },
        ))
    }

    fn mark_saved(&self, revision: u64) {
        self.saved_revision.fetch_max(revision, Ordering::Release);
    }
}

const fn default_true() -> bool {
    true
}

/// Vanilla `RandomSequence.CODEC`: the Xoroshiro state as two longs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PersistentRandomSequence {
    source: [i64; 2],
}

/// Vanilla `RandomSequences.CODEC`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PersistentRandomSequences {
    salt: i32,
    #[serde(default = "default_true")]
    include_world_seed: bool,
    #[serde(default = "default_true")]
    include_sequence_id: bool,
    sequences: BTreeMap<String, PersistentRandomSequence>,
}

impl Default for PersistentRandomSequences {
    fn default() -> Self {
        Self {
            salt: 0,
            include_world_seed: true,
            include_sequence_id: true,
            sequences: BTreeMap::new(),
        }
    }
}

/// Every domain's [`RandomSequences`].
pub struct DomainRandomSequences {
    domains: BTreeMap<String, RandomSequences>,
    save_lock: AsyncMutex<()>,
}

impl DomainRandomSequences {
    /// Loads each domain's sequences through its default world.
    pub(crate) async fn load(worlds: &WorldMap) -> io::Result<Self> {
        let mut domains = BTreeMap::new();
        for domain in worlds.domain_names() {
            let world = domain_default_world(worlds, domain)?;
            let persistent: PersistentRandomSequences = world
                .saved_data
                .load_or_default(saved_data_names::RANDOM_SEQUENCES)
                .await
                .map_err(|error| sequences_io_error(domain, &error))?;
            let sequences = RandomSequences::from_persistent(persistent)
                .map_err(|error| sequences_io_error(domain, &error))?;
            domains.insert(domain.to_owned(), sequences);
        }
        Ok(Self {
            domains,
            save_lock: AsyncMutex::new(()),
        })
    }

    /// Returns a domain's sequences.
    #[must_use]
    pub fn get(&self, domain: &str) -> Option<&RandomSequences> {
        self.domains.get(domain)
    }

    /// Saves every changed domain's sequences, returning how many were written.
    pub async fn save(&self, worlds: &WorldMap) -> io::Result<usize> {
        let _save_guard = self.save_lock.lock().await;
        let mut saved = 0;
        for (domain, sequences) in &self.domains {
            let Some((revision, persistent)) = sequences.pending_save() else {
                continue;
            };
            let world = domain_default_world(worlds, domain)?;
            world
                .saved_data
                .save(saved_data_names::RANDOM_SEQUENCES, &persistent)
                .await
                .map_err(|error| sequences_io_error(domain, &error))?;
            sequences.mark_saved(revision);
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

fn invalid_data(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn sequences_io_error(domain: &str, error: &io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("random sequence I/O failed for domain '{domain}': {error}"),
    )
}

#[cfg(test)]
mod tests;
