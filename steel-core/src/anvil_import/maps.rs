//! Conversion of vanilla map data (`data/minecraft/maps/<id>.dat` and `last_id.dat`) into the
//! map storage of the domain default world.
//!
//! Vanilla 26.x keeps maps in the server data storage, `world/data/minecraft/maps`. Older saves
//! upgraded by vanilla may still have them in the overworld's dimension data, which is read too;
//! a map in both places is taken from the server data storage.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use steel_registry::data_components::components::MapId;
use steel_registry::vanilla_dimension_types::OVERWORLD;
use steel_utils::saved_data::SavedDataManager;

use super::level::read_saved_data;
use super::nbt::compound_bytes;
use super::{AnvilImportError, DimensionImport};
use crate::world::maps::{DomainMaps, MapItemSavedData};

/// Vanilla `MapIndex.NO_MAP_ID`.
const NO_MAP_ID: i32 = -1;

/// What the map import wrote and what it could not carry over.
#[derive(Debug, Default)]
pub struct MapReport {
    /// Maps written.
    pub maps: usize,
    /// The last map id vanilla handed out; new maps continue after it.
    pub last_map_id: i32,
    /// Map files that could not be read, with the reason.
    pub failed: Vec<String>,
}

impl fmt::Display for MapReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "Imported {} maps (last map id {})",
            self.maps, self.last_map_id
        )?;
        if !self.failed.is_empty() {
            writeln!(f, "Maps that failed and were not imported:")?;
            for entry in &self.failed {
                writeln!(f, "  {entry}")?;
            }
        }
        Ok(())
    }
}

/// Writes the vanilla maps of `world` into the world of the overworld in `dimensions`, which
/// holds the maps of its domain in Steel.
///
/// Maps the target already has are kept, so the import can run again on an imported save.
pub fn import_maps(
    world: &Path,
    dimensions: &[DimensionImport],
) -> Result<MapReport, AnvilImportError> {
    let mut report = MapReport {
        last_map_id: NO_MAP_ID,
        ..MapReport::default()
    };
    let Some(overworld) = dimensions
        .iter()
        .find(|dimension| dimension.key == OVERWORLD.key)
    else {
        return Ok(report);
    };
    let world_dir = overworld.target.parent().unwrap_or(&overworld.target);
    let maps = DomainMaps::new();
    maps.set_storage(SavedDataManager::new(Some(world_dir)));

    let sources = [
        world.join("data").join("minecraft").join("maps"),
        overworld.source.join("data").join("minecraft").join("maps"),
    ];
    for directory in &sources {
        let last_id = read_saved_data(&directory.join("last_id.dat"))?
            .and_then(|index| index.int("map"))
            .unwrap_or(NO_MAP_ID);
        report.last_map_id = report.last_map_id.max(last_id);
    }
    let files = map_files(&sources)?;
    for (id, path) in files {
        let id = MapId::new(id);
        if maps.get(id).is_some() {
            continue;
        }
        let data = read_saved_data(&path).and_then(|data| {
            let data = data.ok_or_else(|| AnvilImportError::InvalidFile {
                path: path.clone(),
                reason: "missing file".to_owned(),
            })?;
            MapItemSavedData::from_nbt_bytes(&compound_bytes(&data)).map_err(|error| {
                AnvilImportError::InvalidFile {
                    path: path.clone(),
                    reason: error.to_string(),
                }
            })
        });
        match data {
            Ok(data) => {
                maps.set(id, data);
                report.maps += 1;
                report.last_map_id = report.last_map_id.max(id.id());
            }
            Err(error) => report.failed.push(error.to_string()),
        }
    }
    if report.last_map_id != NO_MAP_ID {
        maps.set_last_map_id(report.last_map_id);
    }
    maps.save().map_err(|source| AnvilImportError::Io {
        path: world_dir.join("data"),
        source,
    })?;
    Ok(report)
}

/// The map files of `sources` by id; an id found in an earlier source wins.
fn map_files(sources: &[PathBuf]) -> Result<BTreeMap<i32, PathBuf>, AnvilImportError> {
    let mut files = BTreeMap::new();
    for directory in sources {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(AnvilImportError::Io {
                    path: directory.clone(),
                    source,
                });
            }
        };
        for entry in entries {
            let path = entry
                .map_err(|source| AnvilImportError::Io {
                    path: directory.clone(),
                    source,
                })?
                .path();
            let id = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".dat"))
                .and_then(|id| id.parse::<i32>().ok())
                .filter(|id| *id >= 0);
            if let Some(id) = id {
                files.entry(id).or_insert(path);
            }
        }
    }
    Ok(files)
}
