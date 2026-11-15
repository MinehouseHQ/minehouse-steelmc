//! Vanilla `Scoreboard` and `ServerScoreboard`, one per Steel domain.
//!
//! Objectives, scores, display slots and teams follow vanilla, and every
//! change reaches the domain's players with vanilla's packets. Steel keeps
//! names in sorted maps where vanilla uses hash maps, so listings come out
//! sorted instead of in hash order.

pub mod criteria;
mod persistence;
mod sync;
pub mod team;
#[cfg(test)]
mod tests;

use std::{
    collections::{BTreeMap, BTreeSet},
    io, mem,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use steel_protocol::packets::game::{CResetScore, CSetScore, NumberFormat, ObjectiveRenderType};
use steel_utils::locks::{AsyncMutex, SyncMutex, SyncRwLock};
use text_components::TextComponent;
use thiserror::Error;

use crate::{
    player::{Player, connection::NetworkConnection as _},
    server::worlds::WorldMap,
    world::World,
};
pub use criteria::{CustomCriteria, DisplaySlot, ObjectiveCriteria, TeamColor};
use persistence::PersistentScoreboard;
use steel_utils::saved_data::names as saved_data_names;
pub use sync::ScoreboardPacket;
use sync::{
    Viewers, display_packet, objective_packet, team_add_or_modify_packet, team_player_packet,
    team_remove_packet,
};
pub use team::{PlayerTeam, TeamProperty};

/// Vanilla `ScoreHolder`: a scoreboard name and, for entities, the display
/// name objectives with `displayAutoUpdate` copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreHolder {
    name: String,
    display_name: Option<TextComponent>,
}

impl ScoreHolder {
    /// Vanilla `ScoreHolder.forNameOnly`.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            display_name: None,
        }
    }

    /// A holder with the display name vanilla's entity `getDisplayName` gives.
    #[must_use]
    pub fn with_display_name(name: impl Into<String>, display_name: TextComponent) -> Self {
        Self {
            name: name.into(),
            display_name: Some(display_name),
        }
    }

    /// Vanilla `getScoreboardName`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Vanilla `getDisplayName`.
    #[must_use]
    pub const fn display_name(&self) -> Option<&TextComponent> {
        self.display_name.as_ref()
    }
}

/// Vanilla `Objective`, as a snapshot of the scoreboard's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreboardObjective {
    name: String,
    criteria: ObjectiveCriteria,
    display_name: TextComponent,
    render_type: ObjectiveRenderType,
    display_auto_update: bool,
    number_format: Option<NumberFormat>,
}

impl ScoreboardObjective {
    /// The name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The criteria.
    #[must_use]
    pub const fn criteria(&self) -> &ObjectiveCriteria {
        &self.criteria
    }

    /// Whether the criteria keeps commands from changing scores.
    #[must_use]
    pub const fn is_read_only(&self) -> bool {
        self.criteria.is_read_only()
    }

    /// The display name.
    #[must_use]
    pub const fn display_name(&self) -> &TextComponent {
        &self.display_name
    }

    /// The render type.
    #[must_use]
    pub const fn render_type(&self) -> ObjectiveRenderType {
        self.render_type
    }

    /// Whether scores copy their holder's display name.
    #[must_use]
    pub const fn display_auto_update(&self) -> bool {
        self.display_auto_update
    }

    /// The number format.
    #[must_use]
    pub const fn number_format(&self) -> Option<&NumberFormat> {
        self.number_format.as_ref()
    }
}

/// One objective property vanilla's setters change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObjectiveProperty {
    /// Sets the display name.
    DisplayName(TextComponent),
    /// Sets the render type.
    RenderType(ObjectiveRenderType),
    /// Sets the display auto update.
    DisplayAutoUpdate(bool),
    /// Sets the number format.
    NumberFormat(Option<NumberFormat>),
}

/// Vanilla `Score`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreboardScore {
    value: i32,
    locked: bool,
    display: Option<TextComponent>,
    number_format: Option<NumberFormat>,
}

