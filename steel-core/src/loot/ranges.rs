//! Vanilla `IntRangePredicate` and `FloatRangePredicate`: ranges whose bounds
//! are providers computed in the loot context.

use serde::Deserialize;

use super::{ContextFloatProvider, ContextIntProvider, Holder, LootContext, References};

/// Vanilla `IntRangePredicate`: one provider the value must equal, or
/// optional inclusive bounds.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum IntRange {
    /// `Point`: the value equals the provider's.
    Point(Holder<ContextIntProvider>),
    /// `Line`: the value is within the bounds present.
    Line {
        min: Option<Holder<ContextIntProvider>>,
        max: Option<Holder<ContextIntProvider>>,
    },
}

impl IntRange {
    /// Vanilla `test(context, input)`; bounds are read with `getInt`.
    pub(crate) fn test(&self, ctx: &LootContext<'_>, input: i32) -> bool {
        let value = |holder: &Holder<ContextIntProvider>| {
            holder
                .value(ctx)
                .map_or(0, |provider| provider.get_int(ctx))
        };
        match self {
            Self::Point(point) => value(point) == input,
            Self::Line { min, max } => {
                min.as_ref().is_none_or(|min| input >= value(min))
                    && max.as_ref().is_none_or(|max| input <= value(max))
            }
        }
    }

    /// The registry ids the bounds refer to.
    pub(crate) fn references(&self, out: &mut References) {
        match self {
            Self::Point(point) => point.references(out),
            Self::Line { min, max } => {
                for bound in [min, max].into_iter().flatten() {
                    bound.references(out);
                }
            }
        }
    }
}

/// Vanilla `FloatRangePredicate`, [`IntRange`] over floats.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum FloatRange {
    /// `Point`: the value equals the provider's.
    Point(Holder<ContextFloatProvider>),
    /// `Line`: the value is within the bounds present.
    Line {
        min: Option<Holder<ContextFloatProvider>>,
        max: Option<Holder<ContextFloatProvider>>,
    },
}

impl FloatRange {
    /// Vanilla `test(context, input)`; bounds are read with `getFloat`.
    #[expect(clippy::float_cmp, reason = "vanilla compares the exact value")]
    pub(crate) fn test(&self, ctx: &LootContext<'_>, input: f32) -> bool {
        let value = |holder: &Holder<ContextFloatProvider>| {
            holder
                .value(ctx)
                .map_or(0.0, |provider| provider.get_float(ctx))
        };
        match self {
            Self::Point(point) => value(point) == input,
            Self::Line { min, max } => {
                min.as_ref().is_none_or(|min| input >= value(min))
                    && max.as_ref().is_none_or(|max| input <= value(max))
            }
        }
    }

    /// The registry ids the bounds refer to.
    pub(crate) fn references(&self, out: &mut References) {
        match self {
            Self::Point(point) => point.references(out),
            Self::Line { min, max } => {
                for bound in [min, max].into_iter().flatten() {
                    bound.references(out);
                }
            }
        }
    }
}
