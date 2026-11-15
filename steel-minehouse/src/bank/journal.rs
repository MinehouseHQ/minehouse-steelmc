//! `bank-journal.json` in the data directory: bank operations that are half done, so a crash
//! or a lost answer from the site never loses or doubles ores (the Fabric `Journal`).
//!
//! - `deposit`: ores were taken from the player, the site has not confirmed yet. Sent again
//!   (same op id, the site counts it once) until it answers.
//! - `delivered`: withdrawn ores were handed out, the site has not heard «delivered» yet. Sent
//!   again; meanwhile the op is skipped when the player's undelivered withdrawals are asked for.
//! - `vending`: goods were taken from a vending stock, the payment is in flight. On start the
//!   site is asked whether the payment exists: yes, the buyer gets the goods; no, they go back.
//! - `owed`: items a player gets on next join (refunds, goods of a vending purchase).
//!
//! The file is rewritten in full (temp file and rename) after every change.
//!
//! Items are kept as hex of their NBT. The Fabric journal kept them as SNBT, so a Fabric
//! journal with items in it is not read: move with an empty one.

use std::{
    fs,
    io::{self, Cursor},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use simdnbt::{ToNbtTag as _, borrow::read_compound, owned::NbtTag};
use steel_registry::item_stack::ItemStack;
use steel_utils::locks::SyncMutex;
use uuid::Uuid;

/// Ores taken from a player, not confirmed by the site yet.
pub const DEPOSIT: &str = "deposit";
/// Withdrawn ores handed out, the site has not heard yet.
pub const DELIVERED: &str = "delivered";
/// Goods taken from a vending stock while the payment is in flight.
pub const VENDING: &str = "vending";
/// Items a player gets on next join.
pub const OWED: &str = "owed";

/// One half-done operation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Entry {
    /// [`DEPOSIT`], [`DELIVERED`], [`VENDING`] or [`OWED`].
    pub kind: String,
    /// The id of the operation on the site.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    /// Whose operation it is.
    pub uuid: String,
    /// How many ores.
    pub amount: i32,
    /// Items, see [`encode`].
    pub items: Vec<String>,
    /// `vending`: the world of the terminal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// `vending`: where the terminal is.
    pub x: i32,
    /// `vending`: where the terminal is.
    pub y: i32,
    /// `vending`: where the terminal is.
    pub z: i32,
    /// `vending`: the id of the terminal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    /// `vending`: the account that is paid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_number: Option<String>,
    /// `vending`: the card that pays.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card: Option<String>,
    /// `deposit`: the account picked in the ATM; none is the personal one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// What the payment is for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// When, in milliseconds of Unix time.
    pub created_at: u64,
}

impl Entry {
    /// An entry of `kind` for `player`, made now.
    #[must_use]
    pub fn new(kind: &str, player: Uuid) -> Self {
        Self {
            kind: kind.to_owned(),
            uuid: player.to_string(),
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| {
                    u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
                }),
            ..Self::default()
        }
    }

    /// Items `player` gets on next join, as [`encode`] gives them.
    #[must_use]
    pub fn owed(player: Uuid, items: Vec<String>) -> Self {
        Self {
            items,
            ..Self::new(OWED, player)
        }
    }

    fn is(&self, kind: &str, op: &str) -> bool {
        self.kind == kind && self.op.as_deref() == Some(op)
    }
}

/// An item as it is kept in the journal: hex of its NBT. Nothing for an empty stack.
#[must_use]
pub fn encode(stack: &ItemStack) -> Option<String> {
    if stack.is_empty() {
        return None;
    }
    let NbtTag::Compound(compound) = stack.clone().to_nbt_tag() else {
        return None;
    };
    let mut bytes = Vec::new();
    compound.write(&mut bytes);
    Some(hex::encode(bytes))
}

/// The item of [`encode`], if it can be read.
#[must_use]
pub fn decode(text: &str) -> Option<ItemStack> {
    let bytes = hex::decode(text).ok()?;
    let nbt = read_compound(&mut Cursor::new(bytes.as_slice())).ok()?;
    ItemStack::from_borrowed_compound(&simdnbt::borrow::NbtCompound::from(&nbt))
}

/// The journal file and its entries.
pub struct Journal {
    file: PathBuf,
    entries: SyncMutex<Vec<Entry>>,
}

impl Journal {
    /// Reads the journal at `file`; a missing file is an empty journal.
    pub fn load(file: PathBuf) -> io::Result<Self> {
        let entries = match fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str(&text).map_err(io::Error::other)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        Ok(Self {
            file,
            entries: SyncMutex::new(entries),
        })
    }

    /// An empty journal that will be written to `file`.
    #[must_use]
    pub const fn empty(file: PathBuf) -> Self {
        Self {
            file,
            entries: SyncMutex::new(Vec::new()),
        }
    }

    /// Every entry.
    #[must_use]
    pub fn all(&self) -> Vec<Entry> {
        self.entries.lock().clone()
    }

    /// Whether operation `op` of `kind` is in the journal.
    #[must_use]
    pub fn has_op(&self, kind: &str, op: &str) -> bool {
        self.entries.lock().iter().any(|entry| entry.is(kind, op))
    }

    /// Adds an entry.
    pub fn add(&self, entry: Entry) {
        let mut entries = self.entries.lock();
        entries.push(entry);
        self.save(&entries);
    }