impl Default for ScoreboardScore {
    fn default() -> Self {
        Self {
            value: 0,
            locked: true,
            display: None,
            number_format: None,
        }
    }
}

impl ScoreboardScore {
    /// The value.
    #[must_use]
    pub const fn value(&self) -> i32 {
        self.value
    }

    /// Whether `/trigger` may not change it.
    #[must_use]
    pub const fn is_locked(&self) -> bool {
        self.locked
    }

    /// The display that replaces the holder's name.
    #[must_use]
    pub const fn display(&self) -> Option<&TextComponent> {
        self.display.as_ref()
    }

    /// The number format.
    #[must_use]
    pub const fn number_format(&self) -> Option<&NumberFormat> {
        self.number_format.as_ref()
    }

    fn packet(&self, owner: &str, objective: &str) -> CSetScore {
        CSetScore {
            owner: owner.to_owned(),
            objective_name: objective.to_owned(),
            score: self.value,
            display: self.display.clone(),
            number_format: self.number_format.clone(),
        }
    }
}

/// Invalid scoreboard operation.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ScoreboardError {
    /// An objective already exists.
    #[error("objective '{0}' already exists")]
    DuplicateObjective(String),
    /// A team already exists.
    #[error("team '{0}' already exists")]
    DuplicateTeam(String),
    /// The requested objective does not exist.
    #[error("objective '{0}' does not exist")]
    MissingObjective(String),
    /// The requested team does not exist.
    #[error("team '{0}' does not exist")]
    MissingTeam(String),
    /// The objective's criteria is read-only.
    #[error("objective '{0}' is read-only")]
    ReadOnlyObjective(String),
}

#[derive(Clone, Debug, Default)]
struct ScoreboardState {
    objectives: BTreeMap<String, ScoreboardObjective>,
    /// Holder name, then objective name.
    scores: BTreeMap<String, BTreeMap<String, ScoreboardScore>>,
    display: [Option<String>; DisplaySlot::ALL.len()],
    teams: BTreeMap<String, PlayerTeam>,
    teams_by_player: BTreeMap<String, String>,
    /// Vanilla `ServerScoreboard.trackedObjectives`.
    tracked: BTreeSet<String>,
}

impl ScoreboardState {
    fn display_objective_name(&self, slot: DisplaySlot) -> Option<&str> {
        self.display[slot.index()].as_deref()
    }

    fn display_slot_count(&self, objective: &str) -> usize {
        self.display
            .iter()
            .filter(|name| name.as_deref() == Some(objective))
            .count()
    }

    fn objective(&self, name: &str) -> Result<&ScoreboardObjective, ScoreboardError> {
        self.objectives
            .get(name)
            .ok_or_else(|| ScoreboardError::MissingObjective(name.to_owned()))
    }

    /// Vanilla `onScoreChanged`: the score reaches clients that track it.
    fn score_changed(&self, owner: &str, objective: &str, packets: &mut Vec<ScoreboardPacket>) {
        if self.tracked.contains(objective)
            && let Some(score) = self
                .scores
                .get(owner)
                .and_then(|scores| scores.get(objective))
        {
            packets.push(ScoreboardPacket::Score(score.packet(owner, objective)));
        }
    }

    /// Vanilla `startTrackingObjective`.
    fn start_tracking(&mut self, name: &str, packets: &mut Vec<ScoreboardPacket>) {
        if let Some(objective) = self.objectives.get(name) {
            packets.extend(self.start_tracking_packets(objective));
        }
        self.tracked.insert(name.to_owned());
    }

    /// Vanilla `stopTrackingObjective`.
    fn stop_tracking(
        &mut self,
        objective: &ScoreboardObjective,
        packets: &mut Vec<ScoreboardPacket>,
    ) {
        packets.extend(self.stop_tracking_packets(objective));
        self.tracked.remove(&objective.name);
    }

