//! Vanilla merchant trading: offers (`MerchantOffer`, `MerchantOffers`) and how
//! villagers and wandering traders roll them from trade sets.

mod generation;
mod offer;
mod rules;
#[cfg(test)]
mod tests;

pub use generation::{TradeContext, add_offers_from_trade_set, get_offer, profession_trade_set};
pub use offer::{ItemCost, MerchantOffer, MerchantOffers};
pub use rules::{TradeRules, set_trade_rules, trade_rules};
