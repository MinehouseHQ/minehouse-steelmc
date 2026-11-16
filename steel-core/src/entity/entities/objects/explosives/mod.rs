//! Explosive entity implementations.

mod end_crystal;
mod primed_tnt;

pub use end_crystal::EndCrystalEntity;
pub use primed_tnt::PrimedTntEntity;
pub(crate) use primed_tnt::use_portal_damage_calculator;
