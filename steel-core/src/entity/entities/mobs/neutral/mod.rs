//! Neutral entity implementations.
//!
//! Those mobs are neutral to players, but will attack if provoked.

mod bee;
mod polar_bear;
mod wolf;

pub use bee::{BeeEntity, attracts_bees};
pub use polar_bear::PolarBearEntity;
pub use wolf::WolfEntity;
