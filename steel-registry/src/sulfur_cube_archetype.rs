//! `minecraft:sulfur_cube_archetype` registry entries.
//!
//! Vanilla `SulfurCubeArchetype`: what a sulfur cube becomes after it
//! swallows a block, chosen by the block's item tag. Entries are built from
//! datapack JSON at build time and synced to the client as NBT.

use std::str::FromStr;

use rustc_hash::FxHashMap;
use serde::{Deserialize, Deserializer, de::Error as _};
use simdnbt::ToNbtTag;
use simdnbt::owned::NbtTag;
use steel_utils::Identifier;
use steel_utils::nbt::json_to_nbt;
use steel_utils::value_providers::FloatProvider;

use crate::attribute::AttributeModifierOperation;
use crate::items::Item;
use crate::{REGISTRY, TaggedRegistryExt};

/// A sulfur cube archetype, vanilla `SulfurCubeArchetype`.
///
/// `PartialEq`/`Eq` come from [`crate::impl_registry_entry!`] (identity by key).
#[derive(Debug)]
pub struct SulfurCubeArchetype {
    /// Registry key.
    pub key: Identifier,
    /// The decoded entry.
    pub data: SulfurCubeArchetypeData,
    /// The source JSON, re-encoded as NBT for registry sync.
    pub json: &'static str,
}

impl SulfurCubeArchetype {
    /// Decodes an entry from its datapack JSON.
    ///
    /// # Panics
    /// When the JSON doesn't match vanilla's codec; entries are build-time
    /// vanilla data checked by the build script.
    #[must_use]
    pub fn from_json(key: Identifier, json: &'static str) -> Self {
        let data = serde_json::from_str(json)
            .unwrap_or_else(|error| panic!("invalid sulfur cube archetype {key}: {error}"));
        Self { key, data, json }
    }

    /// Whether `item` is one of the blocks that give a cube this archetype.
    #[must_use]
    pub fn matches_item(&self, item: &'static Item) -> bool {
        match &self.data.items {
            ItemSet::Tag(tag) => REGISTRY.items.is_in_tag(item, tag),
            ItemSet::Direct(keys) => keys.contains(&item.key),
        }
    }
}

/// The fields of vanilla `SulfurCubeArchetype`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SulfurCubeArchetypeData {
    /// Items that give a cube this archetype.
    pub items: ItemSet,
    /// Attribute modifiers added while the block is inside.
    pub attribute_modifiers: Vec<ArchetypeAttributeModifier>,
    /// Whether the cube floats up in liquids.
    #[serde(default)]
    pub buoyant: bool,
    /// How the cube explodes, if it can.
    pub explosion: Option<ExplosionData>,
    /// Damage the cube deals to what it touches.
    pub contact_damage: Option<ContactDamage>,
    /// Knockback the cube takes when hit.
    pub knockback_modifiers: KnockbackModifiers,
    /// Hit and push sounds.
    pub sound_settings: SoundSettings,
}

/// A `HolderSet<Item>`: a tag or a list of items.
#[derive(Debug, Clone)]
pub enum ItemSet {
    /// `#namespace:path`.
    Tag(Identifier),
    /// One item or a list of items.
    Direct(Vec<Identifier>),
}

impl<'de> Deserialize<'de> for ItemSet {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            One(String),
            Many(Vec<String>),
        }
        let parse = |value: &str| Identifier::from_str(value).map_err(D::Error::custom);
        match Raw::deserialize(deserializer)? {
            Raw::One(value) => match value.strip_prefix('#') {
                Some(tag) => Ok(Self::Tag(parse(tag)?)),
                None => Ok(Self::Direct(vec![parse(&value)?])),
            },
            Raw::Many(values) => Ok(Self::Direct(
                values
                    .iter()
                    .map(|value| parse(value))
                    .collect::<Result<_, _>>()?,
            )),
        }
    }
}

/// Vanilla `SulfurCubeArchetype.AttributeEntry`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchetypeAttributeModifier {
    /// The attribute changed.
    pub attribute: Identifier,
    /// The modifier id.
    pub id: Identifier,
    /// The modifier amount.
    pub amount: f64,
    /// How the amount applies.
    #[serde(deserialize_with = "deserialize_operation")]
    pub operation: AttributeModifierOperation,
}

fn deserialize_operation<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<AttributeModifierOperation, D::Error> {
    let name = String::deserialize(deserializer)?;
    AttributeModifierOperation::by_name(&name)
        .ok_or_else(|| D::Error::custom(format!("unknown attribute operation {name}")))
}

/// Vanilla `SulfurCubeArchetype.ExplosionData`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplosionData {
    /// Explosion power.
    pub power: i32,
    /// Whether the explosion lights fires.
    pub causes_fire: bool,
    /// Fuse length in ticks.
    pub fuse: i32,
}

/// Vanilla `SulfurCubeArchetype.ContactDamage`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContactDamage {
    /// The damage type dealt.
    pub damage_type: Identifier,
    /// How much damage.
    pub amount: FloatProvider,
    /// Whether the cube is the damage's source for non-players too.
    pub attribute_to_source: bool,
}

