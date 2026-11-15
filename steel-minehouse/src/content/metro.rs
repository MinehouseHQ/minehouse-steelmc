//! The blocks of the metro: the station block, which goes under a rail, and the junction
//! block of a switch. A port of the Fabric `StationBlock` and `JunctionBlock` as blocks:
//! they can be crafted, placed and mined, and a saved world keeps them. What they do for
//! trains is not ported yet.

use steel_registry::{
    blocks::{Block, BlockRegistry, StateByteData, behavior::BlockConfig, properties::IntProperty},
    sound_types, vanilla_blocks,
};
use steel_utils::Identifier;

use super::blocks::show_full_block;

/// The line of a station: 0 none, else the id of the dye of the line plus one. It picks
/// the model: the sign of the station is in the color of its line.
pub static LINE: IntProperty = IntProperty::new("line", 0, 16);

/// Vanilla `MapColor.COLOR_ORANGE`.
const MAP_COLOR_ORANGE: u8 = 15;

/// As hard as copper, and mined with a pickaxe like it.
const fn config() -> BlockConfig {
    BlockConfig::new()
        .destroy_time(3.0)
        .explosion_resistance(6.0)
        .requires_correct_tool_for_drops(true)
        .sound_type(sound_types::COPPER)
}

/// «Станция метро».
pub static STATION_BLOCK: Block = Block::new(
    Identifier::new_static("minehouse-tweaks", "metro_station"),
    config(),
    &[&LINE],
)
.with_map_color(StateByteData::new(MAP_COLOR_ORANGE, &[]));

/// «Стрелка метро».
pub static JUNCTION_BLOCK: Block = Block::new(
    Identifier::new_static("minehouse-tweaks", "metro_junction"),
    config(),
    &[],
)
.with_map_color(StateByteData::new(MAP_COLOR_ORANGE, &[]));

/// Has clients see the two blocks as their note block states. Without a state of its own
/// a station is waxed chiseled copper and a junction is smooth quartz, as on the Fabric
/// server for a player without the pack. They break like stone, not like the wood of a
/// note block: a station as smooth quartz, a junction as waxed cut copper.
pub(super) fn show_to_clients(blocks: &mut BlockRegistry) {
    let quartz = blocks.get_default_state_id(&vanilla_blocks::SMOOTH_QUARTZ);
    let chiseled = blocks.get_default_state_id(&vanilla_blocks::WAXED_CHISELED_COPPER);
    let cut = blocks.get_default_state_id(&vanilla_blocks::WAXED_CUT_COPPER);
    show_full_block(blocks, &STATION_BLOCK, chiseled, quartz);
    show_full_block(blocks, &JUNCTION_BLOCK, quartz, cut);
}
