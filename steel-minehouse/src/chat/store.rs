//! What the chat keeps between restarts: mutes and ignore lists, as JSON in the data
//! directory. The mute file has the shape of the Fabric mod's `minehouse-chat-mutes.json`,
//! so the mutes of the old server can be copied over.

use std::{collections::BTreeMap, fs, io, path::PathBuf};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use steel_utils::locks::SyncMutex;
use uuid::Uuid;

/// One muted player.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Mute {
    /// The nickname when muted, for `/mutelist` and `/unmute` of an offline player.
    pub name: String,
    /// Why.
    pub reason: String,
    /// Who muted: a nickname or `Server`.
    pub by: String,
    /// Epoch milliseconds.
    pub since: i64,
    /// Epoch milliseconds; `None` is a mute without an end.
    pub until: Option<i64>,
}

impl Mute {
    fn expired(&self, now: i64) -> bool {
        self.until.is_some_and(|until| until <= now)
    }

    /// Whole seconds left at `now`, at least one; `None` for a mute without an end.
    #[must_use]
    pub fn seconds_left(&self, now: i64) -> Option<u64> {
        self.until
            .map(|until| u64::try_from((until - now) / 1000).unwrap_or(0).max(1))
    }
}

/// Muted players. Expired mutes drop out on the next look.
pub struct Mutes {
    file: PathBuf,
    by_player: SyncMutex<BTreeMap<Uuid, Mute>>,
}

impl Mutes {
    /// Reads `file`; an unreadable one is logged and starts empty.
    #[must_use]
    pub fn load(file: PathBuf) -> Self {
        let by_player = SyncMutex::new(read(&file));
        Self { file, by_player }
    }

    /// The active mute of `player`.
    #[must_use]
    pub fn get(&self, player: Uuid, now: i64) -> Option<Mute> {
        let mut by_player = self.by_player.lock();
        let mute = by_player.get(&player)?.clone();
        if mute.expired(now) {
            by_player.remove(&player);
            write(&self.file, &*by_player);
            return None;
        }
        Some(mute)
    }

    /// Mutes or re-mutes: a new mute replaces the old one.
    pub fn put(&self, player: Uuid, mute: Mute) {
        let mut by_player = self.by_player.lock();
        by_player.insert(player, mute);
        write(&self.file, &*by_player);
    }

    /// Removes the mute of `player` and returns it if it was still active.
    pub fn remove(&self, player: Uuid, now: i64) -> Option<Mute> {
        let mut by_player = self.by_player.lock();
        let mute = by_player.remove(&player)?;
        write(&self.file, &*by_player);
        (!mute.expired(now)).then_some(mute)
    }

    /// A muted player by the nickname they had when muted, for `/unmute` of someone offline.
    #[must_use]
    pub fn find_by_name(&self, name: &str) -> Option<Uuid> {
        self.by_player
            .lock()
            .iter()
            .find(|(_, mute)| mute.name.eq_ignore_ascii_case(name))
            .map(|(player, _)| *player)
    }

    /// Active mutes, oldest first.
    #[must_use]
    pub fn active(&self, now: i64) -> Vec<Mute> {
        let mut by_player = self.by_player.lock();
        let before = by_player.len();
        by_player.retain(|_, mute| !mute.expired(now));
        if by_player.len() != before {
            write(&self.file, &*by_player);
        }
        let mut mutes: Vec<Mute> = by_player.values().cloned().collect();
        mutes.sort_by_key(|mute| mute.since);
        mutes
    }
}

/// Who ignores whom: player → (ignored player → their nickname when ignored).
pub struct Ignores {
    file: PathBuf,
    by_player: SyncMutex<BTreeMap<Uuid, BTreeMap<Uuid, String>>>,
}

impl Ignores {
    /// Reads `file`; an unreadable one is logged and starts empty.
    #[must_use]
    pub fn load(file: PathBuf) -> Self {
        let by_player = SyncMutex::new(read(&file));
        Self { file, by_player }
    }

    /// Whether `player` ignores `other`.
    #[must_use]
    pub fn ignores(&self, player: Uuid, other: Uuid) -> bool {
        self.by_player
            .lock()
            .get(&player)
            .is_some_and(|ignored| ignored.contains_key(&other))
    }

