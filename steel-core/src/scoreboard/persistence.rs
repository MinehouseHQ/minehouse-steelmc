//! Vanilla `ScoreboardSaveData.Packed` in Steel's TOML saved data.
//!
//! Keys follow vanilla's codec. Text components are kept as JSON strings, as
//! Steel's other saved data does. The first Steel format (lowercase keys,
//! objectives with only a read-only flag) still loads; its objectives become
//! `dummy` objectives named after themselves.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use steel_protocol::packets::game::{
    NumberFormat, ObjectiveRenderType, TeamCollisionRule, TeamVisibility,
};
use text_components::TextComponent;

use super::{
    DisplaySlot, ObjectiveCriteria, PlayerTeam, ScoreboardObjective, ScoreboardScore,
    ScoreboardState, TeamColor,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct PersistentScoreboard {
    #[serde(rename = "Objectives", skip_serializing_if = "Vec::is_empty")]
    objectives: Vec<PackedObjective>,
    #[serde(rename = "PlayerScores", skip_serializing_if = "Vec::is_empty")]
    player_scores: Vec<PackedPlayerScore>,
    #[serde(rename = "DisplaySlots", skip_serializing_if = "BTreeMap::is_empty")]
    display_slots: BTreeMap<String, String>,
    #[serde(rename = "Teams", skip_serializing_if = "Vec::is_empty")]
    teams: Vec<PackedTeam>,
    /// The first Steel format.
    #[serde(rename = "objectives", skip_serializing)]
    legacy_objectives: BTreeMap<String, LegacyObjective>,
    #[serde(rename = "scores", skip_serializing)]
    legacy_scores: BTreeMap<String, BTreeMap<String, LegacyScore>>,
    #[serde(rename = "teams", skip_serializing)]
    legacy_teams: BTreeSet<String>,
    #[serde(rename = "holder_teams", skip_serializing)]
    legacy_holder_teams: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackedObjective {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "CriteriaName", default = "dummy_criteria")]
    criteria_name: String,
    #[serde(rename = "DisplayName")]
    display_name: String,
    #[serde(rename = "RenderType", default = "integer_render_type")]
    render_type: String,
    #[serde(rename = "display_auto_update", default)]
    display_auto_update: bool,
    #[serde(rename = "format", default, skip_serializing_if = "Option::is_none")]
    number_format: Option<PackedNumberFormat>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackedPlayerScore {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Objective")]
    objective: String,
    #[serde(rename = "Score", default)]
    value: i32,
    #[serde(rename = "Locked", default)]
    locked: bool,
    #[serde(rename = "display", default, skip_serializing_if = "Option::is_none")]
    display: Option<String>,
    #[serde(rename = "format", default, skip_serializing_if = "Option::is_none")]
    number_format: Option<PackedNumberFormat>,
}

/// Vanilla `NumberFormatTypes.CODEC`: `blank`, `styled` with the style and
/// `fixed` with a component. Steel keeps a style as an empty component.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackedNumberFormat {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackedTeam {
    #[serde(rename = "Name")]
    name: String,
    #[serde(
        rename = "DisplayName",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    display_name: Option<String>,
    #[serde(rename = "TeamColor", default, skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    #[serde(rename = "AllowFriendlyFire", default = "default_true")]
    allow_friendly_fire: bool,
    #[serde(rename = "SeeFriendlyInvisibles", default = "default_true")]
    see_friendly_invisibles: bool,
    #[serde(
        rename = "MemberNamePrefix",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    prefix: Option<String>,
    #[serde(
        rename = "MemberNameSuffix",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    suffix: Option<String>,
    #[serde(rename = "NameTagVisibility", default = "always")]
    name_tag_visibility: String,
    #[serde(rename = "DeathMessageVisibility", default = "always")]
    death_message_visibility: String,
    #[serde(rename = "CollisionRule", default = "always")]
    collision_rule: String,
    #[serde(rename = "Players", default, skip_serializing_if = "Vec::is_empty")]
    players: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LegacyObjective {
    read_only: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LegacyScore {
    value: i32,
    locked: bool,
}

impl Default for LegacyScore {
    fn default() -> Self {
        Self {
            value: 0,
            locked: true,
        }
    }
}

fn dummy_criteria() -> String {
    ObjectiveCriteria::DUMMY.name()
}

fn integer_render_type() -> String {
    ObjectiveRenderType::Integer.name().to_owned()
}

const fn default_true() -> bool {
    true
}

fn always() -> String {
    TeamVisibility::Always.name().to_owned()
}

fn component_to_json(component: &TextComponent) -> String {
    serde_json::to_string(component).unwrap_or_else(|_| String::from("\"\""))
}

fn component_from_json(json: &str, what: &str) -> Result<TextComponent, String> {
    serde_json::from_str(json).map_err(|error| format!("invalid {what}: {error}"))
}

fn pack_number_format(format: &NumberFormat) -> PackedNumberFormat {
    let (kind, value) = match format {
        NumberFormat::Blank => ("blank", None),
        NumberFormat::Styled(style) => ("styled", Some(component_to_json(style))),
        NumberFormat::Fixed(value) => ("fixed", Some(component_to_json(value))),
    };
    PackedNumberFormat {
        kind: kind.to_owned(),
        value,
    }
}

fn unpack_number_format(packed: &PackedNumberFormat) -> Result<NumberFormat, String> {
    let value = || {
        packed
            .value
            .as_deref()
            .ok_or_else(|| format!("number format '{}' has no value", packed.kind))
            .and_then(|json| component_from_json(json, "number format"))
    };
    match packed.kind.as_str() {
        "blank" => Ok(NumberFormat::Blank),
        "styled" => value().map(NumberFormat::Styled),
        "fixed" => value().map(NumberFormat::Fixed),
        kind => Err(format!("unknown number format '{kind}'")),
    }
}

impl PersistentScoreboard {
    /// Vanilla `ScoreboardSaveData.pack`.
    pub(super) fn pack(state: &ScoreboardState) -> Self {
        let objectives = state
            .objectives
            .values()
            .map(|objective| PackedObjective {
                name: objective.name.clone(),
                criteria_name: objective.criteria.name(),
                display_name: component_to_json(&objective.display_name),
                render_type: objective.render_type.name().to_owned(),
                display_auto_update: objective.display_auto_update,
                number_format: objective.number_format.as_ref().map(pack_number_format),
            })
            .collect();
        let player_scores = state
            .scores
            .iter()
            .flat_map(|(holder, scores)| {
                scores.iter().map(|(objective, score)| PackedPlayerScore {
                    name: holder.clone(),
                    objective: objective.clone(),
                    value: score.value,
                    locked: score.locked,
                    display: score.display.as_ref().map(component_to_json),
                    number_format: score.number_format.as_ref().map(pack_number_format),
                })
            })
            .collect();
        let display_slots = DisplaySlot::ALL
            .into_iter()
            .filter_map(|slot| {
                state
                    .display_objective_name(slot)
                    .map(|objective| (slot.name(), objective.to_owned()))
            })
            .collect();
        let teams = state.teams.values().map(pack_team).collect();
        Self {
            objectives,
            player_scores,
            display_slots,
            teams,
            ..Self::default()
        }
    }

    /// Vanilla `ScoreboardSaveData.loadFrom`.
    pub(super) fn unpack(self) -> Result<ScoreboardState, String> {
        let mut state = ScoreboardState::default();
        self.unpack_legacy(&mut state)?;
        for packed in self.objectives {
            let criteria = ObjectiveCriteria::by_name(&packed.criteria_name)
                .ok_or_else(|| format!("unknown criteria '{}'", packed.criteria_name))?;
            let objective = ScoreboardObjective {
                display_name: component_from_json(&packed.display_name, "objective name")?,
                render_type: ObjectiveRenderType::by_name(&packed.render_type)
                    .ok_or_else(|| format!("unknown render type '{}'", packed.render_type))?,
                display_auto_update: packed.display_auto_update,
                number_format: packed
                    .number_format
                    .as_ref()
                    .map(unpack_number_format)
                    .transpose()?,
                criteria,
                name: packed.name,
            };
            state.objectives.insert(objective.name.clone(), objective);
        }
        for packed in self.player_scores {
            if !state.objectives.contains_key(&packed.objective) {
                // Vanilla logs an unknown objective and skips the score.
                continue;
            }
            let score = ScoreboardScore {
                value: packed.value,
                locked: packed.locked,
                display: packed
                    .display
                    .as_deref()
                    .map(|json| component_from_json(json, "score display"))
                    .transpose()?,
                number_format: packed
                    .number_format
                    .as_ref()
                    .map(unpack_number_format)
                    .transpose()?,
            };
            state
                .scores
                .entry(packed.name)
                .or_default()
                .insert(packed.objective, score);
        }
        for packed in self.teams {
            let team = unpack_team(packed)?;
            for player in &team.players {
                state
                    .teams_by_player
                    .insert(player.clone(), team.name.clone());
            }
            state.teams.insert(team.name.clone(), team);
        }
        for (slot, objective) in self.display_slots {
            let slot = DisplaySlot::by_name(&slot)
                .ok_or_else(|| format!("unknown display slot '{slot}'"))?;
            if state.objectives.contains_key(&objective) {
                state.display[slot.index()] = Some(objective);
            }
        }
        Ok(state)
    }

    fn unpack_legacy(&self, state: &mut ScoreboardState) -> Result<(), String> {
        for name in self.legacy_objectives.keys() {
            state.objectives.insert(
                name.clone(),
                ScoreboardObjective {
                    name: name.clone(),
                    criteria: ObjectiveCriteria::DUMMY,
                    display_name: TextComponent::plain(name.clone()),
                    render_type: ObjectiveRenderType::Integer,
                    display_auto_update: false,
                    number_format: None,
                },
            );
        }
        for (holder, scores) in &self.legacy_scores {
            for (objective, score) in scores {
                if !state.objectives.contains_key(objective) {
                    return Err(format!(
                        "score of '{holder}' for unknown objective '{objective}'"
                    ));
                }
                state.scores.entry(holder.clone()).or_default().insert(
                    objective.clone(),
                    ScoreboardScore {
                        value: score.value,
                        locked: score.locked,
                        display: None,
                        number_format: None,
                    },
                );
            }
        }
        for name in &self.legacy_teams {
            state
                .teams
                .insert(name.clone(), PlayerTeam::new(name.clone()));
        }
        for (holder, team) in &self.legacy_holder_teams {
            let Some(entry) = state.teams.get_mut(team) else {
                return Err(format!("'{holder}' is in unknown team '{team}'"));
            };
            entry.players.insert(holder.clone());
            state.teams_by_player.insert(holder.clone(), team.clone());
        }
        Ok(())
    }
}

/// Vanilla `PlayerTeam.pack`, which leaves out defaults.
fn pack_team(team: &PlayerTeam) -> PackedTeam {
    let empty = TextComponent::new();
    let text =
        |component: &TextComponent| (component != &empty).then(|| component_to_json(component));
    PackedTeam {
        name: team.name.clone(),
        display_name: Some(component_to_json(&team.display_name)),
        color: team.color.map(|color| color.name().to_owned()),
        allow_friendly_fire: team.allow_friendly_fire,
        see_friendly_invisibles: team.see_friendly_invisibles,
        prefix: text(&team.player_prefix),
        suffix: text(&team.player_suffix),
        name_tag_visibility: team.name_tag_visibility.name().to_owned(),
        death_message_visibility: team.death_message_visibility.name().to_owned(),
        collision_rule: team.collision_rule.name().to_owned(),
        players: team.players.iter().cloned().collect(),
    }
}

/// Vanilla `ScoreboardSaveData.loadTeam`.
fn unpack_team(packed: PackedTeam) -> Result<PlayerTeam, String> {
    let visibility = |name: &str| {
        TeamVisibility::by_name(name).ok_or_else(|| format!("unknown team visibility '{name}'"))
    };
    let mut team = PlayerTeam::new(packed.name);
    if let Some(display_name) = &packed.display_name {
        team.display_name = component_from_json(display_name, "team name")?;
    }
    if let Some(color) = &packed.color {
        team.color =
            Some(TeamColor::by_name(color).ok_or_else(|| format!("unknown team color '{color}'"))?);
    }
    team.allow_friendly_fire = packed.allow_friendly_fire;
    team.see_friendly_invisibles = packed.see_friendly_invisibles;
    if let Some(prefix) = &packed.prefix {
        team.player_prefix = component_from_json(prefix, "team prefix")?;
    }
    if let Some(suffix) = &packed.suffix {
        team.player_suffix = component_from_json(suffix, "team suffix")?;
    }
    team.name_tag_visibility = visibility(&packed.name_tag_visibility)?;
    team.death_message_visibility = visibility(&packed.death_message_visibility)?;
    team.collision_rule = TeamCollisionRule::by_name(&packed.collision_rule)
        .ok_or_else(|| format!("unknown collision rule '{}'", packed.collision_rule))?;
    team.players.extend(packed.players);
    Ok(team)
}
