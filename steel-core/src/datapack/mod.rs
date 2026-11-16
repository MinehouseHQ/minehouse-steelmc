//! Vanilla's `FolderRepositorySource` for a level's `datapacks` folder: every
//! folder or zip in it whose `pack.mcmeta` reads is a data pack. Which of them
//! are enabled is up to the [`PackRepository`].

use std::fs::{self, File};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use steel_utils::Identifier;
use zip::ZipArchive;

mod metadata;
mod repository;

#[cfg(test)]
pub(crate) use metadata::read_pack_metadata;
pub(crate) use metadata::{FeatureFlagSet, PackMetadata};
pub(crate) use repository::{Pack, PackRepositories, PackRepository, PackSource};

const PACK_METADATA: &str = "pack.mcmeta";

/// One data pack from a `datapacks` folder.
pub(crate) struct DataPack {
    /// Vanilla's pack id, `file/<file name>`.
    id: String,
    /// The file name, vanilla's pack title.
    name: String,
    root: PackRoot,
    metadata: PackMetadata,
}

enum PackRoot {
    Folder(PathBuf),
    Zip(PathBuf),
}

/// A resource file found in a pack.
pub(crate) struct PackResource {
    /// The id the file maps to, without the directory and extension.
    pub(crate) id: Identifier,
    /// The file's contents.
    pub(crate) contents: Vec<u8>,
}

impl DataPack {
    /// Vanilla's pack id, `file/<file name>`.
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    /// The pack's file name, vanilla's pack title.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// What the pack's `pack.mcmeta` says about it.
    pub(crate) const fn metadata(&self) -> &PackMetadata {
        &self.metadata
    }

    /// Vanilla `FileToIdConverter.listMatchingResources`: every file under
    /// `data/<namespace>/<directory>/` ending in `extension`, sorted by id
    /// (path first, as vanilla's `TreeMap<ResourceLocation, _>`).
    ///
    /// Files whose path is not a valid identifier are skipped and logged, as
    /// vanilla's resource listing does.
    pub(crate) fn list_resources(&self, directory: &str, extension: &str) -> Vec<PackResource> {
        let files = match &self.root {
            PackRoot::Folder(root) => folder_files(&root.join("data")),
            PackRoot::Zip(path) => zip_files(path),
        };
        let files = match files {
            Ok(files) => files,
            Err(error) => {
                log::error!("Failed to list resources in data pack {}: {error}", self.id);
                return Vec::new();
            }
        };
        let mut resources = files
            .into_iter()
            .filter_map(|(path, contents)| {
                let id = resource_id(&path, directory, extension)?;
                Some(PackResource {
                    id: id.ok().or_else(|| {
                        log::error!(
                            "Invalid path in data pack {}: data/{path}, ignoring",
                            self.id
                        );
                        None
                    })?,
                    contents: contents.ok().or_else(|| {
                        log::error!("Failed to read data/{path} in data pack {}", self.id);
                        None
                    })?,
                })
            })
            .collect::<Vec<_>>();
        resources.sort_by(|a, b| a.id.cmp(&b.id));
        resources
    }
}

/// The data packs in `directory`, sorted by id. A missing folder has none.
pub(crate) fn find_packs(directory: &Path) -> Vec<DataPack> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut packs = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let id = format!("file/{name}");
            let (root, metadata) = if path.is_dir() {
                let metadata = fs::read(path.join(PACK_METADATA)).ok();
                (PackRoot::Folder(path), metadata)
            } else if is_zip(&path) {
                (PackRoot::Zip(path.clone()), zip_metadata(&path))
            } else {
                log::warn!("Skipping {}: not a data pack", path.display());
                return None;
            };
            let Some(metadata) = metadata else {
                log::warn!("Missing metadata in pack {id}");
                return None;
            };
            Some(DataPack {
                metadata: metadata::read_pack_metadata(&id, &metadata)?,
                id,
                name,
                root,
            })
        })
        .collect::<Vec<_>>();
    packs.sort_by(|a, b| a.id.cmp(&b.id));
    packs
}

fn is_zip(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
}

/// A zip pack's `pack.mcmeta`, if it has one.
fn zip_metadata(path: &Path) -> Option<Vec<u8>> {
    let mut archive = ZipArchive::new(File::open(path).ok()?).ok()?;
    let mut file = archive.by_name(PACK_METADATA).ok()?;
    let mut contents = Vec::new();
    file.read_to_end(&mut contents).ok()?;
    Some(contents)
}

