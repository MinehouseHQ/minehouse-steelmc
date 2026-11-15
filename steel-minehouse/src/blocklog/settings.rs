//! `config/minehouse/blocklog.toml`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The settings of the block log.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct BlockLogConfig {
    /// `false`: nothing is logged and `/co` is not there.
    pub enabled: bool,
    /// Rows older than this many days are deleted. 0 keeps everything.
    pub keep_days: u32,
    /// Lines per page of a lookup.
    pub page_size: u32,
    /// The radius of `/co near`.
    pub near_radius: i32,
    /// The widest `r:` of a lookup.
    pub max_radius: i32,
    /// World names of imported history that differ from ours, `"world_nether" = "minecraft:the_nether"`.
    pub import_worlds: BTreeMap<String, String>,
}

impl Default for BlockLogConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            keep_days: 90,
            page_size: 8,
            near_radius: 5,
            max_radius: 100,
            import_worlds: BTreeMap::new(),
        }
    }
}

impl BlockLogConfig {
    /// How long rows are kept, in milliseconds.
    #[must_use]
    pub fn keep_ms(&self) -> Option<i64> {
        (self.keep_days > 0).then(|| i64::from(self.keep_days) * 86_400_000)
    }
}
