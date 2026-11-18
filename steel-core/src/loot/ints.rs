//! Vanilla `ContextIntProvider` and the pieces it shares with
//! `ContextFloatProvider`: dispatcher cases, weighted lists, score and storage
//! access.

use std::fmt;
use std::str::FromStr as _;
use std::sync::Arc;

use rand::RngExt as _;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use simdnbt::owned::NbtTag;
use steel_utils::Identifier;
use steel_utils::nbt::{NbtPath, parse_nbt_path};

use super::{
    ContextFloatProvider, EntityTarget, Holder, HolderSet, LootCondition, LootContext, LootElement,
    LootRegistryKind, References, json_int,
};
use crate::scoreboard::ScoreHolder;

/// Vanilla's `ArithmeticException` from a provider: an overflow, a division
/// by zero or a value with no int form. `getInt` turns it into 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArithmeticError(pub(crate) String);

impl fmt::Display for ArithmeticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl ArithmeticError {
    fn overflow(operation: &str) -> Self {
        Self(format!("{operation} overflow"))
    }
}

/// What a provider returns before `getInt` or `getFloat` hides errors.
pub(crate) type ProviderResult<T> = Result<T, ArithmeticError>;

/// The value of `holder`, or an error for a reference the context cannot
/// resolve.
pub(super) fn resolve<T: LootElement>(
    holder: &Holder<T>,
    ctx: &LootContext<'_>,
) -> ProviderResult<Arc<T>> {
    holder.value(ctx).ok_or_else(|| {
        ArithmeticError(format!(
            "Unbound reference {}",
            holder.key().map_or_else(String::new, ToString::to_string)
        ))
    })
}

/// [`resolve`] for every holder of a set.
pub(super) fn resolve_all<T: LootElement>(
    set: &HolderSet<T>,
    ctx: &LootContext<'_>,
) -> ProviderResult<Vec<Arc<T>>> {
    set.values(ctx)
        .ok_or_else(|| ArithmeticError("Unbound reference in holder set".to_owned()))
}

/// Vanilla `ContextIntProvider`: an int computed in a loot context.
#[derive(Debug, Clone)]
pub(crate) struct ContextIntProvider(IntProvider);

/// The provider types vanilla registers in `ContextIntProviderTypes`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
enum IntProvider {
    #[serde(rename = "minecraft:abs", alias = "abs")]
    Abs { input: Holder<ContextIntProvider> },
    #[serde(rename = "minecraft:avg", alias = "avg")]
    Avg {
        inputs: HolderSet<ContextIntProvider>,
    },
    #[serde(rename = "minecraft:binomial", alias = "binomial")]
    Binomial {
        n: Holder<ContextIntProvider>,
        p: Holder<ContextFloatProvider>,
    },
    #[serde(rename = "minecraft:conditional", alias = "conditional")]
    Conditional {
        condition: Holder<LootCondition>,
        on_true: Holder<ContextIntProvider>,
        #[serde(default = "ContextIntProvider::zero")]
        on_false: Holder<ContextIntProvider>,
    },
    #[serde(rename = "minecraft:constant", alias = "constant")]
    Constant {
        #[serde(deserialize_with = "super::java_int")]
        value: i32,
    },
    #[serde(rename = "minecraft:sub", alias = "sub")]
    Sub {
        left: Holder<ContextIntProvider>,
        right: Holder<ContextIntProvider>,
    },
    #[serde(
        rename = "minecraft:environment_attribute",
        alias = "environment_attribute"
    )]
    EnvironmentAttribute { attribute: Identifier },
    #[serde(rename = "minecraft:from_float", alias = "from_float")]
    FromFloat { input: Holder<ContextFloatProvider> },
    #[serde(rename = "minecraft:max", alias = "max")]
    Max {
        inputs: HolderSet<ContextIntProvider>,
    },
    #[serde(rename = "minecraft:min", alias = "min")]
    Min {
        inputs: HolderSet<ContextIntProvider>,
    },
    #[serde(rename = "minecraft:floor_mod", alias = "floor_mod")]
    FloorMod {
        left: Holder<ContextIntProvider>,
        right: Holder<ContextIntProvider>,
    },
    #[serde(rename = "minecraft:floor_div", alias = "floor_div")]
    FloorDiv {
        left: Holder<ContextIntProvider>,
        right: Holder<ContextIntProvider>,
    },
    #[serde(rename = "minecraft:mod", alias = "mod")]
    Mod {
        left: Holder<ContextIntProvider>,
        right: Holder<ContextIntProvider>,
    },
    #[serde(rename = "minecraft:div", alias = "div")]
    Div {
        left: Holder<ContextIntProvider>,
        right: Holder<ContextIntProvider>,
    },
    #[serde(rename = "minecraft:negate", alias = "negate")]
    Negate { input: Holder<ContextIntProvider> },
    #[serde(rename = "minecraft:number_dispatcher", alias = "number_dispatcher")]
    NumberDispatcher {
        cases: Vec<Case<ContextIntProvider>>,
        #[serde(default = "ContextIntProvider::zero")]
        default: Holder<ContextIntProvider>,
    },
    #[serde(rename = "minecraft:pow", alias = "pow")]
    Pow {
        base: Holder<ContextIntProvider>,
        exponent: Holder<ContextIntProvider>,
    },
    #[serde(rename = "minecraft:mul", alias = "mul")]
    Mul {
        inputs: HolderSet<ContextIntProvider>,
    },
    #[serde(rename = "minecraft:score", alias = "score")]
    Score {
        target: ScoreTarget,
        score: String,
        #[serde(default = "ContextIntProvider::zero")]
        fallback: Holder<ContextIntProvider>,
    },
    #[serde(rename = "minecraft:storage", alias = "storage")]
    Storage {
        #[serde(flatten)]
        access: StoredNumber,
        #[serde(default = "ContextIntProvider::zero")]
        fallback: Holder<ContextIntProvider>,
    },
    #[serde(rename = "minecraft:add", alias = "add")]
    Add {
        inputs: HolderSet<ContextIntProvider>,
    },
    #[serde(rename = "minecraft:uniform", alias = "uniform")]
    Uniform {
        min: Holder<ContextIntProvider>,
        max: Holder<ContextIntProvider>,
    },
    #[serde(rename = "minecraft:weighted_list", alias = "weighted_list")]
    WeightedList {
        distribution: Vec<Weighted<ContextIntProvider>>,
    },
}

