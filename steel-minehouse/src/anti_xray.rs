//! Anti-xray settings, `config/minehouse/antixray.toml`: which ores each dimension sends to
//! clients as rock while nothing around them lets light through. The hiding itself is in
//! [`steel_core::chunk::anti_xray`].

use std::collections::BTreeMap;

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use steel_core::chunk::anti_xray::{self, HiddenBlocks};
use steel_utils::Identifier;

use crate::config;

/// The anti-xray settings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AntiXrayConfig {
    /// `false`: clients see every block as it is.
    pub enabled: bool,
    /// Per dimension: hidden block = what clients see instead.
    pub worlds: BTreeMap<String, BTreeMap<String, String>>,
}

fn pairs(rock: &str, blocks: &[&str]) -> BTreeMap<String, String> {
    blocks
        .iter()
        .map(|block| (format!("minecraft:{block}"), format!("minecraft:{rock}")))
        .collect()
}

impl Default for AntiXrayConfig {
    fn default() -> Self {
        let mut overworld = pairs(
            "stone",
            &[
                "coal_ore",
                "copper_ore",
                "iron_ore",
                "gold_ore",
                "redstone_ore",
                "lapis_ore",
                "diamond_ore",
                "emerald_ore",
            ],
        );
        overworld.extend(pairs(
            "deepslate",
            &[
                "deepslate_coal_ore",
                "deepslate_copper_ore",
                "deepslate_iron_ore",
                "deepslate_gold_ore",
                "deepslate_redstone_ore",
                "deepslate_lapis_ore",
                "deepslate_diamond_ore",
                "deepslate_emerald_ore",
            ],
        ));
        let nether = pairs(
            "netherrack",
            &["nether_gold_ore", "nether_quartz_ore", "ancient_debris"],
        );
        Self {
            enabled: true,
            worlds: BTreeMap::from([
                ("minecraft:overworld".to_owned(), overworld),
                ("minecraft:the_nether".to_owned(), nether),
            ]),
        }
    }
}

fn identifier(text: &str) -> Result<Identifier, String> {
    text.parse::<Identifier>()
        .map_err(|_| format!("not a block id: {text}"))
}

/// Loads the settings and turns anti-xray on.
pub fn start() {
    let config: AntiXrayConfig = match config::load(&config::path("antixray")) {
        Ok(config) => config,
        Err(error) => {
            log::error!("MineHouse anti-xray is off: {error}");
            return;
        }
    };
    if !config.enabled {
        log::info!("MineHouse anti-xray is off in config/minehouse/antixray.toml");
        return;
    }
    let mut worlds = FxHashMap::default();
    for (world, blocks) in &config.worlds {
        let resolved: Result<Vec<_>, String> = blocks
            .iter()
            .map(|(hidden, shown)| Ok((identifier(hidden)?, identifier(shown)?)))
            .collect();
        match resolved.and_then(|pairs| HiddenBlocks::from_keys(&pairs)) {
            Ok(hidden) => match identifier(world) {
                Ok(world) => {
                    worlds.insert(world, hidden);
                }
                Err(error) => log::error!("MineHouse anti-xray: {error}"),
            },
            Err(error) => log::error!("MineHouse anti-xray skips {world}: {error}"),
        }
    }
    let count = worlds.len();
    if anti_xray::install(worlds) {
        log::info!("MineHouse anti-xray on in {count} dimensions");
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use super::*;

    #[test]
    fn every_default_block_is_known() {
        init_vanilla_registry();
        for blocks in AntiXrayConfig::default().worlds.values() {
            let pairs: Vec<_> = blocks
                .iter()
                .map(|(hidden, shown)| {
                    (
                        identifier(hidden).expect("block id"),
                        identifier(shown).expect("block id"),
                    )
                })
                .collect();
            HiddenBlocks::from_keys(&pairs).expect("vanilla blocks");
        }
    }
}
