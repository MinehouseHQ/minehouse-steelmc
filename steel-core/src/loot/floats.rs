//! Vanilla `ContextFloatProvider`.

use std::sync::{Arc, LazyLock};

use rand::RngExt as _;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use steel_utils::Identifier;

use super::ints::{
    ArithmeticError, Case, ProviderResult, StoredNumber, Weighted, environment_attribute,
    pick_weighted, resolve, resolve_all, select_case, select_conditional,
};
use super::level_value::LevelBasedValue;
use super::{
    ContextIntProvider, Holder, HolderSet, LootCondition, LootContext, LootElement,
    LootRegistryKind, References,
};

/// Vanilla `ContextFloatProvider`: a float computed in a loot context.
#[derive(Debug, Clone)]
pub(crate) struct ContextFloatProvider(FloatProvider);

/// The provider types vanilla registers in `ContextFloatProviderTypes`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
enum FloatProvider {
    #[serde(rename = "minecraft:abs", alias = "abs")]
    Abs { input: Holder<ContextFloatProvider> },
    #[serde(rename = "minecraft:avg", alias = "avg")]
    Avg {
        inputs: HolderSet<ContextFloatProvider>,
    },
    #[serde(rename = "minecraft:ceil", alias = "ceil")]
    Ceil { input: Holder<ContextFloatProvider> },
    #[serde(rename = "minecraft:conditional", alias = "conditional")]
    Conditional {
        condition: Holder<LootCondition>,
        on_true: Holder<ContextFloatProvider>,
        #[serde(default = "ContextFloatProvider::zero")]
        on_false: Holder<ContextFloatProvider>,
    },
    #[serde(rename = "minecraft:constant", alias = "constant")]
    Constant { value: f32 },
    #[serde(rename = "minecraft:cos", alias = "cos")]
    Cos { input: Holder<ContextFloatProvider> },
    #[serde(rename = "minecraft:sub", alias = "sub")]
    Sub {
        left: Holder<ContextFloatProvider>,
        right: Holder<ContextFloatProvider>,
    },
    #[serde(rename = "minecraft:enchantment_level", alias = "enchantment_level")]
    EnchantmentLevel { amount: LevelBasedValue },
    #[serde(
        rename = "minecraft:environment_attribute",
        alias = "environment_attribute"
    )]
    EnvironmentAttribute { attribute: Identifier },
    #[serde(rename = "minecraft:floor", alias = "floor")]
    Floor { input: Holder<ContextFloatProvider> },
    #[serde(rename = "minecraft:from_int", alias = "from_int")]
    FromInt { input: Holder<ContextIntProvider> },
    #[serde(rename = "minecraft:length", alias = "length")]
    Length {
        inputs: HolderSet<ContextFloatProvider>,
    },
    #[serde(rename = "minecraft:max", alias = "max")]
    Max {
        inputs: HolderSet<ContextFloatProvider>,
    },
    #[serde(rename = "minecraft:min", alias = "min")]
    Min {
        inputs: HolderSet<ContextFloatProvider>,
    },
    #[serde(rename = "minecraft:mod", alias = "mod")]
    Mod {
        left: Holder<ContextFloatProvider>,
        right: Holder<ContextFloatProvider>,
    },
    #[serde(rename = "minecraft:negate", alias = "negate")]
    Negate { input: Holder<ContextFloatProvider> },
    #[serde(rename = "minecraft:number_dispatcher", alias = "number_dispatcher")]
    NumberDispatcher {
        cases: Vec<Case<ContextFloatProvider>>,
        #[serde(default = "ContextFloatProvider::zero")]
        default: Holder<ContextFloatProvider>,
    },
    #[serde(rename = "minecraft:pow", alias = "pow")]
    Pow {
        base: Holder<ContextFloatProvider>,
        exponent: Holder<ContextFloatProvider>,
    },
    #[serde(rename = "minecraft:mul", alias = "mul")]
    Mul {
        inputs: HolderSet<ContextFloatProvider>,
    },
    #[serde(rename = "minecraft:div", alias = "div")]
    Div {
        left: Holder<ContextFloatProvider>,
        right: Holder<ContextFloatProvider>,
    },
    #[serde(rename = "minecraft:round", alias = "round")]
    Round { input: Holder<ContextFloatProvider> },
    #[serde(rename = "minecraft:sin", alias = "sin")]
    Sin { input: Holder<ContextFloatProvider> },
    #[serde(rename = "minecraft:sqrt", alias = "sqrt")]
    Sqrt { input: Holder<ContextFloatProvider> },
    #[serde(rename = "minecraft:storage", alias = "storage")]
    Storage {
        #[serde(flatten)]
        access: StoredNumber,
        #[serde(default = "ContextFloatProvider::zero")]
        fallback: Holder<ContextFloatProvider>,
    },
    #[serde(rename = "minecraft:add", alias = "add")]
    Add {
        inputs: HolderSet<ContextFloatProvider>,
    },
    #[serde(rename = "minecraft:truncate", alias = "truncate")]
    Truncate { input: Holder<ContextFloatProvider> },
    #[serde(rename = "minecraft:uniform", alias = "uniform")]
    Uniform {
        min: Holder<ContextFloatProvider>,
        max: Holder<ContextFloatProvider>,
    },
    #[serde(rename = "minecraft:weighted_list", alias = "weighted_list")]
    WeightedList {
        distribution: Vec<Weighted<ContextFloatProvider>>,
    },
}

