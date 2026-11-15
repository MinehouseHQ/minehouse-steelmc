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
use steel_core::behavior::BlockBehaviorRegistry;
use steel_registry::{
    Registry,
    blocks::{Block, BlockRegistry},
    vanilla_blocks,
};
use steel_utils::{BlockStateId, Identifier};

use super::cloud::{self, CLOUD_BLOCK};

/// Which vanilla state each block of ours is drawn over.
const CLIENT_BLOCKS: &str = include_str!("../../pack/client_blocks.json");

/// The note block states `pack/client_blocks.json` gives out, as (our block, properties).
fn note_block_slots() -> Vec<(Identifier, Vec<(String, String)>)> {
    let Ok(file) = serde_json::from_str::<Value>(CLIENT_BLOCKS) else {
        return Vec::new();
    };
    let slots = file.get("note_block").and_then(Value::as_array);
    slots
        .into_iter()
        .flatten()
        .filter_map(|slot| {
            let block = Identifier::from_str(slot.get("block")?.as_str()?).ok()?;
            let state = slot.get("state")?.as_object()?;
            let properties = state
                .iter()
                .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())))
                .collect();
            Some((block, properties))
        })
        .collect()
}

/// The note block state `block` is drawn over.
fn note_block_slot(blocks: &BlockRegistry, block: &Block) -> Option<BlockStateId> {
    let (_, properties) = note_block_slots()
        .into_iter()
        .find(|(owner, _)| *owner == block.key)?;
    let properties: Vec<(&str, &str)> = properties
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let slot = blocks.state_id_from_block_properties(&vanilla_blocks::NOTE_BLOCK, &properties)?;
    // The default state is the one real note blocks are sent as.
    (slot != blocks.get_default_state_id(&vanilla_blocks::NOTE_BLOCK)).then_some(slot)
}

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
    let slot = note_block_slot(blocks, &CLOUD_BLOCK);
    if slot.is_none() {
        log::error!("MineHouse blocks: pack/client_blocks.json has no state for the cloud");
    }
    cloud::show_to_clients(blocks, slot);
}

/// Sets the behaviors of our blocks. Given to [`steel_core::behavior::extend_block_behaviors`].
pub(super) fn register_behaviors(behaviors: &mut BlockBehaviorRegistry) {
    behaviors.set_behavior(&CLOUD_BLOCK, Box::new(cloud::CloudBehavior));
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
        for (block, properties) in slots {
            let properties: Vec<(&str, &str)> = properties
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            assert_eq!(properties.len(), 3, "{block} names every property");
            let state = registry
                .blocks
                .state_id_from_block_properties(&vanilla_blocks::NOTE_BLOCK, &properties)
                .expect("the slot is a note block state");
            assert!(!taken.contains(&state), "{block} has a state to itself");
            taken.push(state);
            assert!(registry.blocks.by_key(&block).is_some(), "{block} is ours");
        }
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

        let slot = note_block_slot(blocks, &CLOUD_BLOCK).expect("the cloud has a slot");
        let cloud = blocks.get_base_state_id(&CLOUD_BLOCK);
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