/// Vanilla `SulfurCubeArchetype.KnockbackModifiers`.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KnockbackModifiers {
    /// Horizontal knockback power.
    pub horizontal_power: f32,
    /// Vertical knockback power.
    pub vertical_power: f32,
}

/// Vanilla `DEFAULT_KNOCKBACK_MODIFIERS`.
pub const DEFAULT_KNOCKBACK_MODIFIERS: KnockbackModifiers = KnockbackModifiers {
    horizontal_power: 0.33,
    vertical_power: 0.06,
};

/// A sound reference: a registered sound id, or an inline sound.
#[derive(Debug, Clone, PartialEq)]
pub struct ArchetypeSound {
    /// The sound id.
    pub sound_id: Identifier,
    /// A fixed range for an inline sound.
    pub fixed_range: Option<f32>,
}

impl<'de> Deserialize<'de> for ArchetypeSound {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Id(Identifier),
            Direct {
                sound_id: Identifier,
                range: Option<f32>,
            },
        }
        Ok(match Raw::deserialize(deserializer)? {
            Raw::Id(sound_id) => Self {
                sound_id,
                fixed_range: None,
            },
            Raw::Direct { sound_id, range } => Self {
                sound_id,
                fixed_range: range,
            },
        })
    }
}

/// Vanilla `SulfurCubeArchetype.SoundSettings`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SoundSettings {
    /// Played when the cube is hit.
    pub hit_sound: ArchetypeSound,
    /// Played when a player pushes the cube.
    pub push_sound: ArchetypeSound,
    /// Push speed needed for the push sound.
    pub push_sound_impulse_threshold: f32,
    /// Seconds between push sounds.
    pub push_sound_cooldown: f32,
}

impl SoundSettings {
    /// Vanilla `DEFAULT_SOUND_SETTINGS`.
    #[must_use]
    pub const fn vanilla_default() -> Self {
        Self {
            hit_sound: ArchetypeSound {
                sound_id: Identifier::vanilla_static("entity.sulfur_cube.regular.hit"),
                fixed_range: None,
            },
            push_sound: ArchetypeSound {
                sound_id: Identifier::vanilla_static("entity.sulfur_cube.regular.push"),
                fixed_range: None,
            },
            push_sound_impulse_threshold: 0.2,
            push_sound_cooldown: 0.5,
        }
    }
}

/// Registry-sync NBT: the entry's datapack JSON in NBT form.
impl ToNbtTag for &SulfurCubeArchetype {
    fn to_nbt_tag(self) -> NbtTag {
        let value: serde_json::Value = serde_json::from_str(self.json)
            .unwrap_or_else(|error| panic!("invalid sulfur cube archetype {}: {error}", self.key));
        json_to_nbt(&value)
            .unwrap_or_else(|| panic!("sulfur cube archetype {} has no NBT form", self.key))
    }
}

/// A reference to a registered archetype.
pub type SulfurCubeArchetypeRef = &'static SulfurCubeArchetype;

/// The `minecraft:sulfur_cube_archetype` registry.
pub struct SulfurCubeArchetypeRegistry {
    entries_by_id: Vec<SulfurCubeArchetypeRef>,
    entries_by_key: FxHashMap<Identifier, usize>,
    allows_registering: bool,
}

impl SulfurCubeArchetypeRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries_by_id: Vec::new(),
            entries_by_key: FxHashMap::default(),
            allows_registering: true,
        }
    }
}

crate::impl_standard_methods!(
    SulfurCubeArchetypeRegistry,
    SulfurCubeArchetypeRef,
    entries_by_id,
    entries_by_key,
    allows_registering
);

crate::impl_registry!(
    SulfurCubeArchetypeRegistry,
    SulfurCubeArchetype,
    entries_by_id,
    entries_by_key,
    sulfur_cube_archetypes
);

#[cfg(test)]
mod tests {
    use simdnbt::ToNbtTag;

    use crate::{
        REGISTRY, RegistryExt, init_vanilla_registry, vanilla_items, vanilla_sulfur_cube_archetypes,
    };

    #[test]
    fn every_vanilla_archetype_decodes_and_has_nbt() {
        init_vanilla_registry();
        assert_eq!(REGISTRY.sulfur_cube_archetypes.len(), 12);
        for (_, archetype) in REGISTRY.sulfur_cube_archetypes.iter() {
            assert!(archetype.to_nbt_tag().compound().is_some());
        }
    }

    #[test]
    fn explosive_archetype_reads_its_fields() {
        let explosive = &*vanilla_sulfur_cube_archetypes::EXPLOSIVE;
        let explosion = explosive
            .data
            .explosion
            .expect("explosive has an explosion");
        assert_eq!((explosion.power, explosion.fuse), (3, 120));
        assert!(!explosion.causes_fire);
        assert!(explosive.data.buoyant);
        let hot = &*vanilla_sulfur_cube_archetypes::HOT;
        assert!(hot.data.contact_damage.is_some());
    }

    #[test]
    fn archetypes_match_their_item_tags() {
        init_vanilla_registry();
        let explosive = &*vanilla_sulfur_cube_archetypes::EXPLOSIVE;
        assert!(explosive.matches_item(&vanilla_items::TNT));
        assert!(!explosive.matches_item(&vanilla_items::STONE));
    }
}