impl<'de> Deserialize<'de> for ContextFloatProvider {
    /// Vanilla `ContextFloatProviders.DIRECT_CODEC`: a bare number is a
    /// constant, anything else a typed provider.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        if let Some(number) = value.as_f64() {
            return Ok(Self::constant(number as f32));
        }
        FloatProvider::deserialize(value)
            .map(Self)
            .map_err(D::Error::custom)
    }
}

impl LootElement for ContextFloatProvider {
    const KIND: LootRegistryKind = LootRegistryKind::FloatProvider;
    const TAG_KIND: LootRegistryKind = LootRegistryKind::FloatProviderTag;

    fn references(&self, out: &mut References) {
        match &self.0 {
            FloatProvider::Abs { input }
            | FloatProvider::Ceil { input }
            | FloatProvider::Cos { input }
            | FloatProvider::Floor { input }
            | FloatProvider::Negate { input }
            | FloatProvider::Round { input }
            | FloatProvider::Sin { input }
            | FloatProvider::Sqrt { input }
            | FloatProvider::Truncate { input } => input.references(out),
            FloatProvider::Avg { inputs }
            | FloatProvider::Length { inputs }
            | FloatProvider::Max { inputs }
            | FloatProvider::Min { inputs }
            | FloatProvider::Mul { inputs }
            | FloatProvider::Add { inputs } => inputs.references(out),
            FloatProvider::Conditional {
                condition,
                on_true,
                on_false,
            } => {
                condition.references(out);
                on_true.references(out);
                on_false.references(out);
            }
            FloatProvider::Constant { .. }
            | FloatProvider::EnchantmentLevel { .. }
            | FloatProvider::EnvironmentAttribute { .. } => {}
            FloatProvider::Sub { left, right }
            | FloatProvider::Mod { left, right }
            | FloatProvider::Div { left, right }
            | FloatProvider::Pow {
                base: left,
                exponent: right,
            }
            | FloatProvider::Uniform {
                min: left,
                max: right,
            } => {
                left.references(out);
                right.references(out);
            }
            FloatProvider::FromInt { input } => input.references(out),
            FloatProvider::NumberDispatcher { cases, default } => {
                for case in cases {
                    case.references(out);
                }
                default.references(out);
            }
            FloatProvider::Storage { fallback, .. } => fallback.references(out),
            FloatProvider::WeightedList { distribution } => {
                for entry in distribution {
                    entry.data.references(out);
                }
            }
        }
    }
}

impl ContextFloatProvider {
    /// Vanilla `ConstantValue`.
    pub(crate) const fn constant(value: f32) -> Self {
        Self(FloatProvider::Constant { value })
    }

    /// Vanilla `ContextFloatProviders.exactly(0.0F)`, the default of
    /// optional fields.
    fn zero() -> Holder<Self> {
        Holder::Direct(Arc::new(Self::constant(0.0)))
    }

    /// Vanilla `ContextFloatProvider.getFloat`: errors and non-finite results
    /// give 0.
    pub(crate) fn get_float(&self, ctx: &LootContext<'_>) -> f32 {
        self.get_float_unsafe(ctx)
            .ok()
            .filter(|value| value.is_finite())
            .unwrap_or(0.0)
    }

    /// Vanilla `ContextFloatProvider.getFloatOrThrow`: a non-finite result is
    /// an error.
    pub(crate) fn get_float_or_throw(&self, ctx: &LootContext<'_>) -> ProviderResult<f32> {
        let value = self.get_float_unsafe(ctx)?;
        if value.is_finite() {
            Ok(value)
        } else {
            Err(ArithmeticError(format!("Invalid value: {value}")))
        }
    }