    /// Vanilla `ServerScoreboard.setDisplayObjective`.
    fn set_display_objective(
        &mut self,
        slot: DisplaySlot,
        objective: Option<&ScoreboardObjective>,
        packets: &mut Vec<ScoreboardPacket>,
    ) {
        let new = objective.map(|objective| objective.name.clone());
        let old = mem::replace(&mut self.display[slot.index()], new.clone());
        if let Some(old) = old.filter(|old| new.as_ref() != Some(old)) {
            if self.display_slot_count(&old) > 0 {
                packets.push(display_packet(slot, new.as_deref()));
            } else if let Some(old) = self.objectives.get(&old).cloned() {
                self.stop_tracking(&old, packets);
            } else {
                self.tracked.remove(&old);
            }
        }
        if let Some(new) = new {
            if self.tracked.contains(&new) {
                packets.push(display_packet(slot, Some(&new)));
            } else {
                self.start_tracking(&new, packets);
            }
        }
    }

    /// Vanilla `Scoreboard.removePlayerFromTeam(player, team)` with
    /// `ServerScoreboard`'s leave packet.
    fn leave_team(&mut self, player: &str, packets: &mut Vec<ScoreboardPacket>) -> bool {
        let Some(team) = self.teams_by_player.remove(player) else {
            return false;
        };
        if let Some(team) = self.teams.get_mut(&team) {
            team.players.remove(player);
        }
        packets.push(team_player_packet(&team, player, false));
        true
    }
}

/// One Steel domain's scoreboard.
pub struct Scoreboard {
    state: SyncRwLock<ScoreboardState>,
    viewers: SyncMutex<Viewers>,
    revision: AtomicU64,
    saved_revision: AtomicU64,
}

impl Default for Scoreboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Scoreboard {
    /// Creates an empty, clean scoreboard.
    #[must_use]
    pub fn new() -> Self {
        Self::from_state(ScoreboardState::default())
    }

    fn from_state(state: ScoreboardState) -> Self {
        Self {
            state: SyncRwLock::new(state),
            viewers: SyncMutex::new(Viewers::default()),
            revision: AtomicU64::new(0),
            saved_revision: AtomicU64::new(0),
        }
    }

    /// Runs a mutation, then marks the scoreboard dirty when it says so and
    /// sends its packets to every viewer after the state lock is released.
    fn mutate<T>(
        &self,
        change: impl FnOnce(&mut ScoreboardState, &mut Vec<ScoreboardPacket>) -> (T, bool),
    ) -> T {
        let mut packets = Vec::new();
        let (result, dirty) = change(&mut self.state.write(), &mut packets);
        if dirty {
            self.revision.fetch_add(1, Ordering::Release);
        }
        self.broadcast(&packets);
        result
    }

    fn broadcast(&self, packets: &[ScoreboardPacket]) {
        if packets.is_empty() {
            return;
        }
        let viewers = self.viewers.lock().online();
        for player in viewers {
            for packet in packets {
                packet.send_to(&player);
            }
        }
    }

    /// Vanilla `PlayerList.updateEntireScoreboard` for a player entering the
    /// domain, who then sees every change.
    pub fn connect_player(&self, player: &Arc<Player>) {
        let packets = self.state.read().entire_scoreboard_packets();
        self.viewers.lock().add(player);
        for packet in &packets {
            packet.send_to(player);
        }
    }

    /// Stops sending changes to a player. A player still connected, who is
    /// moving to another domain, also forgets this scoreboard.
    pub fn disconnect_player(&self, player: &Player) {
        self.viewers.lock().remove(player);
        if !player.connection.closed() {
            let packets = self.state.read().leave_packets();
            for packet in &packets {
                packet.send_to(player);
            }
        }
    }

    /// Vanilla `addObjective`.
    ///
    /// # Errors
    ///
    /// Returns an error if the objective already exists.
    pub fn add_objective(
        &self,
        name: impl Into<String>,
        criteria: ObjectiveCriteria,
        display_name: TextComponent,
        render_type: ObjectiveRenderType,
        display_auto_update: bool,
        number_format: Option<NumberFormat>,
    ) -> Result<ScoreboardObjective, ScoreboardError> {
        let objective = ScoreboardObjective {
            name: name.into(),
            criteria,
            display_name,
            render_type,
            display_auto_update,
            number_format,
        };
        self.mutate(|state, _| {
            if state.objectives.contains_key(&objective.name) {
                return (
                    Err(ScoreboardError::DuplicateObjective(objective.name)),
                    false,
                );
            }
            state
                .objectives
                .insert(objective.name.clone(), objective.clone());
            (Ok(objective), true)
        })
    }