    /// Removes an entry. Returns whether it was there.
    pub fn remove(&self, entry: &Entry) -> bool {
        let mut entries = self.entries.lock();
        let Some(index) = entries.iter().position(|other| other == entry) else {
            return false;
        };
        entries.remove(index);
        self.save(&entries);
        true
    }

    /// Removes operation `op` of `kind`.
    pub fn remove_op(&self, kind: &str, op: &str) {
        let mut entries = self.entries.lock();
        let before = entries.len();
        entries.retain(|entry| !entry.is(kind, op));
        if entries.len() != before {
            self.save(&entries);
        }
    }

    /// Takes what `player` is owed out of the journal: one list of items per entry. An entry
    /// with an item that can't be read stays, for a human to look at.
    pub fn take_owed(&self, player: Uuid) -> Vec<Vec<ItemStack>> {
        let player = player.to_string();
        let mut entries = self.entries.lock();
        let mut owed = Vec::new();
        entries.retain(|entry| {
            if entry.kind != OWED || entry.uuid != player {
                return true;
            }
            let items: Option<Vec<ItemStack>> =
                entry.items.iter().map(|item| decode(item)).collect();
            if let Some(items) = items {
                owed.push(items);
                false
            } else {
                log::error!("Bank: could not read the items of a journal entry of {player}");
                true
            }
        });
        if !owed.is_empty() {
            self.save(&entries);
        }
        owed
    }

    fn save(&self, entries: &[Entry]) {
        // Losing the journal could lose ores: say it loudly, the copy in memory is still right.
        if let Err(error) = self.write(entries) {
            log::error!(
                "Bank: could not write the journal {}: {error}",
                self.file.display()
            );
        }
    }

    fn write(&self, entries: &[Entry]) -> io::Result<()> {
        let text = serde_json::to_string_pretty(entries).map_err(io::Error::other)?;
        if let Some(parent) = self.file.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut temp = self.file.clone().into_os_string();
        temp.push(".tmp");
        fs::write(&temp, text)?;
        fs::rename(&temp, &self.file)
    }
}

#[cfg(test)]
mod tests {
    use std::{env, process};

    use steel_registry::vanilla_items;

    use super::*;

    fn file(name: &str) -> PathBuf {
        env::temp_dir()
            .join(format!("steel-minehouse-bank-{}", process::id()))
            .join(name)
    }

    fn delivered(player: Uuid, op: &str) -> Entry {
        Entry {
            op: Some(op.to_owned()),
            amount: 3,
            ..Entry::new(DELIVERED, player)
        }
    }

    #[test]
    fn entries_survive_a_restart() {
        let file = file("restart.json");
        fs::remove_file(&file).ok();
        let player = Uuid::from_u128(7);
        let journal = Journal::load(file.clone()).unwrap_or_else(|error| panic!("{error}"));
        journal.add(delivered(player, "op-1"));
        journal.add(delivered(player, "op-2"));
        journal.remove_op(DELIVERED, "op-1");

        let again = Journal::load(file.clone()).unwrap_or_else(|error| panic!("{error}"));
        assert!(!again.has_op(DELIVERED, "op-1"));
        assert!(again.has_op(DELIVERED, "op-2"));
        assert!(!again.has_op(DEPOSIT, "op-2"));
        let entry = again.all().remove(0);
        assert!(again.remove(&entry));
        assert!(!again.remove(&entry));
        assert_eq!(fs::read_to_string(&file).unwrap_or_default(), "[]");
        fs::remove_file(&file).ok();
    }

    #[test]
    fn the_file_has_the_field_names_of_the_fabric_journal() {
        let entry = Entry {
            to_number: Some("MH-1".to_owned()),
            ..delivered(Uuid::from_u128(7), "op-1")
        };
        let text = serde_json::to_string(&entry).unwrap_or_default();
        assert!(text.contains("\"toNumber\":\"MH-1\""), "{text}");
        assert!(text.contains("\"createdAt\":"), "{text}");
        assert!(!text.contains("\"card\""), "{text}");
        let read: Entry =
            serde_json::from_str(r#"{"kind":"owed","uuid":"x","items":[],"createdAt":5}"#)
                .unwrap_or_default();
        assert_eq!(read.kind, OWED);
        assert_eq!(read.created_at, 5);
    }

    #[test]
    fn owed_items_are_taken_once_and_only_by_their_owner() {
        steel_registry::init_vanilla_registry();
        let file = file("owed.json");
        fs::remove_file(&file).ok();
        let (owner, other) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let journal = Journal::empty(file.clone());
        let items = [
            ItemStack::with_count(&vanilla_items::DIAMOND, 5),
            ItemStack::empty(),
        ];
        journal.add(Entry::owed(
            owner,
            items.iter().filter_map(encode).collect(),
        ));
        journal.add(Entry {
            items: vec!["not hex".to_owned()],
            ..Entry::new(OWED, owner)
        });

        assert_eq!(journal.take_owed(other).len(), 0);
        let owed = journal.take_owed(owner);
        assert_eq!(owed.len(), 1);
        assert_eq!(owed[0].len(), 1);
        assert!(owed[0][0].is(&vanilla_items::DIAMOND));
        assert_eq!(owed[0][0].count(), 5);
        // The entry that can't be read is still there, the good one is gone.
        assert_eq!(journal.take_owed(owner).len(), 0);
        assert_eq!(journal.all().len(), 1);
        fs::remove_file(&file).ok();
    }
}
