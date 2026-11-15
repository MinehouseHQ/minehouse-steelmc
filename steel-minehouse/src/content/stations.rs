//! What the metro knows of each station block: its name and its line. A port of the
//! Fabric `StationData`, which is saved data of a dimension; here it is one JSON file in
//! the data directory, with the stations of every world.

use std::{collections::BTreeMap, fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};
use steel_registry::dye_color::DyeColor;
use steel_utils::{BlockPos, Identifier, locks::SyncMutex};

use crate::config;

/// The name of a station is cut to this many characters, as on the Fabric server.
pub const NAME_LIMIT: usize = 48;

/// What is known of one station.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Station {
    /// The name a name tag gave it.
    pub name: Option<String>,
    /// The line a dye put it on.
    pub line: Option<DyeColor>,
}

/// One station in the file.
#[derive(Serialize, Deserialize)]
struct Entry {
    pos: [i32; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// The serialized name of the dye, as in the saved data of the Fabric mod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    line: Option<String>,
}

type Key = (i32, i32, i32);

const fn key(pos: BlockPos) -> Key {
    (pos.x(), pos.y(), pos.z())
}

/// The stations of every world, kept in memory and written on each change.
pub struct Stations {
    file: PathBuf,
    worlds: BTreeMap<String, BTreeMap<Key, Station>>,
}

impl Stations {
    /// Reads `file`; a missing or broken file is no stations.
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
        let worlds = saved
            .into_iter()
            .map(|(world, entries)| {
                let stations = entries
                    .into_iter()
                    .map(|entry| {
                        let [x, y, z] = entry.pos;
                        let line = entry
                            .line
                            .as_deref()
                            .and_then(DyeColor::from_serialized_name);
                        let station = Station {
                            name: entry.name,
                            line,
                        };
                        ((x, y, z), station)
                    })
                    .collect();
                (world, stations)
            })
            .collect();
        Self { file, worlds }
    }

    /// The station at `pos`, when the metro knows one there.
    #[must_use]
    pub fn get(&self, world: &Identifier, pos: BlockPos) -> Option<&Station> {
        self.worlds.get(&world.to_string())?.get(&key(pos))
    }

    /// How many stations there are, in every world.
    #[must_use]
    pub fn count(&self) -> usize {
        self.worlds.values().map(BTreeMap::len).sum()
    }

    /// Makes the block at `pos` a station the metro knows. Says whether it was new.
    pub fn add(&mut self, world: &Identifier, pos: BlockPos) -> bool {
        let stations = self.worlds.entry(world.to_string()).or_default();
        if stations.contains_key(&key(pos)) {
            return false;
        }
        stations.insert(key(pos), Station::default());
        self.save();
        true
    }

    /// Names the station, adding it when it was not known.
    pub fn set_name(&mut self, world: &Identifier, pos: BlockPos, name: String) {
        self.station(world, pos).name = Some(name);
        self.save();
    }

    /// Puts the station on a line, adding it when it was not known.
    pub fn set_line(&mut self, world: &Identifier, pos: BlockPos, line: DyeColor) {
        self.station(world, pos).line = Some(line);
        self.save();
    }

    /// Forgets the station. Says whether there was one.
    pub fn remove(&mut self, world: &Identifier, pos: BlockPos) -> bool {
        let Some(stations) = self.worlds.get_mut(&world.to_string()) else {
            return false;
        };
        if stations.remove(&key(pos)).is_none() {
            return false;
        }
        if stations.is_empty() {
            self.worlds.remove(&world.to_string());
        }
        self.save();
        true
    }

    fn station(&mut self, world: &Identifier, pos: BlockPos) -> &mut Station {
        self.worlds
            .entry(world.to_string())
            .or_default()
            .entry(key(pos))
            .or_default()
    }

    /// Every station, by its world and the `(x, y, z)` of its block.
    #[must_use]
    pub fn all(&self) -> BTreeMap<String, BTreeMap<(i32, i32, i32), Station>> {
        self.worlds
            .iter()
            .map(|(world, stations)| (world.clone(), stations.clone()))
            .collect()
    }

    /// Writes through a temporary file, so a crash never leaves half a list.
    fn save(&self) {
        super::map_markers::changed();
        let saved: BTreeMap<&str, Vec<Entry>> = self
            .worlds
            .iter()
            .map(|(world, stations)| {
                let entries = stations
                    .iter()
                    .map(|(&(x, y, z), station)| Entry {
                        pos: [x, y, z],
                        name: station.name.clone(),
                        line: station.line.map(|line| line.serialized_name().to_owned()),
                    })
                    .collect();
                (world.as_str(), entries)
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

static STATIONS: SyncMutex<Option<Stations>> = SyncMutex::new(None);

/// Runs `action` on the stations of the server, reading the file on the first use.
pub fn with<T>(action: impl FnOnce(&mut Stations) -> T) -> T {
    let mut stations = STATIONS.lock();
    let stations =
        stations.get_or_insert_with(|| Stations::load(config::data_path("metro-stations.json")));
    action(stations)
}

/// A name for a station from the text of a name tag: trimmed and cut to [`NAME_LIMIT`]
/// characters.
#[must_use]
pub fn clean_name(text: &str) -> String {
    text.trim().chars().take(NAME_LIMIT).collect()
}

#[cfg(test)]
mod tests {
    use std::{env, process};

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let directory =
            env::temp_dir().join(format!("minehouse-stations-{}-{name}", process::id()));
        drop(fs::remove_dir_all(&directory));
        directory.join("metro-stations.json")
    }

    fn world(path: &'static str) -> Identifier {
        Identifier::new_static("minecraft", path)
    }

    #[test]
    fn stations_come_back_after_a_restart() {
        let file = scratch("restart");
        let nether = world("the_nether");
        let pos = BlockPos::new(-12, 64, 300);
        let mut stations = Stations::load(file.clone());
        assert!(stations.add(&nether, pos));
        assert!(!stations.add(&nether, pos));
        stations.set_name(&nether, pos, "Спавн".to_owned());
        stations.set_line(&nether, pos, DyeColor::LightBlue);
        stations.set_name(
            &world("overworld"),
            BlockPos::new(1, 2, 3),
            "Дом".to_owned(),
        );

        let stations = Stations::load(file.clone());
        let station = stations.get(&nether, pos);
        assert_eq!(
            station,
            Some(&Station {
                name: Some("Спавн".to_owned()),
                line: Some(DyeColor::LightBlue),
            })
        );
        assert_eq!(stations.count(), 2);
        assert_eq!(stations.get(&world("overworld"), pos), None);
        // The line is saved by the name of its dye, as the Fabric mod saves it.
        let text = fs::read_to_string(&file).unwrap_or_default();
        assert!(text.contains("\"light_blue\""), "{text}");
    }

    #[test]
    fn a_removed_station_is_forgotten() {
        let file = scratch("remove");
        let overworld = world("overworld");
        let pos = BlockPos::new(0, 70, 0);
        let mut stations = Stations::load(file.clone());
        stations.set_name(&overworld, pos, "А".to_owned());
        assert!(stations.remove(&overworld, pos));
        assert!(!stations.remove(&overworld, pos));
        assert_eq!(Stations::load(file).count(), 0);
    }

    #[test]
    fn a_name_is_trimmed_and_cut() {
        assert_eq!(clean_name("  Центральная  "), "Центральная");
        assert_eq!(clean_name(&"я".repeat(60)).chars().count(), NAME_LIMIT);
    }
}