    /// A `dummy` objective whose display name is its name, as
    /// `/scoreboard objectives add <name> dummy` makes.
    ///
    /// # Errors
    ///
    /// Returns an error if the objective already exists.
    pub fn add_dummy_objective(
        &self,
        name: impl Into<String>,
    ) -> Result<ScoreboardObjective, ScoreboardError> {
        let name = name.into();
        let display_name = TextComponent::plain(name.clone());
        self.add_objective(
            name,
            ObjectiveCriteria::DUMMY,
            display_name,
            ObjectiveRenderType::Integer,
            false,
            None,
        )
    }

    /// Vanilla `getObjective`.
    #[must_use]
    pub fn objective(&self, name: &str) -> Option<ScoreboardObjective> {
        self.state.read().objectives.get(name).cloned()
    }

    /// Vanilla `getObjectives`.
    #[must_use]
    pub fn objectives(&self) -> Vec<ScoreboardObjective> {
        self.state.read().objectives.values().cloned().collect()
    }

    /// Vanilla `getObjectiveNames`.
    #[must_use]
    pub fn objective_names(&self) -> Vec<String> {
        self.state.read().objectives.keys().cloned().collect()
    }

    /// The objectives with a criteria, as vanilla's `objectivesByCriteria`.
    #[must_use]
    pub fn objectives_with_criteria(
        &self,
        criteria: &ObjectiveCriteria,
    ) -> Vec<ScoreboardObjective> {
        self.state
            .read()
            .objectives
            .values()
            .filter(|objective| &objective.criteria == criteria)
            .cloned()
            .collect()
    }

    /// Vanilla `Objective`'s setters with `ServerScoreboard.onObjectiveChanged`.
    ///
    /// # Errors
    ///
    /// Returns an error if the objective does not exist.
    pub fn modify_objective(
        &self,
        name: &str,
        property: ObjectiveProperty,
    ) -> Result<(), ScoreboardError> {
        self.mutate(|state, packets| {
            let Some(objective) = state.objectives.get_mut(name) else {
                return (
                    Err(ScoreboardError::MissingObjective(name.to_owned())),
                    false,
                );
            };
            match property {
                ObjectiveProperty::DisplayName(display_name) => {
                    objective.display_name = display_name;
                }
                ObjectiveProperty::RenderType(render_type) => objective.render_type = render_type,
                ObjectiveProperty::DisplayAutoUpdate(update) => {
                    objective.display_auto_update = update;
                }
                ObjectiveProperty::NumberFormat(format) => objective.number_format = format,
            }
            if state.tracked.contains(name) {
                packets.push(objective_packet(&state.objectives[name], 2));
            }
            (Ok(()), true)
        })
    }

    /// Vanilla `removeObjective` with `ServerScoreboard`'s packets.
    pub fn remove_objective(&self, name: &str) {
        self.mutate(|state, packets| {
            let Some(objective) = state.objectives.remove(name) else {
                return ((), false);
            };
            for slot in DisplaySlot::ALL {
                if state.display_objective_name(slot) == Some(name) {
                    state.set_display_objective(slot, None, packets);
                }
            }
            for scores in state.scores.values_mut() {
                scores.remove(name);
            }
            if state.tracked.contains(name) {
                state.stop_tracking(&objective, packets);
            }
            ((), true)
        });
    }

    /// Vanilla `setDisplayObjective`.
    ///
    /// # Errors
    ///
    /// Returns an error if the objective does not exist.
    pub fn set_display_objective(
        &self,
        slot: DisplaySlot,
        objective: Option<&str>,
    ) -> Result<(), ScoreboardError> {
        self.mutate(|state, packets| {
            let objective = match objective {
                Some(name) => match state.objective(name) {
                    Ok(objective) => Some(objective.clone()),
                    Err(error) => return (Err(error), false),
                },
                None => None,
            };
            state.set_display_objective(slot, objective.as_ref(), packets);
            (Ok(()), true)
        })
    }

