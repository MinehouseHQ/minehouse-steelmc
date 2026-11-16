//! Server-wide changes to villager trading. Everything is off by default, which is vanilla; an
//! extension switches the rules on with [`set_trade_rules`].

use std::sync::atomic::{AtomicBool, Ordering};

/// How trading departs from vanilla. The default changes nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TradeRules {
    /// Nothing lowers a price: a cured zombie villager, Hero of the Village and a good
    /// reputation are ignored. A bad reputation still raises it.
    pub no_discounts: bool,
    /// A trade that keeps selling out does not get more expensive.
    pub no_demand: bool,
    /// A villager's offer never runs out. A wandering trader's still does.
    pub infinite_trades: bool,
}

static NO_DISCOUNTS: AtomicBool = AtomicBool::new(false);
static NO_DEMAND: AtomicBool = AtomicBool::new(false);
static INFINITE_TRADES: AtomicBool = AtomicBool::new(false);

/// Replaces the trade rules. Takes effect at once; an open trading screen picks them up the
/// next time its offers are sent.
pub fn set_trade_rules(rules: TradeRules) {
    NO_DISCOUNTS.store(rules.no_discounts, Ordering::Relaxed);
    NO_DEMAND.store(rules.no_demand, Ordering::Relaxed);
    INFINITE_TRADES.store(rules.infinite_trades, Ordering::Relaxed);
}

/// The trade rules in force.
#[must_use]
pub fn trade_rules() -> TradeRules {
    TradeRules {
        no_discounts: NO_DISCOUNTS.load(Ordering::Relaxed),
        no_demand: NO_DEMAND.load(Ordering::Relaxed),
        infinite_trades: INFINITE_TRADES.load(Ordering::Relaxed),
    }
}

impl TradeRules {
    /// The special price as it counts: with discounts off it can only raise the price.
    #[must_use]
    pub const fn special_price_diff(self, special_price_diff: i32) -> i32 {
        if self.no_discounts && special_price_diff < 0 {
            0
        } else {
            special_price_diff
        }
    }

    /// The demand as it counts.
    #[must_use]
    pub const fn demand(self, demand: i32) -> i32 {
        if self.no_demand { 0 } else { demand }
    }

    /// Vanilla `MerchantOffer.getModifiedCostCount`, minus the parts switched off.
    #[must_use]
    pub fn cost(
        self,
        base_price: i32,
        demand: i32,
        price_multiplier: f32,
        special_price_diff: i32,
        max_stack_size: i32,
    ) -> i32 {
        // Vanilla `Mth.floor(float)`.
        let demand_diff = (base_price.wrapping_mul(self.demand(demand)) as f32 * price_multiplier)
            .floor()
            .max(0.0) as i32;
        (base_price + demand_diff + self.special_price_diff(special_price_diff))
            .clamp(1, max_stack_size.max(1))
    }

    /// Vanilla `MerchantOffer.updateDemand`. An offer without a limit counts the uses past
    /// its vanilla limit as one full buy-out, so the price climbs as in vanilla and not
    /// without bound.
    #[must_use]
    pub const fn demand_after_restock(
        self,
        demand: i32,
        uses: i32,
        max_uses: i32,
        keeps_limit: bool,
    ) -> i32 {
        if self.no_demand {
            return 0;
        }
        let counted = if self.unlimited(keeps_limit) && uses > max_uses {
            max_uses
        } else {
            uses
        };
        demand + counted - (max_uses - counted)
    }

    /// Whether an offer never runs out. `keeps_limit` marks a wandering trader's offer.
    #[must_use]
    pub const fn unlimited(self, keeps_limit: bool) -> bool {
        self.infinite_trades && !keeps_limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VANILLA: TradeRules = TradeRules {
        no_discounts: false,
        no_demand: false,
        infinite_trades: false,
    };
    const MINEHOUSE: TradeRules = TradeRules {
        no_discounts: true,
        no_demand: false,
        infinite_trades: true,
    };

    #[test]
    fn the_default_is_vanilla() {
        assert_eq!(TradeRules::default(), VANILLA);
        // A cured villager: 10 emeralds down to 1; demand adds on top.
        assert_eq!(VANILLA.cost(10, 0, 0.05, -20, 64), 1);
        assert_eq!(VANILLA.cost(10, 8, 0.05, 0, 64), 14);
        assert_eq!(VANILLA.demand_after_restock(0, 12, 12, false), 12);
        assert_eq!(VANILLA.demand_after_restock(0, 0, 12, false), -12);
        assert!(!VANILLA.unlimited(false));
    }

    #[test]
    fn without_discounts_a_price_only_goes_up() {
        assert_eq!(MINEHOUSE.cost(10, 0, 0.05, -20, 64), 10);
        assert_eq!(MINEHOUSE.cost(10, 0, 0.05, 6, 64), 16);
        assert_eq!(MINEHOUSE.cost(10, 8, 0.05, -3, 64), 14);
        assert_eq!(MINEHOUSE.cost(60, 0, 0.05, 20, 64), 64);
        assert_eq!(MINEHOUSE.special_price_diff(-5), 0);
        assert_eq!(MINEHOUSE.special_price_diff(5), 5);
    }

    #[test]
    fn negative_demand_never_lowers_a_price() {
        assert_eq!(VANILLA.cost(10, -8, 0.05, 0, 64), 10);
    }

    #[test]
    fn demand_can_be_switched_off() {
        let rules = TradeRules {
            no_demand: true,
            ..VANILLA
        };
        assert_eq!(rules.cost(10, 8, 0.05, 0, 64), 10);
        assert_eq!(rules.demand(8), 0);
        assert_eq!(rules.demand_after_restock(5, 12, 12, false), 0);
    }

    #[test]
    fn uses_past_the_limit_count_as_one_buy_out() {
        assert!(MINEHOUSE.unlimited(false));
        assert_eq!(MINEHOUSE.demand_after_restock(0, 500, 12, false), 12);
        assert_eq!(MINEHOUSE.demand_after_restock(0, 3, 12, false), -6);
    }

    #[test]
    fn a_wandering_trader_keeps_its_limit() {
        assert!(!MINEHOUSE.unlimited(true));
        assert_eq!(MINEHOUSE.demand_after_restock(0, 20, 12, true), 28);
    }
}
