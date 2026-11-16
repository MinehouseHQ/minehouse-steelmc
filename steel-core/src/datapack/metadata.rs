//! Vanilla `Pack.readPackMetadata` for a pack's `pack.mcmeta`, and the
//! feature flags packs request.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde_json::Value;
use steel_utils::Identifier;
use text_components::TextComponent;

/// Vanilla `FeatureFlags`: the flags this version registers.
const KNOWN_FEATURE_FLAGS: [&str; 4] = [
    "vanilla",
    "trade_rebalance",
    "redstone_experiments",
    "minecart_improvements",
];

/// Vanilla `FeatureFlagSet`, as the flags' ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FeatureFlagSet(BTreeSet<Identifier>);

impl FeatureFlagSet {
    /// Vanilla `FeatureFlags.VANILLA_SET`.
    pub(crate) fn vanilla() -> Self {
        Self::of(&["vanilla"])
    }

    /// The set of the vanilla flags named by `paths`.
    pub(crate) fn of(paths: &[&'static str]) -> Self {
        Self(
            paths
                .iter()
                .map(|path| Identifier::vanilla_static(path))
                .collect(),
        )
    }

    /// Vanilla `FeatureFlagSet.isEmpty`.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Vanilla `FeatureFlagSet.isSubsetOf`.
    pub(crate) fn is_subset_of(&self, other: &Self) -> bool {
        self.0.is_subset(&other.0)
    }

    /// Vanilla `FeatureFlags.printMissingFlags(allowed, self)`: the requested
    /// flags `allowed` lacks, comma separated.
    pub(crate) fn print_missing(&self, allowed: &Self) -> String {
        let mut missing = String::new();
        for flag in self.0.difference(&allowed.0) {
            if !missing.is_empty() {
                missing.push_str(", ");
            }
            let _ = write!(missing, "{flag}");
        }
        missing
    }
}

/// What a pack's metadata says about it.
#[derive(Debug, Clone)]
pub(crate) struct PackMetadata {
    /// Vanilla `Pack.Metadata.description`.
    pub(crate) description: TextComponent,
    /// Vanilla `Pack.Metadata.requestedFeatures`.
    pub(crate) requested_features: FeatureFlagSet,
}

/// Vanilla `Pack.readPackMetadata` for `pack.mcmeta`'s `contents`. `None`, with
/// a warning, for a pack vanilla would leave out of the repository.
///
/// Vanilla falls back to reading only the `pack` section's description when
/// its supported formats do not decode, so the formats never decide whether a
/// pack loads; they only feed the pack's compatibility, which nothing on a
/// server reads. Steel reads the description alone.
pub(crate) fn read_pack_metadata(pack_id: &str, contents: &[u8]) -> Option<PackMetadata> {
    let read = serde_json::from_slice::<Value>(contents)
        .map_err(|error| error.to_string())
        .and_then(|root| parse_metadata(&root));
    match read {
        Ok(Some(metadata)) => Some(metadata),
        Ok(None) => {
            log::warn!("Missing metadata in pack {pack_id}");
            None
        }
        Err(error) => {
            log::warn!("Failed to read pack {pack_id} metadata: {error}");
            None
        }
    }
}

/// `Ok(None)` when there is no `pack` section.
fn parse_metadata(root: &Value) -> Result<Option<PackMetadata>, String> {
    let root = root
        .as_object()
        .ok_or_else(|| String::from("pack.mcmeta is not an object"))?;
    let Some(pack) = root.get("pack") else {
        return Ok(None);
    };
    let description = pack
        .as_object()
        .and_then(|pack| pack.get("description"))
        .ok_or_else(|| String::from("No key description in pack"))?;
    let description = component_from_json(description)
        .map_err(|error| format!("Invalid pack description: {error}"))?;
    let requested_features = root
        .get("features")
        .map(parse_features)
        .transpose()?
        .unwrap_or_default();
    Ok(Some(PackMetadata {
        description,
        requested_features,
    }))
}

/// Vanilla `ComponentSerialization.CODEC` over JSON: a plain string, a list
/// whose first entry takes the rest as children, or a full component object.
fn component_from_json(value: &Value) -> Result<TextComponent, String> {
    match value {
        Value::String(text) => Ok(TextComponent::plain(text.clone())),
        Value::Array(entries) => {
            let mut entries = entries.iter();
            let first = entries
                .next()
                .ok_or_else(|| String::from("Empty component list"))?;
            let mut component = component_from_json(first)?;
            for entry in entries {
                component.children.push(component_from_json(entry)?);
            }
            Ok(component)
        }
        _ => serde_json::from_value::<TextComponent>(value.clone())
            .map_err(|error| error.to_string()),
    }
}

/// Vanilla `FeatureFlagsMetadataSection.CODEC`: an `enabled` list of known
/// flag ids.
fn parse_features(section: &Value) -> Result<FeatureFlagSet, String> {
    let enabled = section
        .get("enabled")
        .and_then(Value::as_array)
        .ok_or_else(|| String::from("No key enabled in features"))?;
    let mut flags = BTreeSet::new();
    let mut unknown = Vec::new();
    for id in enabled {
        let id = id
            .as_str()
            .and_then(|id| id.parse::<Identifier>().ok())
            .ok_or_else(|| format!("Not a valid feature id: {id}"))?;
        if id.namespace == "minecraft" && KNOWN_FEATURE_FLAGS.contains(&&*id.path) {
            flags.insert(id);
        } else {
            unknown.push(id.to_string());
        }
    }
    if unknown.is_empty() {
        Ok(FeatureFlagSet(flags))
    } else {
        Err(format!("Unknown feature ids: [{}]", unknown.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_description_and_requested_features() {
        let metadata = read_pack_metadata(
            "file/demo",
            br#"{"pack":{"description":"Demo","pack_format":5},"features":{"enabled":["minecraft:trade_rebalance"]}}"#,
        )
        .expect("metadata should read");
        assert_eq!(metadata.description, TextComponent::plain("Demo"));
        assert_eq!(
            metadata.requested_features,
            FeatureFlagSet::of(&["trade_rebalance"])
        );
        assert!(
            !metadata
                .requested_features
                .is_subset_of(&FeatureFlagSet::vanilla())
        );
        assert_eq!(
            metadata
                .requested_features
                .print_missing(&FeatureFlagSet::vanilla()),
            "minecraft:trade_rebalance"
        );
    }

    #[test]
    fn packs_without_a_pack_section_or_with_unknown_features_are_left_out() {
        assert!(read_pack_metadata("file/a", b"{}").is_none());
        assert!(read_pack_metadata("file/b", b"not json").is_none());
        assert!(read_pack_metadata("file/c", br#"{"pack":{}}"#).is_none());
        assert!(
            read_pack_metadata(
                "file/d",
                br#"{"pack":{"description":""},"features":{"enabled":["minecraft:jetpacks"]}}"#,
            )
            .is_none()
        );
        let metadata = read_pack_metadata("file/e", br#"{"pack":{"description":""}}"#)
            .expect("a pack without features should read");
        assert!(metadata.requested_features.is_empty());
    }
}
