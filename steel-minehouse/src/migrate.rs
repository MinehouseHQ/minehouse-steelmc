//! Carries what our Fabric mods kept in the world over to Steel, as a part of
//! `steel import-anvil`.
//!
//! The world itself is converted by `steel_core::anvil_import`. Our blocks and items go
//! with it once [`register`] ran. What is left is the data our mods saved beside the blocks:
//!
//! - the stations of the metro, with their names and lines: the saved data
//!   `minehouse-tweaks/metro_stations.dat` of each dimension becomes `metro-stations.json`;
//! - the payment terminals: on Fabric a terminal was a block entity, here they are
//!   `bank-terminals.json`.
//!
//! Both files are written to the data directory of the imported save, and both stores are the
//! ones the server reads, so what is written is what it will load.

use std::path::Path;

use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_core::anvil_import::read_gzip_root;
use steel_registry::dye_color::DyeColor;
use steel_utils::{BlockPos, Identifier};

use crate::{
    bank::{journal, terminals::Terminals},
    content::{self, stations::Stations},
};

/// The block entity of a payment terminal on the Fabric server.
const TERMINAL: &str = "minehouse-bank:terminal";

/// The id the terminals of prod are saved under: the block Polymer showed the terminal as.
/// A stone button is no block entity in vanilla, so one that carries the fields of a
/// terminal is a terminal.
const TERMINAL_AS_SAVED: &str = "minecraft:stone_button";

/// Whether a block entity the import dropped is a payment terminal.
fn is_terminal(saved: &NbtCompound) -> bool {
    match saved.string("id").map(|id| id.to_str()) {
        Some(id) if id == TERMINAL => true,
        Some(id) if id == TERMINAL_AS_SAVED => {
            saved.string("account").is_some() && saved.string("mode").is_some()
        }
        _ => false,
    }
}

/// Has the import know our blocks and items. Call before the registry is built.
pub fn register() {
    content::register_for_import();
}

/// One imported dimension.
pub struct Dimension<'a> {
    /// The id of the world in Steel, `minecraft:overworld`.
    pub key: &'a Identifier,
    /// The folder of the dimension in the Fabric world, with its `data/`.
    pub source: &'a Path,
    /// The block entities the import dropped in it.
    pub dropped_block_entities: &'a [NbtCompound],
}

