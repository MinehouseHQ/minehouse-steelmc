//! Vanilla `LootItemCondition` and every condition type vanilla registers in
//! `LootItemConditionTypes`.

use std::collections::BTreeMap;
use std::str::FromStr as _;

use glam::DVec3;

use rand::RngExt as _;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use steel_registry::loot_table::EntityEquipmentRef;
use steel_registry::{REGISTRY, RegistryExt as _};
use steel_utils::Identifier;

use super::level_value::LevelBasedValue;
use super::ranges::{FloatRange, IntRange};
use super::{
    ContextFloatProvider, ContextIntProvider, EntityTarget, Holder, HolderSet, LootContext,
    LootElement, LootRegistryKind, References, java_bool, java_bool_opt,
};
use crate::entity::loot_equipment;
use crate::player::advancement::criteria::DamageSourcePredicate;
use crate::player::advancement::criteria::predicates::{
    BlockCondition, EntityPredicate, ItemCondition, LocationPredicate,
};
use crate::scoreboard::ScoreHolder;

/// A vanilla loot condition (`LootItemCondition`).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum LootCondition {
    /// `inverted`: the term fails.
    #[serde(rename = "minecraft:inverted", alias = "inverted")]
    Inverted { term: Holder<LootCondition> },
    /// `any_of`: one of the terms passes.
    #[serde(rename = "minecraft:any_of", alias = "any_of")]
    AnyOf { terms: HolderSet<LootCondition> },
    /// `all_of`: every term passes.
    #[serde(rename = "minecraft:all_of", alias = "all_of")]
    AllOf { terms: HolderSet<LootCondition> },
    /// `random_chance`: passes with the provider's probability.
    #[serde(rename = "minecraft:random_chance", alias = "random_chance")]
    RandomChance {
        chance: Holder<ContextFloatProvider>,
    },
    /// `random_chance_with_enchanted_bonus`: a chance raised by the
    /// attacker's enchantment level.
    #[serde(
        rename = "minecraft:random_chance_with_enchanted_bonus",
        alias = "random_chance_with_enchanted_bonus"
    )]
    RandomChanceWithEnchantedBonus {
        unenchanted_chance: f32,
        enchanted_chance: LevelBasedValue,
        #[serde(deserialize_with = "enchantment")]
        enchantment: Identifier,
    },
    /// `entity_properties`: the entity matches the predicate.
    #[serde(rename = "minecraft:entity_properties", alias = "entity_properties")]
    EntityProperties {
        entity: EntityTarget,
        predicate: Option<EntityPredicate>,
    },
    /// `killed_by_player`: a player dealt the last damage.
    #[serde(rename = "minecraft:killed_by_player", alias = "killed_by_player")]
    KilledByPlayer {},
    /// `entity_scores`: the entity's scores are in range.
    #[serde(rename = "minecraft:entity_scores", alias = "entity_scores")]
    EntityScores {
        scores: BTreeMap<String, IntRange>,
        entity: EntityTarget,
    },
    /// `match_block`: the block state matches.
    #[serde(rename = "minecraft:match_block", alias = "match_block")]
    MatchBlock(Box<BlockCondition>),
    /// `match_tool`: the tool matches.
    #[serde(rename = "minecraft:match_tool", alias = "match_tool")]
    MatchTool {
        predicate: Option<Box<ItemCondition>>,
    },
    /// `table_bonus`: a chance picked by the tool's enchantment level.
    #[serde(rename = "minecraft:table_bonus", alias = "table_bonus")]
    TableBonus {
        #[serde(deserialize_with = "enchantment")]
        enchantment: Identifier,
        #[serde(deserialize_with = "non_empty_chances")]
        chances: Vec<f32>,
    },
    /// `survives_explosion`: passes with chance `1 / radius`.
    #[serde(rename = "minecraft:survives_explosion", alias = "survives_explosion")]
    SurvivesExplosion {},
    /// `damage_source_properties`: the damage source matches.
    #[serde(
        rename = "minecraft:damage_source_properties",
        alias = "damage_source_properties"
    )]
    DamageSourceProperties {
        predicate: Option<Box<DamageSourcePredicate>>,
    },
    /// `location_check`: the origin, offset, matches.
    #[serde(rename = "minecraft:location_check", alias = "location_check")]
    LocationCheck {
        #[serde(default, rename = "offsetX", deserialize_with = "super::java_int")]
        offset_x: i32,
        #[serde(default, rename = "offsetY", deserialize_with = "super::java_int")]
        offset_y: i32,
        #[serde(default, rename = "offsetZ", deserialize_with = "super::java_int")]
        offset_z: i32,
        predicate: Option<Box<LocationPredicate>>,
    },
    /// `weather_check`: the level's weather matches.
    #[serde(rename = "minecraft:weather_check", alias = "weather_check")]
    WeatherCheck {
        #[serde(default, deserialize_with = "java_bool_opt")]
        raining: Option<bool>,
        #[serde(default, deserialize_with = "java_bool_opt")]
        thundering: Option<bool>,
    },
    /// `time_check`: a world clock's time is in range.
    #[serde(rename = "minecraft:time_check", alias = "time_check")]
    TimeCheck {
        #[serde(deserialize_with = "world_clock")]
        clock: Identifier,
        period: Option<i64>,
        value: IntRange,
    },
    /// `int_value_check`: an int provider is in range.
    #[serde(rename = "minecraft:int_value_check", alias = "int_value_check")]
    IntValueCheck {
        value: Holder<ContextIntProvider>,
        test: IntRange,
    },
    /// `float_value_check`: a float provider is in range.
    #[serde(rename = "minecraft:float_value_check", alias = "float_value_check")]
    FloatValueCheck {
        value: Holder<ContextFloatProvider>,
        test: FloatRange,
    },
    /// `enchantment_active_check`: the enchantment's active state matches.
    #[serde(
        rename = "minecraft:enchantment_active_check",
        alias = "enchantment_active_check"
    )]
    EnchantmentActiveCheck {
        #[serde(deserialize_with = "java_bool")]
        active: bool,
    },
    /// `environment_attribute_check`: an environment attribute equals the
    /// value.
    #[serde(
        rename = "minecraft:environment_attribute_check",
        alias = "environment_attribute_check"
    )]
    EnvironmentAttributeCheck { attribute: Identifier, value: Value },
}

