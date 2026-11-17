//! Vanilla `StructureTemplateManager`: the templates structure blocks save and
//! load, kept in the world's `generated` folder over the bundled vanilla ones.

use std::io::{Cursor, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fs, io};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use rustc_hash::FxHashMap;
use simdnbt::borrow::{Nbt as BorrowedNbt, read as read_nbt, read_compound};
use simdnbt::owned::{BaseNbt, NbtCompound, NbtList};
use steel_registry::{REGISTRY, vanilla_template_pools};
use steel_utils::Identifier;
use steel_utils::locks::SyncMutex;

use super::StructureTemplate;

/// The folder under each namespace that holds saved templates.
const STRUCTURE_DIRECTORY_NAME: &str = "structure";
/// Vanilla `STRUCTURE_FILE_EXTENSION`.
const STRUCTURE_FILE_EXTENSION: &str = "nbt";

/// A template in the manager's repository: vanilla's `StructureTemplate`, kept
/// as the compound vanilla `StructureTemplate.save` writes.
#[derive(Clone, Debug)]
pub(crate) struct StoredTemplate {
    /// The template in vanilla's saved form.
    pub(crate) nbt: NbtCompound,
    /// Vanilla `StructureTemplate.author`, which is never saved.
    pub(crate) author: String,
}

impl StoredTemplate {
    /// Vanilla `new StructureTemplate()`: no blocks, no entities, size zero.
    fn empty() -> Self {
        let mut nbt = NbtCompound::new();
        super::capture::write_empty_template(&mut nbt);
        Self {
            nbt,
            author: "?".to_owned(),
        }
    }

    /// The template's blocks and entities, ready to place.
    pub(crate) fn parse(&self, id: &Identifier) -> Option<StructureTemplate> {
        let mut bytes = Vec::new();
        self.nbt.write(&mut bytes);
        let borrowed = read_compound(&mut Cursor::new(bytes.as_slice())).ok()?;
        StructureTemplate::load_compound(&REGISTRY, &(&borrowed).into(), &id.to_string()).ok()
    }

    /// Vanilla `getSize`.
    pub(crate) fn size(&self) -> [i32; 3] {
        let ints = self
            .nbt
            .list("size")
            .and_then(NbtList::ints)
            .unwrap_or_default();
        [0, 1, 2].map(|axis| ints.get(axis).copied().unwrap_or(0))
    }
}

/// Vanilla `StructureTemplateManager`.
pub(crate) struct StructureTemplateManager {
    /// The world's `generated` folder; `None` for a world that saves nothing.
    generated_dir: Option<PathBuf>,
    /// Vanilla `structureRepository`: every template looked up, or `None` for
    /// one that does not exist.
    repository: SyncMutex<FxHashMap<Identifier, Option<StoredTemplate>>>,
}

impl StructureTemplateManager {
    /// A manager reading and writing templates under `generated_dir`.
    pub(crate) fn new(generated_dir: Option<PathBuf>) -> Self {
        Self {
            generated_dir,
            repository: SyncMutex::new(FxHashMap::default()),
        }
    }

    /// Vanilla `get`: the template with `id`, loaded once from the world's
    /// folder or else from the bundled vanilla templates.
    pub(crate) fn get(&self, id: &Identifier) -> Option<StoredTemplate> {
        self.repository
            .lock()
            .entry(id.clone())
            .or_insert_with(|| self.try_load(id))
            .clone()
    }

    /// Vanilla `getOrCreate` followed by changing the template: `update`
    /// edits the stored template, a new empty one if `id` had none.
    pub(crate) fn update(&self, id: &Identifier, update: impl FnOnce(&mut StoredTemplate)) {
        let mut repository = self.repository.lock();
        let entry = repository
            .entry(id.clone())
            .or_insert_with(|| self.try_load(id));
        update(entry.get_or_insert_with(StoredTemplate::empty));
    }

    /// Vanilla `remove`: forgets the cached template, so the next lookup
    /// reads it again.
    pub(crate) fn remove(&self, id: &Identifier) {
        self.repository.lock().remove(id);
    }

    /// Vanilla `save`: writes the cached template to the world's folder.
    /// Returns whether it was written.
    pub(crate) fn save(&self, id: &Identifier) -> bool {
        let Some(template) = self.repository.lock().get(id).cloned().flatten() else {
            return false;
        };
        let Some(path) = self.template_path(id) else {
            return false;
        };
        match write_compressed(&path, &template.nbt) {
            Ok(()) => true,
            Err(error) => {
                log::warn!(
                    "Failed to save structure file {id} to {}: {error}",
                    path.display()
                );
                false
            }
        }
    }

    /// Vanilla `tryLoad`: the world's folder first, then the bundled templates.
    fn try_load(&self, id: &Identifier) -> Option<StoredTemplate> {
        let from_world = self.template_path(id).and_then(|path| fs::read(path).ok());
        let bytes = match from_world {
            Some(bytes) => bytes,
            None => vanilla_template_pools::vanilla_template_nbt_bytes(id)?.to_vec(),
        };
        let nbt = read_compressed(&bytes)?;
        Some(StoredTemplate {
            nbt,
            author: "?".to_owned(),
        })
    }

    /// Vanilla `TemplatePathFactory.createAndValidatePathToStructure`:
    /// `generated/<namespace>/structure/<path>.nbt`, refusing paths that would
    /// leave that folder.
    fn template_path(&self, id: &Identifier) -> Option<PathBuf> {
        let generated = self.generated_dir.as_deref()?;
        let parts: Vec<&str> = id.path.split('/').collect();
        if parts
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..")
            || matches!(&*id.namespace, "" | "." | "..")
        {
            return None;
        }
        let mut path = generated
            .join(&*id.namespace)
            .join(STRUCTURE_DIRECTORY_NAME);
        for part in parts {
            path.push(part);
        }
        path.set_extension(STRUCTURE_FILE_EXTENSION);
        Some(path)
    }
}

fn read_compressed(bytes: &[u8]) -> Option<NbtCompound> {
    let mut data = Vec::new();
    GzDecoder::new(bytes).read_to_end(&mut data).ok()?;
    match read_nbt(&mut Cursor::new(data.as_slice())).ok()? {
        BorrowedNbt::Some(root) => Some(root.as_compound().to_owned()),
        BorrowedNbt::None => None,
    }
}

/// Vanilla `NbtIo.writeCompressed`.
fn write_compressed(path: &Path, nbt: &NbtCompound) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut bytes = Vec::new();
    BaseNbt::new("", nbt.clone()).write(&mut bytes);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&bytes)?;
    fs::write(path, encoder.finish()?)
}

/// The manager a world without one of its own uses: nothing is saved.
pub(crate) fn memory_only_manager() -> Arc<StructureTemplateManager> {
    Arc::new(StructureTemplateManager::new(None))
}
