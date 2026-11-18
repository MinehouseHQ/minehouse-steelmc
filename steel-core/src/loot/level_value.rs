//! Vanilla `LevelBasedValue` decoded at runtime, for loot values written in
//! data packs and commands (the registry's copy is generated at build time).

use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

/// Vanilla `LevelBasedValue`: a float computed from an enchantment level.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LevelBasedValue {
    /// A bare number.
    Constant(f32),
    /// `clamped`: `value` kept within `[min, max]`.
    Clamped {
        value: Box<Self>,
        min: f32,
        max: f32,
    },
    /// `exponent`: `base` to the power of `power`.
    Exponent { base: Box<Self>, power: Box<Self> },
    /// `fraction`: `numerator / denominator`, 0 for a zero denominator.
    Fraction {
        numerator: Box<Self>,
        denominator: Box<Self>,
    },
    /// `levels_squared`: `level² + added`.
    LevelsSquared { added: f32 },
    /// `linear`: `base + per_level_above_first * (level - 1)`.
    Linear {
        base: f32,
        per_level_above_first: f32,
    },
    /// `lookup`: `values[level - 1]`, or `fallback` past the end.
    Lookup {
        values: Vec<f32>,
        fallback: Box<Self>,
    },
}

impl LevelBasedValue {
    /// Vanilla `LevelBasedValue.calculate(level)`.
    pub(crate) fn calculate(&self, level: i32) -> f32 {
        match self {
            Self::Constant(value) => *value,
            Self::Clamped { value, min, max } => {
                // Vanilla `Mth.clamp`: the minimum wins a crossed range.
                let value = value.calculate(level);
                if value < *min { *min } else { value.min(*max) }
            }
            Self::Exponent { base, power } => {
                f64::from(base.calculate(level)).powf(f64::from(power.calculate(level))) as f32
            }
            Self::Fraction {
                numerator,
                denominator,
            } => {
                let denominator = denominator.calculate(level);
                if denominator == 0.0 {
                    0.0
                } else {
                    numerator.calculate(level) / denominator
                }
            }
            Self::LevelsSquared { added } => level.wrapping_mul(level) as f32 + added,
            Self::Linear {
                base,
                per_level_above_first,
            } => base + per_level_above_first * level.wrapping_sub(1) as f32,
            Self::Lookup { values, fallback } => usize::try_from(level)
                .ok()
                .and_then(|level| level.checked_sub(1))
                .and_then(|index| values.get(index))
                .copied()
                .unwrap_or_else(|| fallback.calculate(level)),
        }
    }
}

/// The typed forms of [`LevelBasedValue`], as the dispatch codec reads them.
#[derive(Deserialize)]
#[serde(tag = "type")]
enum Typed {
    #[serde(rename = "minecraft:clamped", alias = "clamped")]
    Clamped {
        value: LevelBasedValue,
        min: f32,
        max: f32,
    },
    #[serde(rename = "minecraft:exponent", alias = "exponent")]
    Exponent {
        base: LevelBasedValue,
        power: LevelBasedValue,
    },
    #[serde(rename = "minecraft:fraction", alias = "fraction")]
    Fraction {
        numerator: LevelBasedValue,
        denominator: LevelBasedValue,
    },
    #[serde(rename = "minecraft:levels_squared", alias = "levels_squared")]
    LevelsSquared { added: f32 },
    #[serde(rename = "minecraft:linear", alias = "linear")]
    Linear {
        base: f32,
        per_level_above_first: f32,
    },
    #[serde(rename = "minecraft:lookup", alias = "lookup")]
    Lookup {
        values: Vec<f32>,
        fallback: LevelBasedValue,
    },
}

impl<'de> Deserialize<'de> for LevelBasedValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Constant(f32),
            Typed(Box<Typed>),
        }
        Ok(match Raw::deserialize(deserializer)? {
            Raw::Constant(value) => Self::Constant(value),
            Raw::Typed(typed) => match *typed {
                Typed::Clamped { value, min, max } => {
                    if max <= min {
                        return Err(D::Error::custom(format!(
                            "Max must be larger than min, min: {min}, max: {max}"
                        )));
                    }
                    Self::Clamped {
                        value: Box::new(value),
                        min,
                        max,
                    }
                }
                Typed::Exponent { base, power } => Self::Exponent {
                    base: Box::new(base),
                    power: Box::new(power),
                },
                Typed::Fraction {
                    numerator,
                    denominator,
                } => Self::Fraction {
                    numerator: Box::new(numerator),
                    denominator: Box::new(denominator),
                },
                Typed::LevelsSquared { added } => Self::LevelsSquared { added },
                Typed::Linear {
                    base,
                    per_level_above_first,
                } => Self::Linear {
                    base,
                    per_level_above_first,
                },
                Typed::Lookup { values, fallback } => Self::Lookup {
                    values,
                    fallback: Box::new(fallback),
                },
            },
        })
    }
}