/// Vanilla `Enchantment.CODEC`: the id of a registered enchantment.
fn enchantment<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Identifier, D::Error> {
    let id = registered_id(deserializer)?;
    if REGISTRY.enchantments.by_key(&id).is_none() {
        return Err(D::Error::custom(format!(
            "Unknown registry key in ResourceKey[minecraft:root / minecraft:enchantment]: {id}"
        )));
    }
    Ok(id)
}

/// Vanilla `WorldClock.CODEC`: the id of a registered world clock.
fn world_clock<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Identifier, D::Error> {
    let id = registered_id(deserializer)?;
    if REGISTRY.world_clocks.by_key(&id).is_none() {
        return Err(D::Error::custom(format!(
            "Unknown registry key in ResourceKey[minecraft:root / minecraft:world_clock]: {id}"
        )));
    }
    Ok(id)
}

fn registered_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Identifier, D::Error> {
    let id = String::deserialize(deserializer)?;
    Identifier::from_str(&id).map_err(D::Error::custom)
}

/// Vanilla `ExtraCodecs.nonEmptyList(Codec.FLOAT.listOf())`.
fn non_empty_chances<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<f32>, D::Error> {
    let chances = Vec::<f32>::deserialize(deserializer)?;
    if chances.is_empty() {
        return Err(D::Error::custom("List must have contents"));
    }
    Ok(chances)
}

impl LootElement for LootCondition {
    const KIND: LootRegistryKind = LootRegistryKind::Predicate;
    const TAG_KIND: LootRegistryKind = LootRegistryKind::PredicateTag;