/// Writes our stores into `data` (the `MINEHOUSE_DATA_DIR` of the new server) from the
/// imported dimensions. Returns lines for the report of the import.
#[must_use]
pub fn carry_over(dimensions: &[Dimension<'_>], data: &Path) -> Vec<String> {
    let mut lines = Vec::new();

    let mut stations = Stations::load(data.join("metro-stations.json"));
    let mut found = 0;
    for dimension in dimensions {
        let file = dimension
            .source
            .join("data/minehouse-tweaks/metro_stations.dat");
        if !file.exists() {
            continue;
        }
        match read_gzip_root(&file) {
            Ok(root) => {
                for (pos, name, line) in saved_stations(&root) {
                    stations.add(dimension.key, pos);
                    if let Some(name) = name {
                        stations.set_name(dimension.key, pos, name);
                    }
                    if let Some(line) = line {
                        stations.set_line(dimension.key, pos, line);
                    }
                    found += 1;
                }
            }
            Err(error) => lines.push(format!("MineHouse: metro stations were not read: {error}")),
        }
    }
    lines.push(format!("MineHouse: {found} metro stations carried over"));

    let terminals = Terminals::load(data.join("bank-terminals.json"));
    let mut set_up = 0;
    let mut found = 0;
    for dimension in dimensions {
        for saved in dimension.dropped_block_entities {
            if !is_terminal(saved) {
                continue;
            }
            let (Some(x), Some(y), Some(z)) = (saved.int("x"), saved.int("y"), saved.int("z"))
            else {
                continue;
            };
            let pos = BlockPos::new(x, y, z);
            terminals.ensure(dimension.key, pos);
            terminals.change(dimension.key, pos, |terminal| {
                let text = |key: &str| {
                    saved
                        .string(key)
                        .map_or_default(|value| value.to_str().into_owned())
                };
                terminal.account = text("account");
                terminal.owner = text("owner");
                let mode = text("mode");
                if !mode.is_empty() {
                    terminal.mode = mode;
                }
                terminal.locked = saved.byte("locked").is_some_and(|locked| locked != 0);
                terminal.price = saved.long("price").unwrap_or(0);
                terminal.count = saved.int("count").unwrap_or(1);
                terminal.sample = saved.compound("sample").and_then(sample_text);
                set_up += usize::from(terminal.locked);
            });
            found += 1;
        }
    }
    lines.push(format!(
        "MineHouse: {found} payment terminals carried over, {set_up} of them set up"
    ));
    let strangers: Vec<&NbtCompound> = dimensions
        .iter()
        .flat_map(|dimension| dimension.dropped_block_entities)
        .filter(|saved| {
            !is_terminal(saved)
                && saved
                    .string("id")
                    .is_some_and(|id| id.to_str() == TERMINAL_AS_SAVED)
        })
        .collect();
    if let Some(first) = strangers.first() {
        let mut fields: Vec<String> = first.iter().map(|(name, _)| name.to_string()).collect();
        fields.sort();
        lines.push(format!(
            "MineHouse: {} block entities saved as {TERMINAL_AS_SAVED} are no terminals, the first has the fields: {}",
            strangers.len(),
            fields.join(", ")
        ));
    }
    lines
}

/// A saved item the way the bank writes items, if Steel can read it.
fn sample_text(sample: &NbtCompound) -> Option<String> {
    let mut bytes = Vec::new();
    sample.write(&mut bytes);
    let text = hex::encode(bytes);
    journal::decode(&text).map(|_| text)
}

/// The stations of a `metro_stations.dat`: `{data: [{pos: [I; x, y, z], name?, line?}]}`.
fn saved_stations(root: &NbtCompound) -> Vec<(BlockPos, Option<String>, Option<DyeColor>)> {
    let Some(NbtTag::List(NbtList::Compound(entries))) = root.get("data") else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let &[x, y, z] = entry.int_array("pos")? else {
                return None;
            };
            let text = |key: &str| entry.string(key).map(|value| value.to_str().into_owned());
            let line = text("line").and_then(|line| DyeColor::from_serialized_name(&line));
            Some((BlockPos::new(x, y, z), text("name"), line))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{env, fs, path::PathBuf, process};

    use steel_core::bootstrap::init_globals;
    use steel_registry::{item_stack::ItemStack, vanilla_items};

    use super::*;

    fn directory(name: &str) -> PathBuf {
        let directory = env::temp_dir().join(format!("minehouse-migrate-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("temp dir");
        directory
    }

    #[test]
    fn stations_are_read_from_the_saved_data() {
        let mut named = NbtCompound::new();
        named.insert("pos", NbtTag::IntArray(vec![10, 64, -3]));
        named.insert("name", "Спавн");
        named.insert("line", "red");
        let mut bare = NbtCompound::new();
        bare.insert("pos", NbtTag::IntArray(vec![0, 70, 0]));
        let mut root = NbtCompound::new();
        root.insert("data", NbtList::Compound(vec![named, bare]));

        let stations = saved_stations(&root);
        assert_eq!(stations.len(), 2);
        assert_eq!(stations[0].0, BlockPos::new(10, 64, -3));
        assert_eq!(stations[0].1.as_deref(), Some("Спавн"));
        assert_eq!(stations[0].2, Some(DyeColor::Red));
        assert_eq!(stations[1].1, None);
        assert_eq!(stations[1].2, None);
    }

    #[test]
    fn terminals_become_the_file_the_bank_reads() {
        init_globals();
        let data = directory("terminals");
        let source = directory("terminals-source");
        let world = Identifier::new_static("minecraft", "overworld");

        let stack = ItemStack::with_count(&vanilla_items::DIAMOND, 3);
        let NbtTag::Compound(sample) = stack.to_nbt_tag_ref() else {
            panic!("an item is a compound");
        };
        let mut terminal = NbtCompound::new();
        terminal.insert("id", TERMINAL);
        terminal.insert("x", 5);
        terminal.insert("y", 70);
        terminal.insert("z", -8);
        terminal.insert("account", "MH-1234-5678");
        terminal.insert("owner", "Alice");
        terminal.insert("mode", "vending");
        terminal.insert("locked", 1_i8);
        terminal.insert("price", 12_i64);
        terminal.insert("count", 3);
        terminal.insert("sample", sample);
        let mut chest = NbtCompound::new();
        chest.insert("id", "other:thing");
        let mut as_saved = NbtCompound::new();
        as_saved.insert("id", TERMINAL_AS_SAVED);
        as_saved.insert("x", 6);
        as_saved.insert("y", 70);
        as_saved.insert("z", -8);
        as_saved.insert("account", "");
        as_saved.insert("mode", "till");
        let mut button = NbtCompound::new();
        button.insert("id", TERMINAL_AS_SAVED);
        button.insert("x", 7);
        button.insert("y", 70);
        button.insert("z", -8);
        let dropped = [terminal, chest, as_saved, button];

        let lines = carry_over(
            &[Dimension {
                key: &world,
                source: &source,
                dropped_block_entities: &dropped,
            }],
            &data,
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("2 payment terminals"))
        );

        let read = Terminals::load(data.join("bank-terminals.json"));
        let terminal = read
            .get(&world, BlockPos::new(5, 70, -8))
            .expect("the terminal is in the file");
        assert_eq!(terminal.account, "MH-1234-5678");
        assert_eq!(terminal.owner, "Alice");
        assert!(terminal.vending());
        assert!(terminal.locked);
        assert_eq!(terminal.price, 12);
        assert_eq!(terminal.count, 3);
        let sample = terminal.sample_stack().expect("the sample is kept");
        assert!(sample.is(&vanilla_items::DIAMOND));
        assert!(read.get(&world, BlockPos::new(6, 70, -8)).is_some());
        assert_eq!(read.count(), 2);
        let _ = fs::remove_dir_all(&data);
        let _ = fs::remove_dir_all(&source);
    }
}
