//! Villager prices and trade limits: what the mod `MineHouse` Villagers did on the Fabric
//! server. Villagers give no discounts and their trades never run out; the trade screen
//! shows the real price with no client mod.
//!
//! The work is done by the trade rules of Steel
//! (`steel-core/src/entity/entities/mobs/villagers/trading/rules.rs`); this module only
//! switches them on from `config/minehouse/villagers.toml`.

use serde::{Deserialize, Serialize};
use steel_core::entity::entities::mobs::villagers::trading::{TradeRules, set_trade_rules};

use crate::config;

/// `config/minehouse/villagers.toml`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct VillagersConfig {
    /// No discounts for a cured zombie villager, Hero of the Village or a good reputation.
    /// The markup for a bad reputation (hitting or killing a villager) stays.
    pub discounts_disabled: bool,
    /// No price rise for a trade that keeps selling out.
    pub demand_disabled: bool,
    /// A villager's trade never locks. Buying past the vanilla limit counts as one full
    /// buy-out for the demand. The wandering trader keeps its limits.
    pub infinite_trades: bool,
}

impl Default for VillagersConfig {
    fn default() -> Self {
        Self {
            discounts_disabled: true,
            demand_disabled: false,
            infinite_trades: true,
        }
    }
}

impl VillagersConfig {
    /// The trade rules these settings ask for.
    #[must_use]
    pub const fn rules(self) -> TradeRules {
        TradeRules {
            no_discounts: self.discounts_disabled,
            no_demand: self.demand_disabled,
            infinite_trades: self.infinite_trades,
        }
    }
}

/// Reads the settings and puts the trade rules in force. A broken file leaves the defaults
/// (no discounts, vanilla demand, unlimited trades).
pub fn start() {
    let config =
        config::load::<VillagersConfig>(&config::path("villagers")).unwrap_or_else(|error| {
            log::error!("Villagers: {error}; using the defaults");
            VillagersConfig::default()
        });
    set_trade_rules(config.rules());
    log::info!(
        "Villagers: discounts {}, demand {}, trades {}",
        if config.discounts_disabled {
            "off"
        } else {
            "vanilla"
        },
        if config.demand_disabled {
            "off"
        } else {
            "vanilla"
        },
        if config.infinite_trades {
            "unlimited"
        } else {
            "vanilla"
        },
    );
}

#[cfg(test)]
mod tests {
    use steel_core::entity::entities::mobs::villagers::trading::{ItemCost, MerchantOffer};
    use steel_registry::{data_components::DataComponentMap, item_stack::ItemStack, vanilla_items};

    use super::*;

    #[test]
    fn defaults_match_the_fabric_server() {
        let rules = VillagersConfig::default().rules();
        assert!(rules.no_discounts);
        assert!(!rules.no_demand);
        assert!(rules.infinite_trades);
    }

    /// The only test that puts rules in force: they are global to the process.
    #[test]
    fn offers_follow_the_rules_in_force() {
        let emeralds =
            |count| ItemCost::new(&vanilla_items::EMERALD, count, DataComponentMap::new());
        let bread = ItemStack::with_count(&vanilla_items::BREAD, 6);
        let mut offer = MerchantOffer::new(emeralds(10), None, bread, 2, 1, 0.05);
        offer.set_special_price_diff(-9);
        for _ in 0..5 {
            offer.increase_uses();
        }
        assert_eq!(offer.cost_a().count(), 1);
        assert!(offer.is_out_of_stock());

        set_trade_rules(VillagersConfig::default().rules());
        assert_eq!(offer.cost_a().count(), 10);
        assert!(!offer.is_out_of_stock());
        // A wandering trader's offer keeps its limit.
        offer.set_keeps_limit(true);
        assert!(offer.is_out_of_stock());
        offer.set_keeps_limit(false);
        // Five uses of two count as one buy-out: demand 2, so 10 + 10 * 2 * 0.05 = 11.
        offer.update_demand();
        assert_eq!(offer.demand(), 2);
        assert_eq!(offer.cost_a().count(), 11);

        set_trade_rules(TradeRules::default());
        assert_eq!(offer.cost_a().count(), 2);
    }

    #[test]
    fn a_partial_file_keeps_the_other_defaults() {
        let config: VillagersConfig =
            toml::from_str("infinite_trades = false").expect("valid toml");
        assert!(config.discounts_disabled);
        assert!(!config.infinite_trades);
    }
}