    fn references(&self, out: &mut References) {
        match self {
            Self::Inverted { term } => term.references(out),
            Self::AnyOf { terms } | Self::AllOf { terms } => terms.references(out),
            Self::RandomChance { chance } => chance.references(out),
            Self::EntityScores { scores, .. } => {
                for range in scores.values() {
                    range.references(out);
                }
            }
            Self::TimeCheck { value, .. } => value.references(out),
            Self::IntValueCheck { value, test } => {
                value.references(out);
                test.references(out);
            }
            Self::FloatValueCheck { value, test } => {
                value.references(out);
                test.references(out);
            }
            Self::RandomChanceWithEnchantedBonus { .. }
            | Self::EntityProperties { .. }
            | Self::KilledByPlayer {}
            | Self::MatchBlock(_)
            | Self::MatchTool { .. }
            | Self::TableBonus { .. }
            | Self::SurvivesExplosion {}
            | Self::DamageSourceProperties { .. }
            | Self::LocationCheck { .. }
            | Self::WeatherCheck { .. }
            | Self::EnchantmentActiveCheck { .. }
            | Self::EnvironmentAttributeCheck { .. } => {}
        }
    }
}

impl LootCondition {
    /// Vanilla `LootItemCondition.test(context)`.
    pub(crate) fn test(&self, ctx: &LootContext<'_>) -> bool {
        match self {
            Self::Inverted { term } => !test_holder(term, ctx),
            Self::AnyOf { terms } => terms
                .values(ctx)
                .is_some_and(|terms| terms.iter().any(|term| term.test(ctx))),
            Self::AllOf { terms } => terms
                .values(ctx)
                .is_some_and(|terms| terms.iter().all(|term| term.test(ctx))),
            Self::RandomChance { chance } => {
                let chance = chance
                    .value(ctx)
                    .map_or(0.0, |chance| chance.get_float(ctx));
                rand::rng().random::<f32>() < chance
            }
            Self::RandomChanceWithEnchantedBonus {
                unenchanted_chance,
                enchanted_chance,
                enchantment,
            } => {
                let level = attacker_enchantment_level(ctx, enchantment);
                let chance = if level > 0 {
                    enchanted_chance.calculate(level)
                } else {
                    *unenchanted_chance
                };
                rand::rng().random::<f32>() < chance
            }
            Self::EntityProperties { entity, predicate } => {
                // Vanilla `LootItemEntityPropertyCondition`: an empty predicate
                // passes even without the entity.
                let Some(predicate) = predicate else {
                    return true;
                };
                ctx.entity_for(*entity)
                    .is_some_and(|entity| predicate.matches(ctx.world, ctx.origin, entity))
            }
            Self::KilledByPlayer {} => ctx.attacking_player.is_some(),
            Self::EntityScores { scores, entity } => entity_scores_match(scores, *entity, ctx),
            Self::MatchBlock(block) => ctx
                .block_state
                .is_some_and(|state| block.matches_state(state) && block.matches_block_entity()),
            Self::MatchTool { predicate } => ctx.tool.is_some_and(|tool| {
                predicate
                    .as_ref()
                    .is_none_or(|predicate| predicate.test(tool))
            }),
            Self::TableBonus {
                enchantment,
                chances,
            } => table_bonus(enchantment, chances, ctx),
            Self::SurvivesExplosion {} => ctx
                .explosion_radius
                .is_none_or(|radius| rand::rng().random::<f32>() <= 1.0 / radius),
            Self::DamageSourceProperties { predicate } => {
                let (Some(origin), Some(source)) = (ctx.origin, ctx.damage_source) else {
                    return false;
                };
                predicate
                    .as_ref()
                    .is_none_or(|predicate| predicate.matches(ctx.world, origin, source))
            }
            Self::LocationCheck {
                offset_x,
                offset_y,
                offset_z,
                predicate,
            } => ctx.origin.is_some_and(|origin| {
                predicate.as_ref().is_none_or(|predicate| {
                    let offset = DVec3::new(
                        f64::from(*offset_x),
                        f64::from(*offset_y),
                        f64::from(*offset_z),
                    );
                    let pos = origin + offset;
                    predicate.matches(ctx.world, pos.x, pos.y, pos.z)
                })
            }),
            Self::WeatherCheck {
                raining,
                thundering,
            } => {
                raining.is_none_or(|raining| raining == ctx.world.is_raining())
                    && thundering.is_none_or(|thundering| thundering == ctx.world.is_thundering())
            }
            Self::TimeCheck {
                clock,
                period,
                value,
            } => time_check(clock, *period, value, ctx),
            Self::IntValueCheck { value, test } => value
                .value(ctx)
                .is_some_and(|value| test.test(ctx, value.get_int(ctx))),
            Self::FloatValueCheck { value, test } => value
                .value(ctx)
                .is_some_and(|value| test.test(ctx, value.get_float(ctx))),
            Self::EnchantmentActiveCheck { active } => ctx.enchantment_active == Some(*active),
            // TODO: Compare the attribute once Steel has vanilla's environment
            // attribute system; until then the check never passes.
            Self::EnvironmentAttributeCheck { attribute, value } => {
                log::debug!("Environment attribute check {attribute} = {value} is not supported");
                false
            }
        }
    }
}

