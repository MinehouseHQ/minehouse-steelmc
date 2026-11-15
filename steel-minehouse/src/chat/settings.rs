//! `config/minehouse/chat.toml`: every text and option of the chat, read as flat keys such
//! as `local.format` or `role.admin.badge`.
//!
//! A key missing from the file falls back to the bundled default, so an update that adds
//! options never breaks an old file; an empty value switches a default off.

use std::{collections::BTreeMap, fs, io, path::Path};

use toml::{Table, Value};

/// The commented default file, written on first start.
const DEFAULTS: &str = include_str!("default.toml");

/// The chat settings in effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatConfig {
    values: BTreeMap<String, String>,
}

impl ChatConfig {
    /// The bundled defaults alone.
    pub fn defaults() -> Result<Self, String> {
        let mut values = BTreeMap::new();
        merge(&mut values, DEFAULTS).map_err(|error| format!("bundled chat.toml: {error}"))?;
        Ok(Self { values })
    }

    /// The defaults with the file at `path` on top. A missing file is created from the
    /// commented defaults.
    pub fn load(path: &Path) -> Result<Self, String> {
        let mut config = Self::defaults()?;
        match fs::read_to_string(path) {
            Ok(text) => merge(&mut config.values, &text)
                .map_err(|error| format!("{}: {error}", path.display()))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if let Err(error) = write_defaults(path) {
                    log::warn!("Could not write {}: {error}", path.display());
                }
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
        Ok(config)
    }

    /// The value of `key` without surrounding spaces, `""` if there is none.
    #[must_use]
    pub fn text(&self, key: &str) -> &str {
        self.values.get(key).map_or("", |value| value.trim())
    }

    /// The whole number at `key`, or `fallback`.
    #[must_use]
    pub fn number(&self, key: &str, fallback: u32) -> u32 {
        self.text(key).parse().unwrap_or(fallback)
    }

    /// A list such as `role.order`.
    pub fn list(&self, key: &str) -> impl Iterator<Item = &str> {
        self.text(key)
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
    }
}

fn write_defaults(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, DEFAULTS)
}

fn merge(values: &mut BTreeMap<String, String>, text: &str) -> Result<(), String> {
    let table: Table = toml::from_str(text).map_err(|error| error.to_string())?;
    flatten(values, "", &table);
    Ok(())
}

/// `[role.admin] badge = "x"` becomes `role.admin.badge = x`; a list is joined with commas.
fn flatten(values: &mut BTreeMap<String, String>, prefix: &str, table: &Table) {
    for (name, value) in table {
        let key = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        match value {
            Value::Table(inner) => flatten(values, &key, inner),
            Value::String(text) => {
                values.insert(key, text.clone());
            }
            Value::Array(items) => {
                let joined = items
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .map_or_else(|| item.to_string(), str::to_owned)
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                values.insert(key, joined);
            }
            other => {
                values.insert(key, other.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse_and_read_as_flat_keys() {
        let config = ChatConfig::defaults().unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(config.number("local.radius", 0), 100);
        assert_eq!(config.text("global.prefix"), "!");
        assert_eq!(config.text("role.admin.badge"), "&c●");
        assert_eq!(
            config.list("role.order").collect::<Vec<_>>(),
            ["admin", "moderator", "vip"]
        );
        assert_eq!(config.text("world.the_nether"), "Незер");
        assert_eq!(config.text("no.such.key"), "");
        assert!(config.text("announce.format").contains('\n'));
    }

    #[test]
    fn a_file_overrides_single_keys_and_can_switch_them_off() {
        let mut config = ChatConfig::defaults().unwrap_or_else(|error| panic!("{error}"));
        let merged = merge(
            &mut config.values,
            "[local]\nradius = 30\n[role.moderator]\nbadge = \"\"\n",
        );
        assert_eq!(merged, Ok(()));
        assert_eq!(config.number("local.radius", 0), 30);
        assert_eq!(config.text("role.moderator.badge"), "");
        assert_eq!(config.text("role.moderator.hover"), "&9Модератор");
        assert_eq!(config.text("role.admin.badge"), "&c●");
        assert!(merge(&mut config.values, "local = [").is_err());
    }
}
