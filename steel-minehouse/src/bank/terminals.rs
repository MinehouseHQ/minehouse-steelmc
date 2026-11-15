//! What each payment terminal is set to: the Fabric `TerminalBlockEntity`.
//!
//! On the Fabric server this was a block entity. Here it is one JSON file in the data
//! directory, `bank-terminals.json`, with the terminals of every world by position. It is
//! rewritten (temp file and rename) after every change. The invoice of a till lives in memory
//! only, as on the Fabric server: a restart drops it.

use std::{
    collections::BTreeMap,
    fs, io,
    path::PathBuf,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use steel_registry::item_stack::ItemStack;
use steel_utils::{BlockPos, Identifier, locks::SyncMutex};
use uuid::Uuid;

use super::journal;

/// The mode of a terminal where the seller names the amount.
pub const TILL: &str = "till";
/// The mode of a terminal that sells from the chest under it.
pub const VENDING: &str = "vending";

/// What a till asks for right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invoice {
    /// Tells this invoice from the next one of the same till.
    pub id: Uuid,
    /// АР.
    pub amount: i64,
    /// Until when a buyer may pay it.
    pub until: Instant,
    /// A payment of it is on its way to the site.
    pub paying: bool,
}

/// One terminal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Terminal {
    /// The id the site knows the terminal by.
    pub id: Uuid,
    /// The number of the account the money goes to, `MH-1234-5678`; empty until set.
    #[serde(default)]
    pub account: String,
    /// The nickname of the owner of that account, shown to buyers.
    #[serde(default)]
    pub owner: String,
    /// [`TILL`] or [`VENDING`].
    #[serde(default = "till")]
    pub mode: String,
    /// Set up and fixed to its account: only breaking the block undoes it.
    #[serde(default)]
    pub locked: bool,
    /// Vending: the price of one sale, АР.
    #[serde(default)]
    pub price: i64,
    /// Vending: how many of the sample one sale hands out.
    #[serde(default = "one")]
    pub count: i32,
    /// Vending: one of the goods, as the journal writes items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
    /// The till's invoice.
    #[serde(skip)]
    pub invoice: Option<Invoice>,
    /// What the label above the terminal says now, so it is only sent when it changes.
    #[serde(skip)]
    pub label: Option<String>,
}

fn till() -> String {
    TILL.to_owned()
}

const fn one() -> i32 {
    1
}

impl Terminal {
    /// A terminal nobody has set up.
    #[must_use]
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            account: String::new(),
            owner: String::new(),
            mode: till(),
            locked: false,
            price: 0,
            count: 1,
            sample: None,
            invoice: None,
            label: None,
        }
    }

    /// Whether it sells from the chest under it.
    #[must_use]
    pub fn vending(&self) -> bool {
        self.mode == VENDING
    }

    /// One of the goods.
    #[must_use]
    pub fn sample_stack(&self) -> Option<ItemStack> {
        let stack = journal::decode(self.sample.as_deref()?)?;
        (!stack.is_empty()).then_some(stack)
    }

    /// The invoice, while a buyer may still pay it.
    #[must_use]
    pub fn active_invoice(&self) -> Option<&Invoice> {
        self.invoice
            .as_ref()
            .filter(|invoice| Instant::now() < invoice.until)
    }

    /// Asks the next buyer for `amount`, for `seconds`.
    pub fn set_invoice(&mut self, amount: i64, seconds: u64) {
        self.invoice = Some(Invoice {
            id: Uuid::new_v4(),
            amount,
            until: Instant::now() + Duration::from_secs(seconds),
            paying: false,
        });
    }
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new()
    }
}

/// One terminal in the file.
#[derive(Serialize, Deserialize)]
struct Saved {
    pos: [i32; 3],
    #[serde(flatten)]
    terminal: Terminal,
}

type Key = (i32, i32, i32);

const fn key(pos: BlockPos) -> Key {
    (pos.x(), pos.y(), pos.z())
}

/// The terminals of every world, kept in memory and written on each change.
pub struct Terminals {
    file: PathBuf,
    worlds: SyncMutex<BTreeMap<String, BTreeMap<Key, Terminal>>>,
}