    /// Vanilla `getDisplayObjective`.
    #[must_use]
    pub fn display_objective(&self, slot: DisplaySlot) -> Option<ScoreboardObjective> {
        let state = self.state.read();
        state
            .display_objective_name(slot)
            .and_then(|name| state.objectives.get(name).cloned())
    }

    /// Vanilla `getPlayerScoreInfo`.
    #[must_use]
    pub fn score_entry(
        &self,
        holder: &ScoreHolder,
        objective: &ScoreboardObjective,
    ) -> Option<ScoreboardScore> {
        self.state
            .read()
            .scores
            .get(holder.name())
            .and_then(|scores| scores.get(objective.name()).cloned())
    }

    /// The score value from [`Self::score_entry`].
    #[must_use]
    pub fn score(&self, holder: &ScoreHolder, objective: &ScoreboardObjective) -> Option<i32> {
        self.score_entry(holder, objective).map(|score| score.value)
    }

    /// Vanilla `ScoreAccess.set` on `getOrCreatePlayerScore(holder, objective)`.
    ///
    /// # Errors
    ///
    /// Returns an error if the objective does not exist or its criteria is
    /// read-only.
    pub fn set_score(
        &self,
        holder: &ScoreHolder,
        objective: &ScoreboardObjective,
        value: i32,
    ) -> Result<(), ScoreboardError> {
        self.update_score(holder, objective.name(), false, |_| value)
            .map(|_| ())
    }

    /// Vanilla `ScoreAccess.add` on a forced-writable score, as criteria
    /// tracking (`forAllObjectives`) changes read-only ones too.
    ///
    /// # Errors
    ///
    /// Returns an error if the objective does not exist.
    pub fn add_score_forced(
        &self,
        holder: &ScoreHolder,
        objective: &str,
        amount: i32,
    ) -> Result<i32, ScoreboardError> {
        self.update_score(holder, objective, true, |old| old.wrapping_add(amount))
    }

    /// Vanilla `ScoreAccess.set` on a forced-writable score.
    ///
    /// # Errors
    ///
    /// Returns an error if the objective does not exist.
    pub fn set_score_forced(
        &self,
        holder: &ScoreHolder,
        objective: &str,
        value: i32,
    ) -> Result<(), ScoreboardError> {
        self.update_score(holder, objective, true, |_| value)
            .map(|_| ())
    }

    /// Vanilla `getOrCreatePlayerScore(...).set(value(old))`; returns the new value.
    fn update_score(
        &self,
        holder: &ScoreHolder,
        objective: &str,
        force_writable: bool,
        value: impl FnOnce(i32) -> i32,
    ) -> Result<i32, ScoreboardError> {
        self.mutate(|state, packets| {
            let objective = match state.objective(objective) {
                Ok(objective) => objective.clone(),
                Err(error) => return (Err(error), false),
            };
            if !force_writable && objective.is_read_only() {
                return (
                    Err(ScoreboardError::ReadOnlyObjective(objective.name)),
                    false,
                );
            }
            let scores = state.scores.entry(holder.name().to_owned()).or_default();
            let created = !scores.contains_key(objective.name());
            let score = scores.entry(objective.name.clone()).or_default();
            let mut changed = created;
            if objective.display_auto_update
                && let Some(display) = holder.display_name()
                && score.display.as_ref() != Some(display)
            {
                score.display = Some(display.clone());
                changed = true;
            }
            let new_value = value(score.value);
            if new_value != score.value {
                score.value = new_value;
                changed = true;
            }
            if changed {
                state.score_changed(holder.name(), objective.name(), packets);
            }
            (Ok(new_value), changed)
        })
    }