/// Tests the condition a holder names; an unresolved reference fails.
fn test_holder(holder: &Holder<LootCondition>, ctx: &LootContext<'_>) -> bool {
    holder
        .value(ctx)
        .is_some_and(|condition| condition.test(ctx))
}

/// Vanilla `EnchantmentHelper.getEnchantmentLevel(enchantment, attacker)`:
/// the highest level on the attacker's equipment, 0 for a non-living one.
fn attacker_enchantment_level(ctx: &LootContext<'_>, enchantment: &Identifier) -> i32 {
    ctx.attacker.and_then(loot_equipment).map_or(0, |slots| {
        EntityEquipmentRef::new(&slots).get_enchantment_level(enchantment)
    })
}

/// Vanilla `EntityHasScoreCondition.test`: every objective exists and the
/// entity's score in it is in range.
/// Vanilla `BonusLevelTableCondition`: the chance at the tool's enchantment
/// level, capped at the last entry.
fn table_bonus(enchantment: &Identifier, chances: &[f32], ctx: &LootContext<'_>) -> bool {
    let level = ctx
        .tool
        .map_or(0, |tool| tool.get_enchantment_level(enchantment));
    let index = usize::try_from(level)
        .unwrap_or(0)
        .min(chances.len().saturating_sub(1));
    let chance = chances.get(index).copied().unwrap_or(0.0);
    rand::rng().random::<f32>() < chance
}

fn entity_scores_match(
    scores: &BTreeMap<String, IntRange>,
    target: EntityTarget,
    ctx: &LootContext<'_>,
) -> bool {
    let Some(entity) = ctx.entity_for(target) else {
        return false;
    };
    let Some(scoreboard) = ctx.world.scoreboard() else {
        return false;
    };
    let holder = ScoreHolder::new(entity.scoreboard_name());
    scores.iter().all(|(objective, range)| {
        scoreboard
            .objective(objective)
            .and_then(|objective| scoreboard.score(&holder, &objective))
            .is_some_and(|score| range.test(ctx, score))
    })
}

/// Vanilla `TimeCheck.test`: the clock's total ticks, modulo the period,
/// narrowed to an int.
fn time_check(
    clock: &Identifier,
    period: Option<i64>,
    value: &IntRange,
    ctx: &LootContext<'_>,
) -> bool {
    let Some(clock) = REGISTRY.world_clocks.by_key(clock) else {
        return false;
    };
    let Some(mut time) = ctx.world.clock_total_ticks(clock) else {
        return false;
    };
    if let Some(period) = period {
        let Some(rem) = time.checked_rem(period) else {
            return false;
        };
        time = rem;
    }
    // Java's `(int)` cast keeps the low 32 bits.
    value.test(ctx, time as i32)
}

/// A field holding loot conditions in an advancement criterion: one
/// condition, or a list that must all pass (vanilla `ContextAwarePredicate`).
#[derive(Debug, Clone)]
pub(crate) struct ContextPredicate(Vec<LootCondition>);

impl ContextPredicate {
    /// Tests every condition in the context.
    pub(crate) fn test(&self, ctx: &LootContext<'_>) -> bool {
        self.0.iter().all(|condition| condition.test(ctx))
    }
}

impl<'de> Deserialize<'de> for ContextPredicate {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Many(Vec<LootCondition>),
            One(Box<LootCondition>),
        }
        Ok(match Raw::deserialize(deserializer)? {
            Raw::Many(conditions) => Self(conditions),
            Raw::One(condition) => Self(vec![*condition]),
        })
    }
}
