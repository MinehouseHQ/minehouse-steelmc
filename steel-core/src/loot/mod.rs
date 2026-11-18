//! Vanilla's data-driven loot logic decoded at runtime: loot conditions
//! (`LootItemCondition`), context number providers (`ContextIntProvider`,
//! `ContextFloatProvider`) and the reloadable registries that hold them
//! (`predicate`, `context_int_provider`, `context_float_provider`).
//!
//! Steel's loot tables are generated at build time; this module covers what
//! data packs and commands supply as JSON or SNBT: `/execute if predicate`,
//! `/compute` and the advancement criteria conditions.

mod condition;
mod context;
mod floats;
mod ints;
mod level_value;
mod nbt_json;
mod ranges;
mod registry;

#[cfg(test)]
mod tests;

use std::str::FromStr as _;
use std::sync::Arc;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use steel_utils::Identifier;

pub(crate) use condition::{ContextPredicate, LootCondition};
pub(crate) use context::{EntityTarget, LootContext};
pub(crate) use floats::ContextFloatProvider;
pub(crate) use ints::ContextIntProvider;
pub(crate) use nbt_json::nbt_to_json;
pub(crate) use registry::{InRegistries, LootRegistries, LootRegistryKind, ServerLootRegistries};

/// Vanilla `Holder<T>` as the loot codecs decode it (`RegistryCodecs.holder`):
/// a registry id, or a value written in place.
#[derive(Debug, Clone)]
pub(crate) enum Holder<T> {
    /// A value written in place.
    Direct(Arc<T>),
    /// A registry entry by id.
    Reference(Identifier),
}

impl<T> Holder<T> {
    /// The registry id, for a reference (vanilla `Holder.unwrapKey`).
    pub(crate) const fn key(&self) -> Option<&Identifier> {
        match self {
            Self::Direct(_) => None,
            Self::Reference(key) => Some(key),
        }
    }
}

impl<T: LootElement> Holder<T> {
    /// The holder's value: the direct value, or the registry entry the
    /// context's registries hold for the id. `None` for an id the context
    /// cannot resolve, which load-time validation keeps out of the registries.
    pub(crate) fn value(&self, ctx: &LootContext<'_>) -> Option<Arc<T>> {
        match self {
            Self::Direct(value) => Some(Arc::clone(value)),
            Self::Reference(id) => ctx.registries?.get::<T>(id),
        }
    }

    /// Adds the registry ids this holder refers to, directly or through its
    /// value, to `out`.
    pub(crate) fn references(&self, out: &mut References) {
        match self {
            Self::Direct(value) => value.references(out),
            Self::Reference(id) => out.push((T::KIND, id.clone())),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Holder<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw<T> {
            Reference(String),
            Direct(T),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Reference(id) => Identifier::from_str(&id)
                .map(Self::Reference)
                .map_err(D::Error::custom),
            Raw::Direct(value) => Ok(Self::Direct(Arc::new(value))),
        }
    }
}

/// Vanilla `HolderSet<T>` as `RegistryCodecs.holderSet` decodes it: a
/// `#tag`, one holder, or a list of holders.
#[derive(Debug, Clone)]
pub(crate) enum HolderSet<T> {
    /// The entries of a registry tag.
    Tag(Identifier),
    /// Holders listed in place.
    Direct(Vec<Holder<T>>),
}

impl<T: LootElement> HolderSet<T> {
    /// The set's values, in order. `None` when one of them cannot be resolved.
    pub(crate) fn values(&self, ctx: &LootContext<'_>) -> Option<Vec<Arc<T>>> {
        match self {
            Self::Tag(tag) => ctx.registries?.tag::<T>(tag),
            Self::Direct(holders) => holders.iter().map(|holder| holder.value(ctx)).collect(),
        }
    }

    /// [`Holder::references`] for every holder; a tag refers to itself.
    pub(crate) fn references(&self, out: &mut References) {
        match self {
            Self::Tag(tag) => out.push((T::TAG_KIND, tag.clone())),
            Self::Direct(holders) => {
                for holder in holders {
                    holder.references(out);
                }
            }
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for HolderSet<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw<T> {
            Id(String),
            Many(Vec<Holder<T>>),
            One(T),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Id(id) => match id.strip_prefix('#') {
                Some(tag) => Identifier::from_str(tag)
                    .map(Self::Tag)
                    .map_err(D::Error::custom),
                None => Identifier::from_str(&id)
                    .map(|id| Self::Direct(vec![Holder::Reference(id)]))
                    .map_err(D::Error::custom),
            },
            Raw::Many(holders) => Ok(Self::Direct(holders)),
            Raw::One(value) => Ok(Self::Direct(vec![Holder::Direct(Arc::new(value))])),
        }
    }
}

/// Registry ids a loot value refers to, with the registry (or tag set) each
/// one belongs to.
pub(crate) type References = Vec<(LootRegistryKind, Identifier)>;

/// A value one of the loot registries holds.
pub(crate) trait LootElement: InRegistries {
    /// The registry holding values of this type.
    const KIND: LootRegistryKind;
    /// The tags of that registry.
    const TAG_KIND: LootRegistryKind;

    /// Adds the registry ids the value refers to to `out`.
    fn references(&self, out: &mut References);
}

/// Decodes a loot value from JSON, as vanilla's codecs read a data pack file.
pub(crate) fn from_json<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| error.to_string())
}

/// Java `Number.intValue` on a decoded JSON number, as vanilla's `Codec.INT`
/// reads one: any number is accepted, a fraction truncates toward zero.
pub(crate) fn java_int<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i32, D::Error> {
    let value = Value::deserialize(deserializer)?;
    json_int(&value).ok_or_else(|| D::Error::custom(format!("Not a number: {value}")))
}

/// [`java_int`] on a value already read.
pub(crate) fn json_int(value: &Value) -> Option<i32> {
    let number = value.as_number()?;
    Some(match number.as_i64() {
        // Java's narrowing cast keeps the low 32 bits.
        Some(value) => value as i32,
        None => number.as_f64()? as i32,
    })
}

/// Vanilla `Codec.BOOL` as `NbtOps` and `JsonOps` read it: a boolean, or a
/// number that is true unless zero.
pub(crate) fn java_bool<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    let value = Value::deserialize(deserializer)?;
    match &value {
        Value::Bool(value) => Ok(*value),
        Value::Number(number) => Ok(number.as_f64().is_some_and(|number| number as i8 != 0)),
        _ => Err(D::Error::custom(format!("Not a boolean: {value}"))),
    }
}

/// [`java_bool`] for an optional field.
pub(crate) fn java_bool_opt<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<bool>, D::Error> {
    java_bool(deserializer).map(Some)
}