    /// Vanilla `ScoreAccess.lock` and `unlock`.
    ///
    /// # Errors
    ///
    /// Returns an error if the objective does not exist.
    pub fn set_score_locked(
        &self,
        holder: &ScoreHolder,
        objective: &ScoreboardObjective,
        locked: bool,
    ) -> Result<(), ScoreboardError> {
        self.mutate(|state, packets| {
            if let Err(error) = state.objective(objective.name()) {
                return (Err(error), false);
            }
            let scores = state.scores.entry(holder.name().to_owned()).or_default();
            let created = !scores.contains_key(objective.name());
            scores
                .entry(objective.name().to_owned())
                .or_default()
                .locked = locked;
            if created {
                state.score_changed(holder.name(), objective.name(), packets);
            }
            (Ok(()), true)
        })
    }

    /// Vanilla `ScoreAccess.display`.
    ///
    /// # Errors
    ///
    /// Returns an error if the objective does not exist.
    pub fn set_score_display(
        &self,
        holder: &ScoreHolder,
        objective: &ScoreboardObjective,
        display: Option<TextComponent>,
    ) -> Result<(), ScoreboardError> {
        self.mutate(|state, packets| {
            if let Err(error) = state.objective(objective.name()) {
                return (Err(error), false);
            }
            let scores = state.scores.entry(holder.name().to_owned()).or_default();
            let created = !scores.contains_key(objective.name());
            let score = scores.entry(objective.name().to_owned()).or_default();
            if !created && score.display == display {
                return (Ok(()), false);
            }
            score.display = display;
            state.score_changed(holder.name(), objective.name(), packets);
            (Ok(()), true)
        })
    }

    /// Vanilla `ScoreAccess.numberFormatOverride`.
    ///
    /// # Errors
    ///
    /// Returns an error if the objective does not exist.
    pub fn set_score_number_format(
        &self,
        holder: &ScoreHolder,
        objective: &ScoreboardObjective,
        number_format: Option<NumberFormat>,
    ) -> Result<(), ScoreboardError> {
        self.mutate(|state, packets| {
            if let Err(error) = state.objective(objective.name()) {
                return (Err(error), false);
            }
            state
                .scores
                .entry(holder.name().to_owned())
                .or_default()
                .entry(objective.name().to_owned())
                .or_default()
                .number_format = number_format;
            state.score_changed(holder.name(), objective.name(), packets);
            (Ok(()), true)
        })
    }

    /// Vanilla `listPlayerScores(objective)`: holder names and their scores.
    #[must_use]
    pub fn objective_scores(
        &self,
        objective: &ScoreboardObjective,
    ) -> Vec<(String, ScoreboardScore)> {
        self.state
            .read()
            .scores
            .iter()
            .filter_map(|(holder, scores)| {
                scores
                    .get(objective.name())
                    .map(|score| (holder.clone(), score.clone()))
            })
            .collect()
    }

    /// Vanilla `listPlayerScores(holder)`: each objective and score value.
    #[must_use]
    pub fn holder_scores(&self, holder: &ScoreHolder) -> Vec<(ScoreboardObjective, i32)> {
        let state = self.state.read();
        state
            .scores
            .get(holder.name())
            .into_iter()
            .flatten()
            .filter_map(|(objective, score)| {
                state
                    .objectives
                    .get(objective)
                    .map(|objective| (objective.clone(), score.value))
            })
            .collect()
    }

    /// Objective names that have a score for the holder.
    #[must_use]
    pub fn holder_objectives(&self, holder: &ScoreHolder) -> BTreeSet<String> {
        self.state
            .read()
            .scores
            .get(holder.name())
            .map_or_else(BTreeSet::new, |scores| scores.keys().cloned().collect())
    }

    /// Vanilla `getTrackedPlayers`.
    #[must_use]
    pub fn tracked_holders(&self) -> Vec<ScoreHolder> {
        self.state
            .read()
            .scores
            .keys()
            .map(|name| ScoreHolder::new(name.clone()))
            .collect()
    }

    /// Vanilla `resetAllPlayerScores` with `onPlayerRemoved`.
    pub fn reset_all_scores(&self, holder: &ScoreHolder) {
        self.mutate(|state, packets| {
            if state.scores.remove(holder.name()).is_none() {
                return ((), false);
            }
            if !state.tracked.is_empty() {
                packets.push(ScoreboardPacket::Reset(CResetScore {
                    owner: holder.name().to_owned(),
                    objective_name: None,
                }));
            }
            ((), true)
        });
    }