impl<'de> Deserialize<'de> for ContextIntProvider {
    /// Vanilla `ContextIntProviders.DIRECT_CODEC`: a bare int is a constant,
    /// anything else a typed provider.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        if value.is_number() {
            let value = json_int(&value)
                .ok_or_else(|| D::Error::custom(format!("Not a number: {value}")))?;
            return Ok(Self::constant(value));
        }
        IntProvider::deserialize(value)
            .map(Self)
            .map_err(D::Error::custom)
    }
}

impl LootElement for ContextIntProvider {
    const KIND: LootRegistryKind = LootRegistryKind::IntProvider;
    const TAG_KIND: LootRegistryKind = LootRegistryKind::IntProviderTag;

    fn references(&self, out: &mut References) {
        match &self.0 {
            IntProvider::Abs { input } | IntProvider::Negate { input } => input.references(out),
            IntProvider::Avg { inputs }
            | IntProvider::Max { inputs }
            | IntProvider::Min { inputs }
            | IntProvider::Mul { inputs }
            | IntProvider::Add { inputs } => inputs.references(out),
            IntProvider::Binomial { n, p } => {
                n.references(out);
                p.references(out);
            }
            IntProvider::Conditional {
                condition,
                on_true,
                on_false,
            } => {
                condition.references(out);
                on_true.references(out);
                on_false.references(out);
            }
            IntProvider::Constant { .. } | IntProvider::EnvironmentAttribute { .. } => {}
            IntProvider::Sub { left, right }
            | IntProvider::FloorMod { left, right }
            | IntProvider::FloorDiv { left, right }
            | IntProvider::Mod { left, right }
            | IntProvider::Div { left, right }
            | IntProvider::Pow {
                base: left,
                exponent: right,
            }
            | IntProvider::Uniform {
                min: left,
                max: right,
            } => {
                left.references(out);
                right.references(out);
            }
            IntProvider::FromFloat { input } => input.references(out),
            IntProvider::NumberDispatcher { cases, default } => {
                for case in cases {
                    case.references(out);
                }
                default.references(out);
            }
            IntProvider::Score { fallback, .. } | IntProvider::Storage { fallback, .. } => {
                fallback.references(out);
            }
            IntProvider::WeightedList { distribution } => {
                for entry in distribution {
                    entry.data.references(out);
                }
            }
        }
    }
}

impl ContextIntProvider {
    /// Vanilla `ConstantValue`.
    pub(crate) const fn constant(value: i32) -> Self {
        Self(IntProvider::Constant { value })
    }

    /// Vanilla `ContextIntProviders.exactly(0)`, the default of optional
    /// fields.
    fn zero() -> Holder<Self> {
        Holder::Direct(Arc::new(Self::constant(0)))
    }

