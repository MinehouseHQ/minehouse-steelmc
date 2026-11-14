//! Module configs in `config/minehouse/<name>.toml`.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use serde::{Serialize, de::DeserializeOwned};

/// Directory with the `MineHouse` module configs, next to Steel's `config/config.toml`.
pub const CONFIG_DIR: &str = "config/minehouse";

/// Path of the config file of module `name`.
#[must_use]
pub fn path(name: &str) -> PathBuf {
    Path::new(CONFIG_DIR).join(format!("{name}.toml"))
}

/// Loads a module config. Missing fields take their defaults and the file is written back, so
/// new options show up after an update (as the Fabric mods do with their JSON configs). A file
/// that can't be written is only logged: in a container the settings may come from the
/// environment alone.
pub fn load<T: Serialize + DeserializeOwned + Default>(path: &Path) -> Result<T, String> {
    let config = match fs::read_to_string(path) {
        Ok(text) => {
            toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => T::default(),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };

    if let Err(error) = write(path, &config) {
        log::warn!("Could not write {}: {error}", path.display());
    }

    Ok(config)
}

fn write<T: Serialize>(path: &Path, config: &T) -> io::Result<()> {
    let text = toml::to_string_pretty(config).map_err(io::Error::other)?;
    if fs::read_to_string(path).is_ok_and(|current| current == text) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)
}

/// The value of environment variable `name`, unless it is unset or blank.
#[must_use]
pub fn env_value(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use std::process;

    use serde::Deserialize;

    use super::*;

    #[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
    #[serde(default)]
    struct Sample {
        enabled: bool,
        name: String,
    }

    impl Default for Sample {
        fn default() -> Self {
            Self {
                enabled: true,
                name: "main".to_owned(),
            }
        }
    }

    #[test]
    fn missing_fields_get_defaults_and_are_written_back() {
        let dir = env::temp_dir().join(format!("steel-minehouse-config-{}", process::id()));
        let file = dir.join("sample.toml");
        fs::create_dir_all(&dir).ok();
        fs::write(&file, "name = \"test\"\n").ok();

        let loaded: Sample = load(&file).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            loaded,
            Sample {
                enabled: true,
                name: "test".to_owned()
            }
        );
        let written = fs::read_to_string(&file).unwrap_or_default();
        assert!(written.contains("enabled = true"), "{written}");

        fs::remove_dir_all(&dir).ok();
    }
}