    /// Vanilla `resetSinglePlayerScore`.
    pub fn reset_score(&self, holder: &ScoreHolder, objective: &ScoreboardObjective) {
        let emptied = self.mutate(|state, packets| {
            let Some(scores) = state.scores.get_mut(holder.name()) else {
                return (false, false);
            };
            let removed = scores.remove(objective.name()).is_some();
            if scores.is_empty() {
                return (true, false);
            }
            if removed && state.tracked.contains(objective.name()) {
                packets.push(ScoreboardPacket::Reset(CResetScore {
                    owner: holder.name().to_owned(),
                    objective_name: Some(objective.name().to_owned()),
                }));
            }
            (false, removed)
        });
        if emptied {
            self.reset_all_scores(holder);
        }
    }

    /// Vanilla `addPlayerTeam`.
    ///
    /// # Errors
    ///
    /// Returns an error if the team already exists.
    pub fn add_team(&self, name: impl Into<String>) -> Result<PlayerTeam, ScoreboardError> {
        let team = PlayerTeam::new(name);
        self.mutate(|state, packets| {
            if state.teams.contains_key(team.name()) {
                return (
                    Err(ScoreboardError::DuplicateTeam(team.name().to_owned())),
                    false,
                );
            }
            state.teams.insert(team.name().to_owned(), team.clone());
            packets.push(team_add_or_modify_packet(&team, true));
            (Ok(team), true)
        })
    }

    /// Vanilla `getPlayerTeam`.
    #[must_use]
    pub fn team(&self, name: &str) -> Option<PlayerTeam> {
        self.state.read().teams.get(name).cloned()
    }

    /// Vanilla `getPlayerTeams`.
    #[must_use]
    pub fn teams(&self) -> Vec<PlayerTeam> {
        self.state.read().teams.values().cloned().collect()
    }

    /// Vanilla `getTeamNames`.
    #[must_use]
    pub fn team_names(&self) -> Vec<String> {
        self.state.read().teams.keys().cloned().collect()
    }

    /// Vanilla `removePlayerTeam`.
    pub fn remove_team(&self, name: &str) {
        self.mutate(|state, packets| {
            let Some(team) = state.teams.remove(name) else {
                return ((), false);
            };
            for player in &team.players {
                state.teams_by_player.remove(player);
            }
            packets.push(team_remove_packet(name));
            ((), true)
        });
    }

    /// Vanilla's team setters with `ServerScoreboard.onTeamChanged`.
    ///
    /// # Errors
    ///
    /// Returns an error if the team does not exist.
    pub fn modify_team(&self, name: &str, property: TeamProperty) -> Result<(), ScoreboardError> {
        self.mutate(|state, packets| {
            let Some(team) = state.teams.get_mut(name) else {
                return (Err(ScoreboardError::MissingTeam(name.to_owned())), false);
            };
            team.apply(property);
            packets.push(team_add_or_modify_packet(team, false));
            (Ok(()), true)
        })
    }

    /// Vanilla `getPlayersTeam`.
    #[must_use]
    pub fn holder_team_name(&self, holder: &ScoreHolder) -> Option<String> {
        self.state
            .read()
            .teams_by_player
            .get(holder.name())
            .cloned()
    }

    /// Vanilla `getPlayersTeam`, the whole team.
    #[must_use]
    pub fn holder_team(&self, holder: &str) -> Option<PlayerTeam> {
        let state = self.state.read();
        state
            .teams_by_player
            .get(holder)
            .and_then(|team| state.teams.get(team).cloned())
    }

