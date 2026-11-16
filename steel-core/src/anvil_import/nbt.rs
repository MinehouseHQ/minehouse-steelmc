//! NBT helpers for vanilla save data.

use std::io::Cursor;

use simdnbt::owned::{Nbt, NbtCompound, NbtList, NbtTag, read};
use steel_registry::item_stack::ItemStack;
use steel_utils::Identifier;

use crate::command::extension_item_stack;

/// Reads a named root compound as vanilla writes it to region and `.dat` files.
pub fn read_root(bytes: &[u8]) -> Result<NbtCompound, String> {
    match read(&mut Cursor::new(bytes)) {
        Ok(Nbt::Some(root)) => {
            let mut root = root.as_compound();
            upgrade_extension_items(&mut root, &extension_item_stack);
            Ok(root)
        }
        Ok(Nbt::None) => Err("NBT root is empty".to_owned()),
        Err(error) => Err(format!("malformed NBT: {error}")),
    }
}

/// Returns the elements of a list as tags.
///
/// Vanilla writes a list whose elements have different types as a list of compounds, wrapping
/// each element that is not a compound into a compound with a single empty key.
pub fn list_elements(list: &NbtList) -> Vec<NbtTag> {
    list.as_nbt_tags()
        .into_iter()
        .map(|tag| match tag {
            NbtTag::Compound(compound) if compound.len() == 1 && compound.contains("") => compound
                .get("")
                .cloned()
                .unwrap_or(NbtTag::Compound(NbtCompound::new())),
            other => other,
        })
        .collect()
}

/// Reads a string field.
pub fn string(compound: &NbtCompound, key: &str) -> Option<String> {
    compound
        .string(key)
        .map(|value| value.to_str().into_owned())
}

/// Reads a string tag.
pub fn tag_string(tag: &NbtTag) -> Option<String> {
    tag.string().map(|value| value.to_str().into_owned())
}

/// Parses a namespaced id.
pub fn identifier(value: &str) -> Option<Identifier> {
    value.parse().ok()
}

/// Reads a byte as a boolean the way vanilla's `getBooleanOr` does.
pub fn boolean(compound: &NbtCompound, key: &str) -> bool {
    compound.byte(key).is_some_and(|value| value != 0)
}

/// Reads a list of doubles with an exact length.
pub fn doubles<const N: usize>(compound: &NbtCompound, key: &str) -> Option<[f64; N]> {
    compound.list(key)?.doubles()?.try_into().ok()
}

/// Reads a list of floats with an exact length.
pub fn floats<const N: usize>(compound: &NbtCompound, key: &str) -> Option<[f32; N]> {
    compound.list(key)?.floats()?.try_into().ok()
}

/// Number of bits needed to store `value` distinct values, `Mth.ceillog2`.
pub const fn ceil_log2(value: usize) -> u32 {
    if value <= 1 {
        0
    } else {
        (value - 1).bit_width()
    }
}

/// Unpacks vanilla `SimpleBitStorage` data: values never span two longs and fill each long
/// from the least significant bits.
pub fn unpack_bit_storage(data: &[i64], bits: u32, count: usize) -> Result<Vec<u32>, String> {
    if bits == 0 || bits > 32 {
        return Err(format!("invalid bit storage width {bits}"));
    }
    let values_per_long = (64 / bits) as usize;
    let expected = count.div_ceil(values_per_long);
    if data.len() != expected {
        return Err(format!(
            "bit storage has {} longs, expected {expected} for {count} values of {bits} bits",
            data.len()
        ));
    }
    let mask = (1u64 << bits) - 1;
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        let word = data[index / values_per_long] as u64;
        let shift = (index % values_per_long) as u32 * bits;
        values.push(((word >> shift) & mask) as u32);
    }
    Ok(values)
}

/// Serializes a compound the way Steel stores NBT payloads, or nothing for an empty compound.
pub fn compound_bytes(compound: &NbtCompound) -> Vec<u8> {
    if compound.is_empty() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    compound.write(&mut bytes);
    bytes
}

/// Removes the given keys from a compound.
pub fn without_keys(compound: &NbtCompound, keys: &[&str]) -> NbtCompound {
    let mut copy = compound.clone();
    for key in keys {
        let _ = copy.remove(key);
    }
    copy
}

/// Rewrites every item of a registry extension under `compound` the way Steel keeps it.
///
/// A modded server saved such an item under its own id (`minehouse-tweaks:magnifier`). In
/// Steel it is a vanilla item with components, which `stack_of` gives for the id. The count
/// stays, and so do the components the saved item had that the new stack does not set
/// (a custom name, what a card remembers); `minecraft:custom_data` of the two is merged.
/// Items are found wherever they are: inventories, chests, item entities, bundles.
pub fn upgrade_extension_items(
    compound: &mut NbtCompound,
    stack_of: &dyn Fn(&str) -> Option<ItemStack>,
) {
    for tag in compound.values_mut() {
        upgrade_tag(tag, stack_of);
    }
    let (Some(id), Some(count)) = (compound.string("id"), compound.int("count")) else {
        return;
    };
    // No vanilla id has a namespace other than `minecraft`, and most ids are vanilla.
    let id = id.to_str();
    if id.starts_with("minecraft:") || !id.contains(':') {
        return;
    }
    let Some(mut stack) = stack_of(&id) else {
        return;
    };
    stack.set_count(count);
    let NbtTag::Compound(mut upgraded) = stack.to_nbt_tag_ref() else {
        return;
    };
    let saved = compound.remove("components");
    if let Some(NbtTag::Compound(saved)) = saved {
        let mut components = match upgraded.remove("components") {
            Some(NbtTag::Compound(components)) => components,
            _ => NbtCompound::new(),
        };
        for (key, value) in saved {
            let key = key.to_string();
            match (components.compound_mut(&key), value) {
                (Some(ours), NbtTag::Compound(theirs)) if key == "minecraft:custom_data" => {
                    for (name, tag) in theirs {
                        let name = name.to_string();
                        if !ours.contains(&name) {
                            ours.insert(name, tag);
                        }
                    }
                }
                (_, value) => {
                    if !components.contains(&key) {
                        components.insert(key, value);
                    }
                }
            }
        }
        upgraded.insert("components", components);
    }
    for key in ["id", "count", "components"] {
        if let Some(tag) = upgraded.remove(key) {
            let _ = compound.remove(key);
            compound.insert(key, tag);
        }
    }
}

fn upgrade_tag(tag: &mut NbtTag, stack_of: &dyn Fn(&str) -> Option<ItemStack>) {
    match tag {
        NbtTag::Compound(compound) => upgrade_extension_items(compound, stack_of),
        NbtTag::List(list) => upgrade_list(list, stack_of),
        _ => {}
    }
}

fn upgrade_list(list: &mut NbtList, stack_of: &dyn Fn(&str) -> Option<ItemStack>) {
    match list {
        NbtList::Compound(compounds) => {
            for compound in compounds {
                upgrade_extension_items(compound, stack_of);
            }
        }
        NbtList::List(lists) => {
            for list in lists {
                upgrade_list(list, stack_of);
            }
        }
        _ => {}
    }
}