    /// Vanilla `ContextIntProvider.getInt`: errors give 0.
    pub(crate) fn get_int(&self, ctx: &LootContext<'_>) -> i32 {
        self.get_int_unsafe(ctx).unwrap_or(0)
    }

    /// Vanilla `ContextIntProvider.getIntUnsafe`.
    pub(crate) fn get_int_unsafe(&self, ctx: &LootContext<'_>) -> ProviderResult<i32> {
        let int = |holder: &Holder<Self>| resolve(holder, ctx)?.get_int_unsafe(ctx);
        let ints = |set: &HolderSet<Self>| -> ProviderResult<Vec<i32>> {
            resolve_all(set, ctx)?
                .iter()
                .map(|value| value.get_int_unsafe(ctx))
                .collect()
        };
        match &self.0 {
            IntProvider::Abs { input } => {
                let value = int(input)?;
                value
                    .checked_abs()
                    .ok_or_else(|| ArithmeticError::overflow("integer"))
            }
            IntProvider::Avg { inputs } => {
                let values = ints(inputs)?;
                let sum = values.iter().map(|value| i64::from(*value)).sum::<i64>();
                let count = i64::try_from(values.len()).unwrap_or(i64::MAX);
                if count == 0 {
                    return Err(ArithmeticError("/ by zero".to_owned()));
                }
                long_to_int(sum / count)
            }
            IntProvider::Binomial { n, p } => {
                let n = int(n)?;
                let p = resolve(p, ctx)?.get_float_or_throw(ctx)?;
                let mut rng = rand::rng();
                Ok((0..n).filter(|_| rng.random::<f32>() < p).count() as i32)
            }
            IntProvider::Conditional {
                condition,
                on_true,
                on_false,
            } => int(select_conditional(condition, on_true, on_false, ctx)),
            IntProvider::Constant { value } => Ok(*value),
            IntProvider::Sub { left, right } => int(left)?
                .checked_sub(int(right)?)
                .ok_or_else(|| ArithmeticError::overflow("integer")),
            IntProvider::EnvironmentAttribute { attribute } => {
                Err(environment_attribute(attribute))
            }
            IntProvider::FromFloat { input } => {
                float_to_int(resolve(input, ctx)?.get_float_unsafe(ctx)?)
            }
            IntProvider::Max { inputs } => Ok(ints(inputs)?.into_iter().fold(i32::MIN, i32::max)),
            IntProvider::Min { inputs } => Ok(ints(inputs)?.into_iter().fold(i32::MAX, i32::min)),
            IntProvider::FloorMod { left, right } => floor_mod(int(left)?, int(right)?),
            IntProvider::FloorDiv { left, right } => floor_div_exact(int(left)?, int(right)?),
            // Java `%` and `/`: `MIN_VALUE % -1` is 0 and `MIN_VALUE / -1`
            // wraps; only a zero divisor throws.
            IntProvider::Mod { left, right } => Ok(int(left)?.wrapping_rem(nonzero(int(right)?)?)),
            IntProvider::Div { left, right } => Ok(int(left)?.wrapping_div(nonzero(int(right)?)?)),
            IntProvider::Negate { input } => int(input)?
                .checked_neg()
                .ok_or_else(|| ArithmeticError::overflow("integer")),
            IntProvider::NumberDispatcher { cases, default } => {
                int(select_case(cases, default, ctx))
            }
            IntProvider::Pow { base, exponent } => pow_exact(int(base)?, int(exponent)?),
            IntProvider::Mul { inputs } => {
                // Vanilla multiplies in a long, wrapping, then narrows.
                let product = ints(inputs)?.into_iter().fold(1_i64, |product, value| {
                    product.wrapping_mul(i64::from(value))
                });
                long_to_int(product)
            }
            IntProvider::Score {
                target,
                score,
                fallback,
            } => match target.score(score, ctx) {
                Some(value) => Ok(value),
                None => int(fallback),
            },
            IntProvider::Storage { access, fallback } => match access.numeric_tag(ctx) {
                // Java `Number.intValue`.
                Some(value) => Ok(value.int_value()),
                None => int(fallback),
            },
            IntProvider::Add { inputs } => {
                let sum = ints(inputs)?
                    .into_iter()
                    .fold(0_i64, |sum, value| sum.wrapping_add(i64::from(value)));
                long_to_int(sum)
            }
            IntProvider::Uniform { min, max } => Ok(next_int(int(min)?, int(max)?)),
            IntProvider::WeightedList { distribution } => int(pick_weighted(distribution)?),
        }
    }
}