    /// Vanilla `addPlayerToTeam`: leaves any other team first, and is
    /// `false` when the holder already was a member.
    ///
    /// # Errors
    ///
    /// Returns an error if the team does not exist.
    pub fn add_holder_to_team(
        &self,
        holder: &ScoreHolder,
        team: &str,
    ) -> Result<bool, ScoreboardError> {
        self.mutate(|state, packets| {
            if !state.teams.contains_key(team) {
                return (Err(ScoreboardError::MissingTeam(team.to_owned())), false);
            }
            let left = state.teams_by_player.contains_key(holder.name())
                && state.leave_team(holder.name(), packets);
            state
                .teams_by_player
                .insert(holder.name().to_owned(), team.to_owned());
            let added = state
                .teams
                .get_mut(team)
                .is_some_and(|team| team.players.insert(holder.name().to_owned()));
            if added {
                packets.push(team_player_packet(team, holder.name(), true));
            }
            (Ok(added), added || left)
        })
    }

    /// Vanilla `removePlayerFromTeam(player)`.
    pub fn remove_holder_from_team(&self, holder: &str) -> bool {
        self.mutate(|state, packets| {
            let left = state.leave_team(holder, packets);
            (left, left)
        })
    }

    /// Vanilla `Scoreboard.entityRemoved`: a dead non-player entity loses its
    /// scores and team.
    pub fn entity_removed(&self, holder: &ScoreHolder, is_player: bool, is_alive: bool) {
        if !is_player && !is_alive {
            self.reset_all_scores(holder);
            self.remove_holder_from_team(holder.name());
        }
    }

    fn pending_save(&self) -> Option<(u64, PersistentScoreboard)> {
        let state = self.state.read();
        let revision = self.revision.load(Ordering::Acquire);
        if revision == self.saved_revision.load(Ordering::Acquire) {
            return None;
        }
        Some((revision, PersistentScoreboard::pack(&state)))
    }

    fn mark_saved(&self, revision: u64) {
        self.saved_revision.fetch_max(revision, Ordering::Release);
    }
}

/// Loaded scoreboards keyed by Steel domain.
pub struct DomainScoreboards {
    scoreboards: BTreeMap<String, Scoreboard>,
    save_lock: AsyncMutex<()>,
}

impl DomainScoreboards {
    /// Loads one scoreboard through each domain's default world saved-data boundary.
    pub async fn load(worlds: &WorldMap) -> io::Result<Self> {
        let mut domains = worlds.domain_names().collect::<Vec<_>>();
        domains.sort_unstable();
        let mut scoreboards = BTreeMap::new();
        for domain in domains {
            let world = domain_default_world(worlds, domain)?;
            let persistent: PersistentScoreboard = world
                .saved_data
                .load_or_default(saved_data_names::SCOREBOARD)
                .await
                .map_err(|error| scoreboard_io_error(domain, error))?;
            let state = persistent.unpack().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid scoreboard for domain '{domain}': {error}"),
                )
            })?;
            scoreboards.insert(domain.to_owned(), Scoreboard::from_state(state));
        }
        Ok(Self {
            scoreboards,
            save_lock: AsyncMutex::new(()),
        })
    }

    /// Returns the scoreboard for a domain.
    #[must_use]
    pub fn get(&self, domain: &str) -> Option<&Scoreboard> {
        self.scoreboards.get(domain)
    }

    /// Saves every dirty domain scoreboard and returns the number written.
    pub async fn save(&self, worlds: &WorldMap) -> io::Result<usize> {
        let _save_guard = self.save_lock.lock().await;
        let mut saved = 0;
        for (domain, scoreboard) in &self.scoreboards {
            let Some((revision, state)) = scoreboard.pending_save() else {
                continue;
            };
            let world = domain_default_world(worlds, domain)?;
            world
                .saved_data
                .save(saved_data_names::SCOREBOARD, &state)
                .await
                .map_err(|error| scoreboard_io_error(domain, error))?;
            scoreboard.mark_saved(revision);
            saved += 1;
        }
        Ok(saved)
    }
}

fn domain_default_world<'a>(worlds: &'a WorldMap, domain: &str) -> io::Result<&'a World> {
    worlds
        .default_world(domain)
        .map(AsRef::as_ref)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("domain '{domain}' has no loaded default world"),
            )
        })
}

fn scoreboard_io_error(domain: &str, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("scoreboard I/O failed for domain '{domain}': {error}"),
    )
}
