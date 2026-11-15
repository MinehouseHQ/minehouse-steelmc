//! Our own blocks on a vanilla client.
//!
//! A block of ours is a real block of the registry, under the id the Fabric mod gave it, so
//! a world saved by the Fabric server loads as it is. The client's game does not have it:
//! it is sent a vanilla state instead, and the server resource pack draws that state with
//! our model.
//!
//! Full blocks borrow the states of the note block, which look all the same to a client.
//! A real note block is always sent as its default state, and every other state is a slot
//! for a block of ours. `pack/client_blocks.json` says which slot is whose: this module and
//! `pack/build_pack.py` both read it, so the server and the pack cannot disagree.

use std::str::FromStr as _;

use serde_json::Value;
use steel_core::behavior::{
    BlockBehavior, BlockBehaviorRegistry, BlockItem, BlockLootContext, BlockPlaceContext,
    InteractionResult, ItemBehavior, UseOnContext,
};
use steel_registry::{
    Registry, TaggedRegistryExt as _,
    blocks::{Block, BlockRegistry},
    item_stack::ItemStack,
    vanilla_blocks,
};
use steel_utils::{BlockStateId, Identifier};

use super::{
    CustomItem, JUNCTION, STATION,
    cloud::{self, CLOUD_BLOCK},
    metro::{self, JUNCTION_BLOCK, STATION_BLOCK},
};

/// Which vanilla state each block of ours is drawn over.
const CLIENT_BLOCKS: &str = include_str!("../../pack/client_blocks.json");

/// A note block state `pack/client_blocks.json` gives to a block of ours.
struct Slot {
    /// The block of ours.
    block: Identifier,
    /// The states of the block the slot is for, as properties. None: all of them.
    when: Vec<(String, String)>,
    /// The note block state, as properties.
    state: Vec<(String, String)>,
}

/// The properties under `key` of a slot.
fn properties_of(slot: &Value, key: &str) -> Vec<(String, String)> {
    let properties = slot.get(key).and_then(Value::as_object);
    properties
        .into_iter()
        .flatten()
        .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())))
        .collect()
}

/// The note block states `pack/client_blocks.json` gives out.
fn note_block_slots() -> Vec<Slot> {
    let Ok(file) = serde_json::from_str::<Value>(CLIENT_BLOCKS) else {
        return Vec::new();
    };
    let slots = file.get("note_block").and_then(Value::as_array);
    slots
        .into_iter()
        .flatten()
        .filter_map(|slot| {
            Some(Slot {
                block: Identifier::from_str(slot.get("block")?.as_str()?).ok()?,
                when: properties_of(slot, "when"),
                state: properties_of(slot, "state"),
            })
        })
        .collect()
}

fn borrowed(properties: &[(String, String)]) -> Vec<(&str, &str)> {
    properties
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// The note block state `state` of `block` is drawn over.
fn note_block_slot(
    blocks: &BlockRegistry,
    block: &'static Block,
    state: BlockStateId,
) -> Option<BlockStateId> {
    let slot = note_block_slots().into_iter().find(|slot| {
        slot.block == block.key
            && (slot.when.is_empty()
                || blocks.state_id_from_block_properties(block, &borrowed(&slot.when))
                    == Some(state))
    })?;
    let slot = blocks
        .state_id_from_block_properties(&vanilla_blocks::NOTE_BLOCK, &borrowed(&slot.state))?;
    // The default state is the one real note blocks are sent as.
    (slot != blocks.get_default_state_id(&vanilla_blocks::NOTE_BLOCK)).then_some(slot)
}

/// Has clients see every state of the full block `block` as its note block state, or as
/// `fallback` when `pack/client_blocks.json` gives it none, and its break as `broken`:
/// a note block breaks like wood, which few of our blocks are.
pub(super) fn show_full_block(
    blocks: &mut BlockRegistry,
    block: &'static Block,
    fallback: BlockStateId,
    broken: BlockStateId,
) {
    let first = blocks.get_base_state_id(block).0;
    for state in (first..first + block.state_count()).map(BlockStateId) {
        let slot = note_block_slot(blocks, block, state);
        if slot.is_none() {
            log::error!(
                "MineHouse blocks: pack/client_blocks.json has no state for {}",
                block.key
            );
        }
        blocks.show_to_clients_as(state, slot.unwrap_or(fallback));
        blocks.show_break_to_clients_as(state, broken);
    }
}

/// A block of ours that is placed as its default state and drops its item.
struct OwnBlock {
    block: &'static Block,
    item: &'static CustomItem,
}

impl BlockBehavior for OwnBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state())
    }

    fn get_drops(
        &self,
        _state: BlockStateId,
        _context: &BlockLootContext<'_>,
    ) -> Option<Vec<ItemStack>> {
        Some(vec![self.item.stack(1)])
    }
}

/// An item of ours that places a block of ours, the way a vanilla block item does.
pub(super) struct PlacesBlock(pub &'static Block);

impl ItemBehavior for PlacesBlock {
    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        BlockItem::new(self.0).place(context.build_place_context())
    }
}

