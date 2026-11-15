//! The blocks of the metro: the station block, which goes under a rail, and the junction
//! block of a switch. A port of the Fabric `StationBlock` and `JunctionBlock` as blocks:
//! they can be crafted, placed and mined, and a saved world keeps them. A station takes its
//! name from a name tag and its line from a dye, and [`super::stations`] remembers both.
//! What the blocks do for trains is not ported yet.

use std::{ptr, sync::Arc};

use steel_core::{
    behavior::{
        BlockBehavior, BlockLootContext, BlockPlaceContext, InteractionResult, InventoryAccess,
    },
    player::Player,
    world::World,
};
use steel_protocol::packets::game::SoundSource;
use steel_registry::{
    blocks::{
        Block, BlockRegistry, StateByteData, behavior::BlockConfig,
        block_state_ext::BlockStateExt as _, properties::IntProperty,
    },
    data_components::vanilla_components::{CUSTOM_NAME, DYE},
    dye_color::DyeColor,
    item_stack::ItemStack,
    items::item::BlockHitResult,
    sound_events, sound_types, vanilla_blocks, vanilla_items,
};
use steel_utils::{
    BlockPos, BlockStateId, Identifier,
    types::{InteractionHand, UpdateFlags},
};
use text_components::{Modifier as _, TextComponent, format::Color, resolving::NoResolutor};

use super::{STATION, blocks::show_full_block, id_of, stations};

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

/// The name of a line of the metro, as an adjective for «линия».
#[must_use]
pub const fn line_name(line: DyeColor) -> &'static str {
    match line {
        DyeColor::White => "Белая",
        DyeColor::LightGray => "Светло-серая",
        DyeColor::Gray => "Серая",
        DyeColor::Black => "Чёрная",
        DyeColor::Brown => "Коричневая",
        DyeColor::Red => "Красная",
        DyeColor::Orange => "Оранжевая",
        DyeColor::Yellow => "Жёлтая",
        DyeColor::Lime => "Салатовая",
        DyeColor::Green => "Зелёная",
        DyeColor::Cyan => "Бирюзовая",
        DyeColor::LightBlue => "Голубая",
        DyeColor::Blue => "Синяя",
        DyeColor::Purple => "Фиолетовая",
        DyeColor::Magenta => "Пурпурная",
        DyeColor::Pink => "Розовая",
    }
}

/// The value of [`LINE`] for a line.
fn line_value(line: Option<DyeColor>) -> u8 {
    line.map_or(0, |line| {
        u8::try_from(line.id()).map_or(0, |id| id.saturating_add(1))
    })
}

/// What a click on a station says about it.
fn describe(station: &stations::Station) -> String {
    let name = station.name.as_ref().map_or_else(
        || "Станция без названия. Назови её биркой (ПКМ)".to_owned(),
        |name| format!("Станция «{name}»"),
    );
    let line = station.line.map_or_else(
        || " · линию задаёт краситель".to_owned(),
        |line| format!(" · {} линия", line_name(line)),
    );
    name + &line
}

/// Tells the player above the hotbar, as the Fabric `Trains.tell` does.
fn tell(player: &Player, text: String) {
    player.send_overlay_message(&TextComponent::plain(text).color(Color::Yellow));
}

fn is_station(state: BlockStateId) -> bool {
    ptr::eq(state.get_block(), &raw const STATION_BLOCK)
}

/// Makes the block show the line the metro has for it.
fn sync(world: &Arc<World>, pos: BlockPos) {
    let state = world.get_block_state(pos);
    if !is_station(state) {
        return;
    }
    let line = stations::with(|stations| stations.get(&world.key, pos).and_then(|s| s.line));
    let want = line_value(line);
    if state.get_value(&LINE) != want {
        world.set_block(
            pos,
            state.set_value(&LINE, want),
            UpdateFlags::UPDATE_CLIENTS,
        );
    }
}

/// The station block: named by a name tag, put on a line by a dye.
pub(super) struct Station;

impl Station {
    fn dye(
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        color: DyeColor,
        inv: &mut InventoryAccess,
    ) -> InteractionResult {
        let changed = stations::with(|stations| {
            if stations.get(&world.key, pos).and_then(|s| s.line) == Some(color) {
                return false;
            }
            stations.set_line(&world.key, pos, color);
            true
        });
        if !changed {
            tell(
                player,
                format!("Станция уже на линии «{}»", line_name(color)),
            );
            return InteractionResult::Success;
        }
        sync(world, pos);
        if !player.has_infinite_materials() {
            inv.with_item(ItemStack::shrink_one);
        }
        world.play_sound(
            &sound_events::ITEM_DYE_USE,
            SoundSource::Blocks,
            pos,
            1.0,
            1.0,
            None,
        );
        tell(player, format!("{} линия", line_name(color)));
        InteractionResult::Success
    }
}

impl BlockBehavior for Station {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(STATION_BLOCK.default_state())
    }

    fn get_drops(
        &self,
        _state: BlockStateId,
        _context: &BlockLootContext<'_>,
    ) -> Option<Vec<ItemStack>> {
        Some(vec![STATION.stack(1)])
    }

    fn use_item_on(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hand: InteractionHand,
        _hit_result: &BlockHitResult,
        inv: &mut InventoryAccess,
    ) -> InteractionResult {
        let held = inv.with_item(|stack| stack.clone());
        // A dye or a name tag of the game, not an item of ours that is one to the client.
        if id_of(&held).is_some() {
            return InteractionResult::TryEmptyHandInteraction;
        }
        if let Some(color) = held.get(DYE).copied() {
            return Self::dye(world, pos, player, color, inv);
        }
        let name = held
            .is(&vanilla_items::NAME_TAG)
            .then(|| held.get(CUSTOM_NAME))
            .flatten()
            .map(|name| stations::clean_name(&name.to_plain(&NoResolutor)));
        let Some(name) = name else {
            return InteractionResult::TryEmptyHandInteraction;
        };
        stations::with(|stations| stations.set_name(&world.key, pos, name.clone()));
        if !player.has_infinite_materials() {
            inv.with_item(ItemStack::shrink_one);
        }
        world.play_sound(
            &sound_events::BLOCK_NOTE_BLOCK_BELL,
            SoundSource::Blocks,
            pos,
            1.0,
            1.2,
            None,
        );
        tell(player, format!("Станция «{name}»"));
        InteractionResult::Success
    }

    fn use_without_item(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        let station = stations::with(|stations| {
            stations.add(&world.key, pos);
            stations.get(&world.key, pos).cloned().unwrap_or_default()
        });
        sync(world, pos);
        tell(player, describe(&station));
        InteractionResult::Success
    }

    fn on_place(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        old_state: BlockStateId,
        _moved_by_piston: bool,
    ) {
        if !is_station(old_state) {
            stations::with(|stations| stations.add(&world.key, pos));
        }
    }

    fn affect_neighbors_after_removal(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _moved_by_piston: bool,
    ) {
        stations::with(|stations| stations.remove(&world.key, pos));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_the_dye_id_plus_one() {
        assert_eq!(line_value(None), 0);
        assert_eq!(line_value(Some(DyeColor::White)), 1);
        assert_eq!(line_value(Some(DyeColor::Black)), 16);
    }

    #[test]
    fn a_click_tells_the_name_and_the_line() {
        let mut station = stations::Station::default();
        assert_eq!(
            describe(&station),
            "Станция без названия. Назови её биркой (ПКМ) · линию задаёт краситель"
        );
        station.name = Some("Спавн".to_owned());
        station.line = Some(DyeColor::Red);
        assert_eq!(describe(&station), "Станция «Спавн» · Красная линия");
    }
}
