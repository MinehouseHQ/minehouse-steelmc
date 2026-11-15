//! Which minecarts are coupled to which. A port of the Fabric `CartLinks`, which is saved
//! with each cart; here it is one JSON file in the data directory, by the id of the cart.
//!
//! A cart couples to at most two others, so carts form a line, never a fork. A coupling is
//! kept on both carts. The chain spent on it is remembered on one side only, so it drops
//! once when the coupling breaks (nothing drops for one made in creative).

use std::{
    collections::{BTreeMap, VecDeque},
    fs, io,
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// How many carts one cart couples to.
pub const MAX: usize = 2;

/// One coupling of a cart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The cart on the other end.
    pub to: Uuid,
    /// The key of the chain item spent on it, on one of the two sides.
    pub chain: Option<String>,
}

/// A coupling that was broken.
#[derive(Debug, PartialEq, Eq)]
pub struct Broken {
    /// The key of the chain item it was made with, if one was spent on it.
    pub chain: Option<String>,
}

/// The couplings of one cart.
pub type Slots = [Option<Link>; MAX];

const FREE: Slots = [None, None];

/// One coupling in the file.
#[derive(Serialize, Deserialize)]
struct Entry {
    to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chain: Option<String>,
}

/// The couplings of every cart, kept in memory and written on each change.
pub struct Couplings {
    file: PathBuf,
    carts: BTreeMap<Uuid, Slots>,
}