/// Vanilla `ContextIntProvider.longToIntSafe`.
fn long_to_int(value: i64) -> ProviderResult<i32> {
    i32::try_from(value)
        .map_err(|_| ArithmeticError(format!("Value {value} can't be safely converted to int")))
}

/// Vanilla `ContextIntProvider.floatToIntSafe`: a finite float, truncated to
/// a long, that fits in an int.
pub(super) fn float_to_int(value: f32) -> ProviderResult<i32> {
    if !value.is_finite() {
        return Err(ArithmeticError(format!(
            "Value {value} can't be safely converted to int"
        )));
    }
    // Java's `(long)` cast truncates toward zero and saturates.
    long_to_int(value as i64)
}

/// Java `Math.floorDivExact`.
fn floor_div_exact(left: i32, right: i32) -> ProviderResult<i32> {
    if right == 0 {
        return Err(ArithmeticError("/ by zero".to_owned()));
    }
    let quotient = left
        .checked_div(right)
        .ok_or_else(|| ArithmeticError::overflow("integer"))?;
    if (left ^ right) < 0 && quotient * right != left {
        Ok(quotient - 1)
    } else {
        Ok(quotient)
    }
}

/// Java `Math.powExact(int, int)`, after vanilla's own `0^0` check.
fn pow_exact(base: i32, exponent: i32) -> ProviderResult<i32> {
    if base == 0 && exponent == 0 {
        return Err(ArithmeticError(
            "Result of 0 to the power of 0 is undefined".to_owned(),
        ));
    }
    let Ok(exponent) = u32::try_from(exponent) else {
        return Err(ArithmeticError("negative exponent".to_owned()));
    };
    base.checked_pow(exponent)
        .ok_or_else(|| ArithmeticError::overflow("integer"))
}

/// Vanilla `Mth.nextInt(random, min, max)`: `min` for an empty range.
fn next_int(min: i32, max: i32) -> i32 {
    if min >= max {
        return min;
    }
    rand::rng().random_range(min..=max)
}

/// A divisor, or Java's `ArithmeticException` for zero.
fn nonzero(divisor: i32) -> ProviderResult<i32> {
    if divisor == 0 {
        return Err(ArithmeticError("/ by zero".to_owned()));
    }
    Ok(divisor)
}

/// Java `Math.floorMod`: the result takes the divisor's sign.
fn floor_mod(left: i32, right: i32) -> ProviderResult<i32> {
    let right = nonzero(right)?;
    let rem = left.wrapping_rem(right);
    Ok(if rem != 0 && (rem ^ right) < 0 {
        rem + right
    } else {
        rem
    })
}

/// The environment attribute providers and checks read
/// `level.environmentAttributes()`, which Steel does not have.
pub(super) fn environment_attribute(attribute: &Identifier) -> ArithmeticError {
    // TODO: Read the attribute once Steel has vanilla's environment attribute system.
    ArithmeticError(format!(
        "Environment attribute {attribute} is not supported"
    ))
}

/// Vanilla `ConditionalProvider.selectValue`.
pub(super) fn select_conditional<'h, T>(
    condition: &Holder<LootCondition>,
    on_true: &'h Holder<T>,
    on_false: &'h Holder<T>,
    ctx: &LootContext<'_>,
) -> &'h Holder<T> {
    if condition
        .value(ctx)
        .is_some_and(|condition| condition.test(ctx))
    {
        on_true
    } else {
        on_false
    }
}

/// Vanilla `DispatcherProvider.Case`.
#[derive(Debug, Clone, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub(super) struct Case<T> {
    condition: Holder<LootCondition>,
    value: Holder<T>,
}

impl<T: LootElement> Case<T> {
    pub(super) fn references(&self, out: &mut References) {
        self.condition.references(out);
        self.value.references(out);
    }
}

/// Vanilla `DispatcherProvider.selectValue`: the first case whose condition
/// passes, else the default.
pub(super) fn select_case<'h, T>(
    cases: &'h [Case<T>],
    default: &'h Holder<T>,
    ctx: &LootContext<'_>,
) -> &'h Holder<T> {
    cases
        .iter()
        .find(|case| {
            case.condition
                .value(ctx)
                .is_some_and(|condition| condition.test(ctx))
        })
        .map_or(default, |case| &case.value)
}

/// Vanilla `Weighted<T>` in a `WeightedList`.
#[derive(Debug, Clone, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub(super) struct Weighted<T> {
    pub(super) data: Holder<T>,
    #[serde(deserialize_with = "non_negative_int")]
    weight: i32,
}

/// Vanilla `ExtraCodecs.NON_NEGATIVE_INT`.
fn non_negative_int<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i32, D::Error> {
    let value = super::java_int(deserializer)?;
    if value < 0 {
        return Err(D::Error::custom(format!(
            "Value must be non-negative: {value}"
        )));
    }
    Ok(value)
}