/// The blocks a pickaxe mines. The tag stays on the server: clients do not have the blocks.
const PICKAXE: &[&Block] = &[&STATION_BLOCK, &JUNCTION_BLOCK];

/// Adds our blocks to the registry and says what clients see for them. Given to
/// [`steel_registry::extend_registry`].
pub(super) fn register(registry: &mut Registry) {
    let blocks = &mut registry.blocks;

    // Every real note block goes out as one state: a client draws them all alike.
    let note_block = &vanilla_blocks::NOTE_BLOCK;
    let first = blocks.get_base_state_id(note_block).0;
    let default = blocks.get_default_state_id(note_block);
    for state in first..first + note_block.state_count() {
        blocks.show_to_clients_as(BlockStateId(state), default);
    }

    blocks.register(&CLOUD_BLOCK);
    let slot = note_block_slot(blocks, &CLOUD_BLOCK, blocks.get_base_state_id(&CLOUD_BLOCK));
    if slot.is_none() {
        log::error!("MineHouse blocks: pack/client_blocks.json has no state for the cloud");
    }
    cloud::show_to_clients(blocks, slot);

    blocks.register(&STATION_BLOCK);
    blocks.register(&JUNCTION_BLOCK);
    metro::show_to_clients(blocks);
    blocks.modify_tag(
        &Identifier::vanilla_static("mineable/pickaxe"),
        |mut keys| {
            keys.extend(PICKAXE.iter().map(|block| block.key.clone()));
            keys
        },
    );
}