impl Couplings {
    /// Reads `file`; a missing or broken file is no couplings.
    #[must_use]
    pub fn load(file: PathBuf) -> Self {
        let saved: BTreeMap<String, Vec<Entry>> = match fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
                log::error!("Could not read {}, starting empty: {error}", file.display());
                BTreeMap::new()
            }),
            Err(error) => {
                if error.kind() != io::ErrorKind::NotFound {
                    log::error!("Could not read {}, starting empty: {error}", file.display());
                }
                BTreeMap::new()
            }
        };
        let mut carts = BTreeMap::new();
        for (cart, entries) in saved {
            let Ok(cart) = Uuid::parse_str(&cart) else {
                continue;
            };
            let mut slots = FREE;
            let links = entries.into_iter().filter_map(|entry| {
                let to = Uuid::parse_str(&entry.to).ok()?;
                Some(Link {
                    to,
                    chain: entry.chain,
                })
            });
            for (slot, link) in slots.iter_mut().zip(links) {
                *slot = Some(link);
            }
            if slots != FREE {
                carts.insert(cart, slots);
            }
        }
        Self { file, carts }
    }

    /// The couplings of `cart`.
    #[must_use]
    pub fn slots(&self, cart: Uuid) -> &Slots {
        self.carts.get(&cart).unwrap_or(&FREE)
    }

    /// Whether `cart` is coupled to anything.
    #[must_use]
    pub fn is_linked(&self, cart: Uuid) -> bool {
        self.carts.contains_key(&cart)
    }

    /// Whether `cart` has both its couplings taken.
    #[must_use]
    pub fn is_full(&self, cart: Uuid) -> bool {
        self.slots(cart).iter().all(Option::is_some)
    }

    /// How many carts have a coupling.
    #[must_use]
    pub fn count(&self) -> usize {
        self.carts.len()
    }

    fn slot_of(&self, cart: Uuid, other: Uuid) -> Option<usize> {
        self.slots(cart)
            .iter()
            .position(|link| link.as_ref().is_some_and(|link| link.to == other))
    }

    /// Whether the two carts are coupled to each other directly.
    #[must_use]
    pub fn are_linked(&self, first: Uuid, second: Uuid) -> bool {
        self.slot_of(first, second).is_some() || self.slot_of(second, first).is_some()
    }

    /// The carts `cart` is coupled to.
    pub fn neighbours(&self, cart: Uuid) -> impl Iterator<Item = Uuid> + '_ {
        self.slots(cart).iter().flatten().map(|link| link.to)
    }

    fn put(&mut self, cart: Uuid, link: Link) -> bool {
        let slots = self.carts.entry(cart).or_insert(FREE);
        let Some(free) = slots.iter_mut().find(|slot| slot.is_none()) else {
            return false;
        };
        *free = Some(link);
        true
    }

    /// Couples two carts; `chain` is the item spent on it. `false` when they are the same
    /// cart, coupled already, or one of them has no coupling free.
    pub fn link(&mut self, first: Uuid, second: Uuid, chain: Option<String>) -> bool {
        if first == second
            || self.is_full(first)
            || self.is_full(second)
            || self.are_linked(first, second)
        {
            return false;
        }
        self.put(first, Link { to: second, chain });
        self.put(
            second,
            Link {
                to: first,
                chain: None,
            },
        );
        self.save();
        true
    }

    fn take(&mut self, cart: Uuid, slot: usize) -> Option<Link> {
        let slots = self.carts.get_mut(&cart)?;
        let link = slots.get_mut(slot)?.take();
        if *slots == FREE {
            self.carts.remove(&cart);
        }
        link
    }

    /// Breaks the coupling in `slot` of `cart`, on both carts.
    fn unlink_slot(&mut self, cart: Uuid, slot: usize) -> Option<Broken> {
        let link = self.take(cart, slot)?;
        let back = self
            .slot_of(link.to, cart)
            .and_then(|back| self.take(link.to, back));
        Some(Broken {
            chain: link.chain.or_else(|| back.and_then(|back| back.chain)),
        })
    }

    /// Breaks the coupling between two carts. `None` when they were not coupled.
    pub fn unlink(&mut self, cart: Uuid, other: Uuid) -> Option<Broken> {
        let slot = self.slot_of(cart, other)?;
        let broken = self.unlink_slot(cart, slot);
        self.save();
        broken
    }

    /// Breaks every coupling of `cart`. Gives the chains they were made with.
    pub fn unlink_all(&mut self, cart: Uuid) -> Vec<String> {
        let mut chains = Vec::new();
        let mut any = false;
        for slot in 0..MAX {
            if let Some(broken) = self.unlink_slot(cart, slot) {
                any = true;
                chains.extend(broken.chain);
            }
        }
        if any {
            self.save();
        }
        chains
    }

    /// Every cart coupled to `cart`, directly or through others, `cart` first. Only carts
    /// that are `here` are followed, and at most `limit` carts are gathered.
    pub fn train(&self, cart: Uuid, limit: usize, here: impl Fn(Uuid) -> bool) -> Vec<Uuid> {
        let mut train = vec![cart];
        let mut queue = VecDeque::from([cart]);
        while let Some(at) = queue.pop_front() {
            if train.len() >= limit {
                break;
            }
            for next in self.neighbours(at) {
                if !train.contains(&next) && here(next) {
                    train.push(next);
                    queue.push_back(next);
                }
            }
        }
        train
    }

    /// The carts of `train` in the order they are coupled, from one end. `None` when they
    /// are not one line (a ring, which the couplings never make).
    #[must_use]
    pub fn line(&self, train: &[Uuid]) -> Option<Vec<Uuid>> {
        let within = |cart: Uuid| {
            self.neighbours(cart)
                .filter(|next| train.contains(next))
                .collect::<Vec<_>>()
        };
        let end = train
            .iter()
            .copied()
            .find(|&cart| within(cart).len() <= 1)?;
        let mut line = vec![end];
        let mut at = end;
        while line.len() < train.len() {
            let next = within(at).into_iter().find(|next| !line.contains(next))?;
            line.push(next);
            at = next;
        }
        Some(line)
    }

    /// Writes through a temporary file, so a crash never leaves half a list.
    fn save(&self) {
        let saved: BTreeMap<String, Vec<Entry>> = self
            .carts
            .iter()
            .map(|(cart, slots)| {
                let entries = slots
                    .iter()
                    .flatten()
                    .map(|link| Entry {
                        to: link.to.to_string(),
                        chain: link.chain.clone(),
                    })
                    .collect();
                (cart.to_string(), entries)
            })
            .collect();
        let result = (|| {
            if let Some(parent) = self.file.parent() {
                fs::create_dir_all(parent)?;
            }
            let text = serde_json::to_string_pretty(&saved).map_err(io::Error::other)?;
            let temporary = self.file.with_extension("json.tmp");
            fs::write(&temporary, text)?;
            fs::rename(&temporary, &self.file)
        })();
        if let Err(error) = result {
            log::error!("Could not save {}: {error}", self.file.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{env, process};

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let directory =
            env::temp_dir().join(format!("minehouse-couplings-{}-{name}", process::id()));
        drop(fs::remove_dir_all(&directory));
        directory.join("metro-couplings.json")
    }

    fn cart(number: u128) -> Uuid {
        Uuid::from_u128(number)
    }

    #[test]
    fn couplings_come_back_after_a_restart() {
        let file = scratch("restart");
        let mut couplings = Couplings::load(file.clone());
        assert!(couplings.link(cart(1), cart(2), Some("minecraft:iron_chain".to_owned())));
        assert!(couplings.link(cart(2), cart(3), None));
        // Coupled already, the same cart, and a third coupling on one cart.
        assert!(!couplings.link(cart(2), cart(1), None));
        assert!(!couplings.link(cart(1), cart(1), None));
        assert!(!couplings.link(cart(2), cart(4), None));

        let couplings = Couplings::load(file);
        assert!(couplings.are_linked(cart(1), cart(2)));
        assert!(couplings.are_linked(cart(3), cart(2)));
        assert!(!couplings.are_linked(cart(1), cart(3)));
        assert!(couplings.is_full(cart(2)));
        assert_eq!(couplings.count(), 3);
    }

    #[test]
    fn a_chain_drops_once_whichever_side_breaks_it() {
        let mut couplings = Couplings::load(scratch("chain"));
        couplings.link(cart(1), cart(2), Some("minecraft:iron_chain".to_owned()));
        // The chain is kept on the first cart; the second breaks the coupling.
        assert_eq!(
            couplings.unlink(cart(2), cart(1)),
            Some(Broken {
                chain: Some("minecraft:iron_chain".to_owned())
            })
        );
        assert_eq!(couplings.unlink(cart(2), cart(1)), None);
        assert!(!couplings.is_linked(cart(1)));
        assert_eq!(couplings.count(), 0);

        couplings.link(cart(1), cart(2), Some("minecraft:iron_chain".to_owned()));
        couplings.link(cart(2), cart(3), None);
        assert_eq!(couplings.unlink_all(cart(2)), vec!["minecraft:iron_chain"]);
        assert_eq!(couplings.count(), 0);
    }

    #[test]
    fn a_train_is_walked_from_one_end() {
        let mut couplings = Couplings::load(scratch("line"));
        couplings.link(cart(2), cart(3), None);
        couplings.link(cart(2), cart(1), None);
        couplings.link(cart(3), cart(4), None);

        let train = couplings.train(cart(3), 24, |_| true);
        assert_eq!(train.len(), 4);
        let line = couplings.line(&train).unwrap_or_default();
        assert!(
            line == [cart(1), cart(2), cart(3), cart(4)]
                || line == [cart(4), cart(3), cart(2), cart(1)],
            "{line:?}"
        );
        // A cart that is not here cuts the train short.
        let train = couplings.train(cart(1), 24, |id| id != cart(3));
        assert_eq!(train, [cart(1), cart(2)]);
        assert_eq!(couplings.line(&train), Some(vec![cart(1), cart(2)]));
    }
}