/// `path` relative to `data/`, as `<namespace>/<directory>/<rest><extension>`,
/// to the id `<namespace>:<rest>`. `None` for files of another kind; an invalid
/// id is `Some(Err)`.
fn resource_id(path: &str, directory: &str, extension: &str) -> Option<Result<Identifier, ()>> {
    let (namespace, rest) = path.split_once('/')?;
    let rest = rest.strip_prefix(directory)?.strip_prefix('/')?;
    let id_path = rest.strip_suffix(extension)?;
    Some(
        Identifier::validate(namespace, id_path)
            .then(|| Identifier::new(namespace.to_owned(), id_path.to_owned()))
            .ok_or(()),
    )
}

type PackFiles = Vec<(String, io::Result<Vec<u8>>)>;

/// Every file under a folder pack's `data`, as paths relative to it.
fn folder_files(data: &Path) -> io::Result<PackFiles> {
    let mut files = Vec::new();
    if !data.is_dir() {
        return Ok(files);
    }
    let mut pending = vec![data.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(relative) = path.strip_prefix(data) {
                let relative = relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                files.push((relative, fs::read(&path)));
            }
        }
    }
    Ok(files)
}

/// Every file under a zip pack's `data/`, as paths relative to it.
fn zip_files(path: &Path) -> io::Result<PackFiles> {
    let mut archive = ZipArchive::new(File::open(path)?).map_err(io::Error::other)?;
    let mut files = Vec::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(io::Error::other)?;
        if !file.is_file() {
            continue;
        }
        let name = file.name().map_err(io::Error::other)?;
        let Some(relative) = name.strip_prefix("data/").map(str::to_owned) else {
            continue;
        };
        let mut contents = Vec::new();
        let read = file.read_to_end(&mut contents).map(|_| contents);
        files.push((relative, read));
    }
    Ok(files)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write as _;
    use std::{env, process};

    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("steel-datapack-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir should be created");
        dir
    }

    /// A `pack.mcmeta` that reads.
    pub(crate) const PACK_MCMETA: &str =
        r#"{"pack":{"description":"","min_format":121,"max_format":121}}"#;

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("file has a parent")).expect("dirs");
        fs::write(path, contents).expect("file should be written");
    }

    #[test]
    fn folders_and_zips_with_metadata_are_packs_in_id_order() {
        let dir = temp_dir("find");
        write(&dir.join("b_pack/pack.mcmeta"), PACK_MCMETA);
        write(&dir.join("a_pack/pack.mcmeta"), PACK_MCMETA);
        write(&dir.join("broken_pack/pack.mcmeta"), "{}");
        write(&dir.join("not_a_pack/data/x.txt"), "");
        {
            let file = File::create(dir.join("c_pack.zip")).expect("zip file");
            let mut zip = ZipWriter::new(file);
            zip.start_file(PACK_METADATA, SimpleFileOptions::default())
                .expect("entry");
            zip.write_all(PACK_MCMETA.as_bytes()).expect("write");
            zip.start_file(
                "data/demo/function/hello.mcfunction",
                SimpleFileOptions::default(),
            )
            .expect("entry");
            zip.write_all(b"say hi").expect("write");
            zip.finish().expect("finish");
        }

        let packs = find_packs(&dir);
        let ids = packs.iter().map(DataPack::id).collect::<Vec<_>>();
        assert_eq!(ids, ["file/a_pack", "file/b_pack", "file/c_pack.zip"]);

        let functions = packs[2].list_resources("function", ".mcfunction");
        assert_eq!(functions.len(), 1);
        assert_eq!(functions[0].id.to_string(), "demo:hello");
        assert_eq!(functions[0].contents, b"say hi");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resources_map_to_ids_and_skip_invalid_paths() {
        let dir = temp_dir("resources");
        write(&dir.join("pack/pack.mcmeta"), PACK_MCMETA);
        write(&dir.join("pack/data/demo/function/a/b.mcfunction"), "");
        write(&dir.join("pack/data/demo/function/Upper.mcfunction"), "");
        write(&dir.join("pack/data/demo/function/notes.txt"), "");
        write(&dir.join("pack/data/demo/tags/function/load.json"), "{}");

        let packs = find_packs(&dir);
        let functions = packs[0].list_resources("function", ".mcfunction");
        let ids = functions
            .iter()
            .map(|resource| resource.id.to_string())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["demo:a/b"]);
        let tags = packs[0].list_resources("tags/function", ".json");
        assert_eq!(tags[0].id.to_string(), "demo:load");
        let _ = fs::remove_dir_all(&dir);
    }
}