    /// Vanilla `ContextFloatProvider.getFloatUnsafe`.
    pub(crate) fn get_float_unsafe(&self, ctx: &LootContext<'_>) -> ProviderResult<f32> {
        let float = |holder: &Holder<Self>| resolve(holder, ctx)?.get_float_unsafe(ctx);
        let floats = |set: &HolderSet<Self>| -> ProviderResult<Vec<f32>> {
            resolve_all(set, ctx)?
                .iter()
                .map(|value| value.get_float_unsafe(ctx))
                .collect()
        };
        Ok(match &self.0 {
            FloatProvider::Abs { input } => float(input)?.abs(),
            FloatProvider::Avg { inputs } => {
                let values = floats(inputs)?;
                let sum = values.iter().fold(0.0_f32, |sum, value| sum + value);
                sum / values.len() as f32
            }
            // Vanilla `Mth.ceil`: through an int, which saturates.
            FloatProvider::Ceil { input } => float(input)?.ceil() as i32 as f32,
            FloatProvider::Conditional {
                condition,
                on_true,
                on_false,
            } => float(select_conditional(condition, on_true, on_false, ctx))?,
            FloatProvider::Constant { value } => *value,
            FloatProvider::Cos { input } => mth_cos(f64::from(float(input)?)),
            FloatProvider::Sub { left, right } => float(left)? - float(right)?,
            FloatProvider::EnchantmentLevel { amount } => {
                amount.calculate(ctx.enchantment_level.unwrap_or(0))
            }
            FloatProvider::EnvironmentAttribute { attribute } => {
                return Err(environment_attribute(attribute));
            }
            FloatProvider::Floor { input } => float(input)?.floor(),
            FloatProvider::FromInt { input } => resolve(input, ctx)?.get_int_unsafe(ctx)? as f32,
            FloatProvider::Length { inputs } => floats(inputs)?
                .into_iter()
                .fold(0.0_f32, |sum, value| sum + value * value)
                .sqrt(),
            FloatProvider::Max { inputs } => floats(inputs)?.into_iter().fold(-f32::MAX, java_max),
            FloatProvider::Min { inputs } => floats(inputs)?.into_iter().fold(f32::MAX, java_min),
            FloatProvider::Mod { left, right } => {
                let right = float(right)?;
                if right == 0.0 {
                    f32::NAN
                } else {
                    float(left)? % right
                }
            }
            FloatProvider::Negate { input } => -float(input)?,
            FloatProvider::NumberDispatcher { cases, default } => {
                float(select_case(cases, default, ctx))?
            }
            FloatProvider::Pow { base, exponent } => {
                let (base, exponent) = (float(base)?, float(exponent)?);
                if base == 0.0 && exponent == 0.0 {
                    f32::NAN
                } else {
                    f64::from(base).powf(f64::from(exponent)) as f32
                }
            }
            FloatProvider::Mul { inputs } => floats(inputs)?
                .into_iter()
                .fold(1.0_f32, |product, value| product * value),
            FloatProvider::Div { left, right } => float(left)? / float(right)?,
            FloatProvider::Round { input } => java_round(float(input)?) as f32,
            FloatProvider::Sin { input } => mth_sin(f64::from(float(input)?)),
            FloatProvider::Sqrt { input } => float(input)?.sqrt(),
            FloatProvider::Storage { access, fallback } => match access.numeric_tag(ctx) {
                Some(value) => value.float_value(),
                None => float(fallback)?,
            },
            FloatProvider::Add { inputs } => floats(inputs)?
                .into_iter()
                .fold(0.0_f32, |sum, value| sum + value),
            FloatProvider::Truncate { input } => float(input)?.trunc(),
            FloatProvider::Uniform { min, max } => {
                let (min, max) = (float(min)?, float(max)?);
                if min >= max {
                    min
                } else {
                    rand::rng().random::<f32>() * (max - min) + min
                }
            }
            FloatProvider::WeightedList { distribution } => float(pick_weighted(distribution)?)?,
        })
    }
}

/// Java `Math.max(float, float)`: NaN wins.
const fn java_max(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        f32::NAN
    } else {
        a.max(b)
    }
}

/// Java `Math.min(float, float)`: NaN wins.
const fn java_min(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        f32::NAN
    } else {
        a.min(b)
    }
}

/// Java `Math.round(float)`: half up, saturating, NaN to 0.
fn java_round(value: f32) -> i32 {
    (f64::from(value) + 0.5).floor() as i32
}

/// Table entries per radian in vanilla `Mth`'s sine table.
const MTH_SIN_SCALE: f64 = 10_430.378_350_470_453;

/// Vanilla `Mth.SIN`, the 65536-entry sine table.
static SIN_TABLE: LazyLock<Box<[f32]>> = LazyLock::new(|| {
    (0..65536_u32)
        .map(|i| (f64::from(i) / MTH_SIN_SCALE).sin() as f32)
        .collect()
});

/// Vanilla `Mth.sin(double)`, read from the table.
fn mth_sin(value: f64) -> f32 {
    SIN_TABLE[((value * MTH_SIN_SCALE) as i64 & 65535) as usize]
}

/// Vanilla `Mth.cos(double)`, read from the table a quarter turn on.
fn mth_cos(value: f64) -> f32 {
    SIN_TABLE[((value * MTH_SIN_SCALE + 16384.0) as i64 & 65535) as usize]
}