    /// Flips the ignore. `true` if `other` is now ignored, `false` if the ignore was removed.
    pub fn toggle(&self, player: Uuid, other: Uuid, other_name: &str) -> bool {
        let mut by_player = self.by_player.lock();
        let ignored = by_player.entry(player).or_default();
        let added = ignored.remove(&other).is_none();
        if added {
            ignored.insert(other, other_name.to_owned());
        }
        if ignored.is_empty() {
            by_player.remove(&player);
        }
        write(&self.file, &*by_player);
        added
    }

    /// The nicknames `player` ignores.
    #[must_use]
    pub fn names(&self, player: Uuid) -> Vec<String> {
        self.by_player
            .lock()
            .get(&player)
            .map_or_default(|ignored| ignored.values().cloned().collect())
    }
}

fn read<T: DeserializeOwned + Default>(file: &PathBuf) -> T {
    match fs::read_to_string(file) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
            log::error!("Could not read {}, starting empty: {error}", file.display());
            T::default()
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => T::default(),
        Err(error) => {
            log::error!("Could not read {}, starting empty: {error}", file.display());
            T::default()
        }
    }
}

/// Writes through a temporary file, so a crash never leaves half a list.
fn write<T: Serialize>(file: &PathBuf, value: &T) {
    let result = (|| {
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
        let temporary = file.with_extension("json.tmp");
        fs::write(&temporary, text)?;
        fs::rename(&temporary, file)
    })();
    if let Err(error) = result {
        log::error!("Could not save {}: {error}", file.display());
    }
}

#[cfg(test)]
mod tests {
    use std::{env, process};

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let directory = env::temp_dir().join(format!("minehouse-chat-{}-{name}", process::id()));
        drop(fs::remove_dir_all(&directory));
        directory
    }

    fn mute(name: &str, since: i64, until: Option<i64>) -> Mute {
        Mute {
            name: name.to_owned(),
            reason: "спам".to_owned(),
            by: "Server".to_owned(),
            since,
            until,
        }
    }

    #[test]
    fn mutes_expire_and_survive_a_restart() {
        let directory = scratch("mutes");
        let file = directory.join("chat-mutes.json");
        let (alice, bob) = (Uuid::from_u128(1), Uuid::from_u128(2));

        let mutes = Mutes::load(file.clone());
        mutes.put(bob, mute("Bob", 2_000, None));
        mutes.put(alice, mute("Alice", 1_000, Some(61_000)));
        assert_eq!(
            mutes.get(alice, 1_000).map(|mute| mute.name),
            Some("Alice".to_owned())
        );
        assert_eq!(
            mutes
                .get(alice, 1_000)
                .and_then(|mute| mute.seconds_left(1_000)),
            Some(60)
        );
        assert_eq!(mutes.find_by_name("bob"), Some(bob));

        let reloaded = Mutes::load(file);
        let names: Vec<String> = reloaded
            .active(5_000)
            .into_iter()
            .map(|mute| mute.name)
            .collect();
        assert_eq!(names, ["Alice", "Bob"]);
        assert_eq!(reloaded.get(alice, 61_000), None);
        assert_eq!(reloaded.remove(alice, 61_000), None);
        assert_eq!(
            reloaded.remove(bob, 61_000).map(|mute| mute.name),
            Some("Bob".to_owned())
        );
        assert_eq!(reloaded.active(61_000), []);
        drop(fs::remove_dir_all(directory));
    }

    #[test]
    fn the_mute_file_of_the_fabric_mod_reads_as_is() {
        let text = r#"{"00000000-0000-0000-0000-000000000001":
            {"name":"Alice","reason":"спам","by":"Server","since":5,"until":null}}"#;
        let parsed: BTreeMap<Uuid, Mute> =
            serde_json::from_str(text).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            parsed.get(&Uuid::from_u128(1)),
            Some(&mute("Alice", 5, None))
        );
    }

    #[test]
    fn ignores_toggle_and_survive_a_restart() {
        let directory = scratch("ignores");
        let file = directory.join("chat-ignores.json");
        let (alice, bob) = (Uuid::from_u128(1), Uuid::from_u128(2));

        let ignores = Ignores::load(file.clone());
        assert!(ignores.toggle(alice, bob, "Bob"));
        assert!(ignores.ignores(alice, bob));
        assert!(!ignores.ignores(bob, alice));

        let reloaded = Ignores::load(file);
        assert_eq!(reloaded.names(alice), ["Bob"]);
        assert!(!reloaded.toggle(alice, bob, "Bob"));
        assert!(!reloaded.ignores(alice, bob));
        assert_eq!(reloaded.names(alice), Vec::<String>::new());
        drop(fs::remove_dir_all(directory));
    }
}
