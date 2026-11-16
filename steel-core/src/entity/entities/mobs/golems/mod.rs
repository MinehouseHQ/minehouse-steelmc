//! Golems: built by players from blocks, they defend them and villages.

mod copper_golem;
mod iron_golem;
mod shulker;
mod snow_golem;

pub use copper_golem::{CopperGolemEntity, CopperGolemState, EQUIPMENT_SLOT_ANTENNA};
pub use iron_golem::IronGolemEntity;
pub use shulker::ShulkerEntity;
pub use snow_golem::SnowGolemEntity;