/// Vanilla `WeightedList.getRandomOrThrow`: an entry picked with chance
/// proportional to its weight.
pub(super) fn pick_weighted<T>(entries: &[Weighted<T>]) -> ProviderResult<&Holder<T>> {
    let total = entries
        .iter()
        .map(|entry| i64::from(entry.weight))
        .sum::<i64>();
    if total <= 0 {
        return Err(ArithmeticError("Weighted list has no entries".to_owned()));
    }
    let mut roll = rand::rng().random_range(0..total);
    for entry in entries {
        let weight = i64::from(entry.weight);
        if roll < weight {
            return Ok(&entry.data);
        }
        roll -= weight;
    }
    Err(ArithmeticError("Weighted list has no entries".to_owned()))
}

/// Vanilla `ScoreboardNameProvider`: whose score a `score` provider reads.
#[derive(Debug, Clone)]
pub(super) enum ScoreTarget {
    /// `context`, or a bare entity target: the entity parameter's score.
    Context(EntityTarget),
    /// `fixed`: a score holder by name.
    Fixed(String),
}

impl<'de> Deserialize<'de> for ScoreTarget {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "type")]
        enum Typed {
            #[serde(rename = "minecraft:fixed", alias = "fixed")]
            Fixed { name: String },
            #[serde(rename = "minecraft:context", alias = "context")]
            Context { target: EntityTarget },
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Inline(EntityTarget),
            Typed(Typed),
        }
        Ok(match Raw::deserialize(deserializer)? {
            Raw::Inline(target) | Raw::Typed(Typed::Context { target }) => Self::Context(target),
            Raw::Typed(Typed::Fixed { name }) => Self::Fixed(name),
        })
    }
}

impl ScoreTarget {
    /// Vanilla `ScoreboardValue.getScoreInfo`: the holder's score in
    /// `objective`, `None` without a holder, objective or score.
    fn score(&self, objective: &str, ctx: &LootContext<'_>) -> Option<i32> {
        let holder = match self {
            Self::Context(target) => ScoreHolder::new(ctx.entity_for(*target)?.scoreboard_name()),
            Self::Fixed(name) => ScoreHolder::new(name.clone()),
        };
        let scoreboard = ctx.world.scoreboard()?;
        let objective = scoreboard.objective(objective)?;
        scoreboard.score(&holder, &objective)
    }
}

/// Vanilla `StoredNumberAccess`: a number in command storage.
#[derive(Debug, Clone)]
pub(super) struct StoredNumber {
    storage: Identifier,
    path: NbtPath,
}

impl<'de> Deserialize<'de> for StoredNumber {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            storage: String,
            path: String,
        }
        let raw = Raw::deserialize(deserializer)?;
        Ok(Self {
            storage: Identifier::from_str(&raw.storage).map_err(D::Error::custom)?,
            path: parse_nbt_path(&raw.path).map_err(D::Error::custom)?,
        })
    }
}

/// A numeric NBT tag as Java's boxed `Number`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum StoredValue {
    Integer(i64),
    Float(f64),
}

impl StoredValue {
    /// Java `Number.intValue`.
    pub(super) const fn int_value(self) -> i32 {
        match self {
            Self::Integer(value) => value as i32,
            Self::Float(value) => value as i32,
        }
    }

    /// Java `Number.floatValue`.
    pub(super) const fn float_value(self) -> f32 {
        match self {
            Self::Integer(value) => value as f32,
            Self::Float(value) => value as f32,
        }
    }
}

impl StoredNumber {
    /// Vanilla `getNumericTag`: the one numeric tag the path selects, `None`
    /// for no tag, several, or one that is not a number.
    pub(super) fn numeric_tag(&self, ctx: &LootContext<'_>) -> Option<StoredValue> {
        let compound = ctx.storage?.get(&self.storage);
        let tags = self.path.get(&NbtTag::Compound(compound));
        let [tag] = tags.as_slice() else {
            return None;
        };
        Some(match tag {
            NbtTag::Byte(value) => StoredValue::Integer(i64::from(*value)),
            NbtTag::Short(value) => StoredValue::Integer(i64::from(*value)),
            NbtTag::Int(value) => StoredValue::Integer(i64::from(*value)),
            NbtTag::Long(value) => StoredValue::Integer(*value),
            NbtTag::Float(value) => StoredValue::Float(f64::from(*value)),
            NbtTag::Double(value) => StoredValue::Float(*value),
            _ => return None,
        })
    }
}
