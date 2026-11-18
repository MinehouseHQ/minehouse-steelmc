//! The skins players chose, one file per player: `skins/<uuid>.json` in the data directory,
//! which lives in the save. The files have the format of the Fabric `SkinRestorer`
//! (`world/skinrestorer/<uuid>.json` on prod), so the prod files are copied over as they are.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use steel_registry::game_profile::ProfileProperty;
use uuid::Uuid;

use crate::config;

/// Where the textures of a skin came from, as `SkinRestorer` names them.
pub(crate) const MOJANG: &str = "mojang";
pub(crate) const ELY_BY: &str = "ely.by";
pub(crate) const WEB: &str = "web";
/// No skin: Steve or Alex.
pub(crate) const EMPTY: &str = "empty";

/// A signed `textures` property, written like authlib's `Property`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Textures {
    #[serde(default = "textures_name")]
    pub name: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

fn textures_name() -> String {
    "textures".to_owned()
}

impl Textures {
    /// The property, if it fits the network limits.
    pub(crate) fn property(&self) -> Option<ProfileProperty> {
        ProfileProperty::new(
            "textures".to_owned(),
            self.value.clone(),
            self.signature.clone(),
        )
        .ok()
    }
}

/// The skin of one player.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredSkin {
    /// `mojang`, `ely.by`, `web`, `empty`; prod may also have others.
    pub provider: String,
    /// The nickname or link the skin was taken from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument: Option<String>,
    /// `classic` or `slim` for a skin from a link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// The textures; none for `empty`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Textures>,
    /// The `SkinRestorer` format version: kept so the file stays readable by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i32>,
}

impl StoredSkin {
    pub(crate) fn new(
        provider: &str,
        argument: Option<String>,
        variant: Option<String>,
        value: Option<Textures>,
    ) -> Self {
        Self {
            provider: provider.to_owned(),
            argument,
            variant,
            value,
            version: None,
        }
    }

    /// Whether the textures are looked up again by the nickname on every join, like
    /// `SkinRestorer`'s `refreshSkin` (a link is signed once and kept).
    pub(crate) fn refreshes(&self) -> bool {
        (self.provider == MOJANG || self.provider == ELY_BY) && self.argument.is_some()
    }
}

/// The folder of the skin files.
#[derive(Clone)]
pub(crate) struct SkinStore {
    directory: PathBuf,
}

impl SkinStore {
    pub(crate) fn open() -> Self {
        Self::at(config::data_path("skins"))
    }

    pub(crate) const fn at(directory: PathBuf) -> Self {
        Self { directory }
    }

    fn file(&self, uuid: Uuid) -> PathBuf {
        self.directory.join(format!("{}.json", uuid.hyphenated()))
    }

    pub(crate) fn load(&self, uuid: Uuid) -> Option<StoredSkin> {
        let file = self.file(uuid);
        let text = match fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
            Err(error) => {
                log::warn!("Skins: {} can't be read: {error}", file.display());
                return None;
            }
        };
        serde_json::from_str(&text)
            .inspect_err(|error| log::warn!("Skins: {} is broken: {error}", file.display()))
            .ok()
    }

    pub(crate) fn save(&self, uuid: Uuid, skin: &StoredSkin) {
        if let Err(error) = write(&self.file(uuid), skin) {
            log::error!("Skins: the skin of {uuid} was not saved: {error}");
        }
    }

    pub(crate) fn delete(&self, uuid: Uuid) {
        match fs::remove_file(self.file(uuid)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => log::error!("Skins: the skin of {uuid} was not deleted: {error}"),
        }
    }

    /// Copies the `SkinRestorer` files of `source` (prod's `world/skinrestorer`) that read as
    /// skins. Returns how many were copied and the names of those that were not.
    pub(crate) fn import(&self, source: &Path) -> io::Result<(usize, Vec<String>)> {
        let mut copied = 0;
        let mut skipped = Vec::new();
        for entry in fs::read_dir(source)? {
            let path = entry?.path();
            let name = path
                .file_name()
                .map_or_default(|name| name.to_string_lossy().into_owned());
            let skin = name
                .strip_suffix(".json")
                .and_then(|uuid| Uuid::parse_str(uuid).ok())
                .zip(
                    fs::read_to_string(&path)
                        .ok()
                        .and_then(|text| serde_json::from_str::<StoredSkin>(&text).ok()),
                );
            match skin {
                Some((uuid, skin)) => {
                    write(&self.file(uuid), &skin)?;
                    copied += 1;
                }
                None => skipped.push(name),
            }
        }
        Ok((copied, skipped))
    }
}

fn write(file: &Path, skin: &StoredSkin) -> io::Result<()> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(skin).map_err(io::Error::other)?;
    let temporary = file.with_extension("json.tmp");
    fs::write(&temporary, text)?;
    fs::rename(temporary, file)
}

#[cfg(test)]
mod tests {
    use std::{env, process};

    use super::*;

    #[test]
    fn prod_files_are_read_and_copied() {
        let root = env::temp_dir().join(format!("minehouse-skins-{}", process::id()));
        let _ = fs::remove_dir_all(&root);
        let source = root.join("skinrestorer");
        fs::create_dir_all(&source).expect("temp dir");
        let uuid = Uuid::from_u128(0x1234);
        // As SkinRestorer 2.11 writes them: Gson with `originalValue` and a version stamp.
        fs::write(
            source.join(format!("{}.json", uuid.hyphenated())),
            r#"{"provider":"web","argument":"https://example.com/a.png","variant":"slim",
                "value":{"name":"textures","value":"abc","signature":"sig"},
                "originalValue":{"name":"textures","value":"old"},"version":1}"#,
        )
        .expect("skin file");
        fs::write(source.join("cache.json"), "{}").expect("other file");

        let store = SkinStore::at(root.join("skins"));
        let (copied, skipped) = store.import(&source).expect("import");
        assert_eq!((copied, skipped), (1, vec!["cache.json".to_owned()]));
        let skin = store.load(uuid).expect("copied skin");
        assert_eq!(skin.provider, WEB);
        assert_eq!(skin.variant.as_deref(), Some("slim"));
        assert_eq!(
            skin.value.and_then(|textures| textures.signature),
            Some("sig".to_owned())
        );
        assert!(!store.load(uuid).is_some_and(|skin| skin.refreshes()));
        let _ = fs::remove_dir_all(&root);
    }
}