/// Sets the behaviors of our blocks. Given to [`steel_core::behavior::extend_block_behaviors`].
pub(super) fn register_behaviors(behaviors: &mut BlockBehaviorRegistry) {
    behaviors.set_behavior(&CLOUD_BLOCK, Box::new(cloud::CloudBehavior));
    for (block, item) in [(&STATION_BLOCK, &STATION), (&JUNCTION_BLOCK, &JUNCTION)] {
        behaviors.set_behavior(block, Box::new(OwnBlock { block, item }));
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::RegistryExt as _;

    use super::*;

    /// A registry of its own with our blocks in it, and the number of vanilla states.
    fn extended() -> (Registry, u16) {
        let mut registry = Registry::new_vanilla();
        let vanilla_states = registry.blocks.next_state_id;
        register(&mut registry);
        (registry, vanilla_states)
    }

    #[test]
    fn the_table_lists_the_note_block_properties_of_the_game() {
        let file: Value = serde_json::from_str(CLIENT_BLOCKS).expect("client_blocks.json parses");
        let listed = file["note_block_properties"]
            .as_object()
            .expect("the properties are listed");
        let properties = vanilla_blocks::NOTE_BLOCK.properties;
        assert_eq!(listed.len(), properties.len());
        for property in properties {
            let values: Vec<&str> = (0..property.value_count())
                .map(|index| property.value_name_from_index(index))
                .collect();
            let listed: Vec<&str> = listed[property.get_name()]
                .as_array()
                .expect("a property lists its values")
                .iter()
                .filter_map(Value::as_str)
                .collect();
            assert_eq!(listed, values, "{}", property.get_name());
        }
    }

    #[test]
    fn every_slot_is_a_note_block_state_of_its_own() {
        let (registry, _) = extended();
        let slots = note_block_slots();
        assert_ne!(slots.len(), 0);
        let mut taken = vec![
            registry
                .blocks
                .get_default_state_id(&vanilla_blocks::NOTE_BLOCK),
        ];
        for slot in slots {
            let block = registry
                .blocks
                .by_key(&slot.block)
                .unwrap_or_else(|| panic!("{} is ours", slot.block));
            assert_eq!(slot.state.len(), 3, "{} names every property", slot.block);
            let state = registry
                .blocks
                .state_id_from_block_properties(&vanilla_blocks::NOTE_BLOCK, &borrowed(&slot.state))
                .expect("the slot is a note block state");
            assert!(
                !taken.contains(&state),
                "{} has a state to itself",
                slot.block
            );
            taken.push(state);
            if !slot.when.is_empty() {
                let own = registry
                    .blocks
                    .state_id_from_block_properties(block, &borrowed(&slot.when));
                assert!(own.is_some(), "{} has the state of its slot", slot.block);
            }
        }
    }

    #[test]
    fn metro_blocks_have_a_state_each_and_break_like_stone() {
        let (registry, vanilla_states) = extended();
        let blocks = &registry.blocks;
        let (shown, _) = blocks.client_states(vanilla_states);
        let shown = shown.expect("our blocks change what clients are sent");
        let broken = blocks.client_break_states(&shown, vanilla_states);
        let note_block = &vanilla_blocks::NOTE_BLOCK;
        let default = blocks.get_default_state_id(note_block);
        let first_slot = blocks.get_base_state_id(note_block).0;
        let slots = first_slot..first_slot + note_block.state_count();

        // Seventeen stations (no line and sixteen lines) and a junction, all told apart.
        let mut seen = Vec::new();
        for block in [&STATION_BLOCK, &JUNCTION_BLOCK] {
            let first = blocks.get_base_state_id(block).0;
            for state in first..first + block.state_count() {
                let sent = shown[usize::from(state)];
                assert!(slots.contains(&sent) && sent != default.0, "{}", block.key);
                assert!(!seen.contains(&sent), "{} shares a state", block.key);
                seen.push(sent);
            }
        }
        assert_eq!(seen.len(), 18);

        let quartz = blocks.get_default_state_id(&vanilla_blocks::SMOOTH_QUARTZ);
        let cut = blocks.get_default_state_id(&vanilla_blocks::WAXED_CUT_COPPER);
        let station = blocks.get_base_state_id(&STATION_BLOCK);
        let junction = blocks.get_base_state_id(&JUNCTION_BLOCK);
        assert_eq!(broken[usize::from(station.0) + 16], quartz.0);
        assert_eq!(broken[usize::from(junction.0)], cut.0);
    }

    #[test]
    fn metro_blocks_keep_the_ids_of_the_fabric_mod_and_need_a_pickaxe() {
        let (registry, _) = extended();
        let blocks = &registry.blocks;
        assert_eq!(
            STATION_BLOCK.key.to_string(),
            "minehouse-tweaks:metro_station"
        );
        assert_eq!(
            JUNCTION_BLOCK.key.to_string(),
            "minehouse-tweaks:metro_junction"
        );
        assert_eq!(STATION_BLOCK.state_count(), 17);
        assert_eq!(JUNCTION_BLOCK.state_count(), 1);
        // What a chunk saved by the Fabric server names.
        let red = blocks
            .state_id_from_properties(&STATION_BLOCK.key, &[("line", "15")])
            .expect("a saved station loads");
        assert_eq!(blocks.get_property_str(red, "line"), Some("15"));
        let default = blocks.get_default_state_id(&STATION_BLOCK);
        assert_eq!(blocks.get_property_str(default, "line"), Some("0"));

        let pickaxe = Identifier::vanilla_static("mineable/pickaxe");
        for block in PICKAXE {
            assert!(block.config.requires_correct_tool_for_drops);
            assert!(blocks.is_in_tag(block, &pickaxe), "{}", block.key);
        }
        // The tag still has what vanilla put into it.
        assert!(blocks.is_in_tag(&vanilla_blocks::STONE, &pickaxe));
        assert!(!blocks.is_in_tag(&CLOUD_BLOCK, &pickaxe));
    }

    #[test]
    fn clients_are_sent_only_states_their_game_has() {
        let (registry, vanilla_states) = extended();
        let blocks = &registry.blocks;
        let (shown, problems) = blocks.client_states(vanilla_states);
        assert_eq!(problems, Vec::<String>::new());
        let shown = shown.expect("our blocks change what clients are sent");
        let broken = blocks.client_break_states(&shown, vanilla_states);
        let sent = |state: BlockStateId| BlockStateId(shown[usize::from(state.0)]);

        let cloud = blocks.get_base_state_id(&CLOUD_BLOCK);
        let slot = note_block_slot(blocks, &CLOUD_BLOCK, cloud).expect("the cloud has a slot");
        let air = blocks.get_default_state_id(&vanilla_blocks::AIR);
        let wool = blocks.get_default_state_id(&vanilla_blocks::WHITE_WOOL);
        // Whole and melting clouds are the slot, a gone one is air.
        assert_eq!(sent(cloud), slot);
        assert_eq!(sent(BlockStateId(cloud.0 + 1)), slot);
        assert_eq!(sent(BlockStateId(cloud.0 + 2)), air);
        // A cloud breaks like wool, though it is drawn over a note block.
        assert_eq!(broken[usize::from(cloud.0)], wool.0);

        // A real note block never reaches a client as a slot, whatever its state.
        let note_block = &vanilla_blocks::NOTE_BLOCK;
        let default = blocks.get_default_state_id(note_block);
        let first = blocks.get_base_state_id(note_block).0;
        for state in first..first + note_block.state_count() {
            assert_eq!(sent(BlockStateId(state)), default);
        }
        // Everything else goes out as it is.
        let stone = blocks.get_default_state_id(&vanilla_blocks::STONE);
        assert_eq!(sent(stone), stone);
        assert_eq!(broken[usize::from(stone.0)], stone.0);
        assert!(shown.iter().all(|state| *state < vanilla_states));
    }

    #[test]
    fn a_cloud_keeps_the_id_and_states_of_the_fabric_mod() {
        let (registry, _) = extended();
        let blocks = &registry.blocks;
        assert_eq!(CLOUD_BLOCK.key.to_string(), "minehouse-tweaks:cloud");
        assert_eq!(CLOUD_BLOCK.state_count(), 3);
        // What a chunk saved by the Fabric server names.
        for fade in ["0", "1", "2"] {
            let state = blocks
                .state_id_from_properties(&CLOUD_BLOCK.key, &[("fade", fade)])
                .expect("a saved cloud loads");
            assert_eq!(blocks.get_property_str(state, "fade"), Some(fade));
        }
        let default = blocks.get_default_state_id(&CLOUD_BLOCK);
        assert_eq!(blocks.get_property_str(default, "fade"), Some("0"));
    }
}
