//! Point of interest type command arguments.

use std::ptr;

use steel_registry::poi::PoiTypeRef;
use steel_registry::{POI_TYPE_REGISTRY, REGISTRY, RegistryExt as _, TaggedRegistryExt as _};
use steel_utils::{Identifier, translations};

use super::argument::{parse_identifier, unknown_resource};
use super::suggestions::identifier_matches;
use crate::command::brigadier::{
    CommandSyntaxError, CommandSyntaxErrorKind, StringReader, SuggestionsBuilder,
};

/// Vanilla `ResourceOrTagArgument.Result<PoiType>`: a registered point of
/// interest type or a point of interest type tag.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PoiTypeOrTag {
    Type(PoiTypeRef),
    Tag(Identifier),
}

impl PoiTypeOrTag {
    /// Vanilla `Result.test`: whether the POI type matches.
    pub(crate) fn matches(&self, poi_type: PoiTypeRef) -> bool {
        match self {
            Self::Type(expected) => ptr::eq(*expected, poi_type),
            Self::Tag(tag) => REGISTRY.poi_types.is_in_tag(poi_type, tag),
        }
    }

    /// Vanilla `Result.asPrintable`.
    pub(crate) fn as_printable(&self) -> String {
        match self {
            Self::Type(poi_type) => poi_type.key.to_string(),
            Self::Tag(tag) => format!("#{tag}"),
        }
    }
}

/// Vanilla `ResourceOrTagArgument.parse` for `minecraft:point_of_interest_type`.
pub(super) fn parse_poi_type_or_tag(
    reader: &mut StringReader<'_>,
) -> Result<PoiTypeOrTag, CommandSyntaxError> {
    if reader.peek() != Some('#') {
        let key = parse_identifier(reader)?;
        return REGISTRY.poi_types.by_key(&key).map_or_else(
            || Err(unknown_resource(reader, &key, &POI_TYPE_REGISTRY)),
            |poi_type| Ok(PoiTypeOrTag::Type(poi_type)),
        );
    }

    let start = reader.checkpoint();
    reader.skip();
    let key = match parse_identifier(reader) {
        Ok(key) => key,
        Err(error) => {
            reader.restore(start);
            return Err(error);
        }
    };
    if REGISTRY.poi_types.tag_keys().any(|tag| tag == &key) {
        return Ok(PoiTypeOrTag::Tag(key));
    }

    let message = translations::ARGUMENT_RESOURCE_TAG_NOT_FOUND
        .message([key.to_string(), POI_TYPE_REGISTRY.to_string()])
        .component();
    let error = reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(message)));
    reader.restore(start);
    Err(error)
}

/// Suggests point of interest types and, after `#`, their tags.
pub(super) fn suggest_poi_types(builder: &mut SuggestionsBuilder<'_>) {
    let remaining = builder.remaining_lowercase();
    let suggestions = if let Some(tag_prefix) = remaining.strip_prefix('#') {
        REGISTRY
            .poi_types
            .tag_keys()
            .filter(|tag| identifier_matches(tag_prefix, tag))
            .map(|tag| format!("#{tag}"))
            .collect::<Vec<_>>()
    } else {
        REGISTRY
            .poi_types
            .iter()
            .filter(|(_, poi_type)| identifier_matches(remaining, &poi_type.key))
            .map(|(_, poi_type)| poi_type.key.to_string())
            .chain(
                REGISTRY
                    .poi_types
                    .tag_keys()
                    .filter(|tag| identifier_matches(remaining, tag))
                    .map(|tag| format!("#{tag}")),
            )
            .collect()
    };
    for suggestion in suggestions {
        builder.suggest(suggestion);
    }
}