impl Terminals {
    /// Reads `file`; a missing or broken file is no terminals.
    #[must_use]
    pub fn load(file: PathBuf) -> Self {
        let saved: BTreeMap<String, Vec<Saved>> = match fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
                log::error!("Bank: could not read {}: {error}", file.display());
                BTreeMap::new()
            }),
            Err(error) => {
                if error.kind() != io::ErrorKind::NotFound {
                    log::error!("Bank: could not read {}: {error}", file.display());
                }
                BTreeMap::new()
            }
        };
        let worlds = saved
            .into_iter()
            .map(|(world, terminals)| {
                let terminals = terminals
                    .into_iter()
                    .map(|saved| {
                        let [x, y, z] = saved.pos;
                        ((x, y, z), saved.terminal)
                    })
                    .collect();
                (world, terminals)
            })
            .collect();
        Self {
            file,
            worlds: SyncMutex::new(worlds),
        }
    }

    fn save(&self, worlds: &BTreeMap<String, BTreeMap<Key, Terminal>>) {
        let saved: BTreeMap<&String, Vec<Saved>> = worlds
            .iter()
            .filter(|(_, terminals)| !terminals.is_empty())
            .map(|(world, terminals)| {
                let terminals = terminals
                    .iter()
                    .map(|(&(x, y, z), terminal)| Saved {
                        pos: [x, y, z],
                        terminal: terminal.clone(),
                    })
                    .collect();
                (world, terminals)
            })
            .collect();
        let written = serde_json::to_string_pretty(&saved)
            .map_err(io::Error::other)
            .and_then(|text| {
                let mut temp = self.file.clone().into_os_string();
                temp.push(".tmp");
                if let Some(parent) = self.file.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&temp, text)?;
                fs::rename(&temp, &self.file)
            });
        if let Err(error) = written {
            log::error!("Bank: could not write {}: {error}", self.file.display());
        }
    }

    /// The terminal at `pos`, as it is now.
    #[must_use]
    pub fn get(&self, world: &Identifier, pos: BlockPos) -> Option<Terminal> {
        self.worlds
            .lock()
            .get(&world.to_string())?
            .get(&key(pos))
            .cloned()
    }

    /// The terminal at `pos`, made new if the bank knows none there.
    pub fn ensure(&self, world: &Identifier, pos: BlockPos) -> Terminal {
        let mut worlds = self.worlds.lock();
        let terminals = worlds.entry(world.to_string()).or_default();
        if let Some(terminal) = terminals.get(&key(pos)) {
            return terminal.clone();
        }
        let terminal = Terminal::new();
        terminals.insert(key(pos), terminal.clone());
        self.save(&worlds);
        terminal
    }

    /// Changes what is saved of the terminal at `pos`. None when there is none.
    pub fn change<R>(
        &self,
        world: &Identifier,
        pos: BlockPos,
        change: impl FnOnce(&mut Terminal) -> R,
    ) -> Option<R> {
        let mut worlds = self.worlds.lock();
        let terminal = worlds.get_mut(&world.to_string())?.get_mut(&key(pos))?;
        let before = terminal.clone();
        let result = change(terminal);
        // The invoice and the label are not in the file.
        let same = Terminal {
            invoice: before.invoice.clone(),
            label: before.label.clone(),
            ..terminal.clone()
        } == before;
        if !same {
            self.save(&worlds);
        }
        Some(result)
    }

    /// Forgets the terminal at `pos`.
    pub fn remove(&self, world: &Identifier, pos: BlockPos) -> Option<Terminal> {
        let mut worlds = self.worlds.lock();
        let removed = worlds.get_mut(&world.to_string())?.remove(&key(pos))?;
        self.save(&worlds);
        Some(removed)
    }

    /// Where the terminals of `world` are.
    #[must_use]
    pub fn places(&self, world: &Identifier) -> Vec<BlockPos> {
        self.worlds
            .lock()
            .get(&world.to_string())
            .into_iter()
            .flat_map(BTreeMap::keys)
            .map(|&(x, y, z)| BlockPos::new(x, y, z))
            .collect()
    }

    /// How many terminals there are, in every world.
    #[must_use]
    pub fn count(&self) -> usize {
        self.worlds.lock().values().map(BTreeMap::len).sum()
    }
}

#[cfg(test)]
mod tests {
    use std::{env, str::FromStr as _};

    use super::*;

    fn overworld() -> Identifier {
        Identifier::from_str("minecraft:overworld").unwrap_or_else(|_| unreachable!())
    }

    fn file(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("minehouse-terminals-{}", Uuid::new_v4()));
        dir.join(name)
    }

    #[test]
    fn a_terminal_survives_a_restart_without_its_invoice() {
        let file = file("bank-terminals.json");
        let pos = BlockPos::new(1, 64, -3);
        let terminals = Terminals::load(file.clone());
        let made = terminals.ensure(&overworld(), pos);
        terminals.change(&overworld(), pos, |terminal| {
            terminal.account = "MH-0000-0042".to_owned();
            terminal.owner = "seller".to_owned();
            terminal.mode = VENDING.to_owned();
            terminal.price = 7;
            terminal.count = 16;
            terminal.locked = true;
            terminal.set_invoice(5, 120);
        });
        assert!(
            terminals
                .get(&overworld(), pos)
                .is_some_and(|terminal| terminal.active_invoice().is_some())
        );

        let again = Terminals::load(file);
        let loaded = again.get(&overworld(), pos);
        assert_eq!(
            loaded,
            Some(Terminal {
                id: made.id,
                account: "MH-0000-0042".to_owned(),
                owner: "seller".to_owned(),
                mode: VENDING.to_owned(),
                locked: true,
                price: 7,
                count: 16,
                sample: None,
                invoice: None,
                label: None,
            })
        );
        assert_eq!(again.places(&overworld()), vec![pos]);
        assert_eq!(again.count(), 1);
    }

    #[test]
    fn a_removed_terminal_is_gone_from_the_file() {
        let file = file("bank-terminals.json");
        let pos = BlockPos::new(0, 70, 0);
        let terminals = Terminals::load(file.clone());
        terminals.ensure(&overworld(), pos);
        assert!(terminals.remove(&overworld(), pos).is_some());
        assert!(terminals.remove(&overworld(), pos).is_none());
        assert_eq!(Terminals::load(file).count(), 0);
    }

    #[test]
    fn an_invoice_stops_being_active_when_its_time_is_up() {
        let mut terminal = Terminal::new();
        assert!(terminal.active_invoice().is_none());
        terminal.set_invoice(3, 0);
        assert!(terminal.active_invoice().is_none());
        terminal.set_invoice(3, 60);
        assert_eq!(
            terminal.active_invoice().map(|invoice| invoice.amount),
            Some(3)
        );
    }
}
