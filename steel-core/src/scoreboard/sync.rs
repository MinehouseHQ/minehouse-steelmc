//! Vanilla `ServerScoreboard`'s client side: which players see a domain
//! scoreboard and the packets that keep them in sync.

use std::{
    ptr,
    sync::{Arc, Weak},
};

use steel_protocol::packets::game::{
    CResetScore, CSetDisplayObjective, CSetObjective, CSetPlayerTeam, CSetScore, ObjectiveMethod,
    TeamMethod,
};

use super::{DisplaySlot, ScoreboardObjective, ScoreboardState, team::PlayerTeam};
use crate::player::Player;

/// One scoreboard packet waiting to be sent once the state lock is released.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "packets live only until the state lock is released"
)]
pub enum ScoreboardPacket {
    /// `ClientboundSetObjectivePacket`.
    Objective(CSetObjective),
    /// `ClientboundSetDisplayObjectivePacket`.
    Display(CSetDisplayObjective),
    /// `ClientboundSetScorePacket`.
    Score(CSetScore),
    /// `ClientboundResetScorePacket`.
    Reset(CResetScore),
    /// `ClientboundSetPlayerTeamPacket`.
    Team(CSetPlayerTeam),
}

impl ScoreboardPacket {
    pub(super) fn send_to(&self, player: &Player) {
        match self {
            Self::Objective(packet) => player.send_packet(packet.clone()),
            Self::Display(packet) => player.send_packet(packet.clone()),
            Self::Score(packet) => player.send_packet(packet.clone()),
            Self::Reset(packet) => player.send_packet(packet.clone()),
            Self::Team(packet) => player.send_packet(packet.clone()),
        }
    }
}

/// Vanilla `ClientboundSetObjectivePacket(objective, method)`.
pub(super) fn objective_packet(objective: &ScoreboardObjective, method: i32) -> ScoreboardPacket {
    let method = match method {
        0 => ObjectiveMethod::Add {
            display_name: objective.display_name.clone(),
            render_type: objective.render_type,
            number_format: objective.number_format.clone(),
        },
        2 => ObjectiveMethod::Change {
            display_name: objective.display_name.clone(),
            render_type: objective.render_type,
            number_format: objective.number_format.clone(),
        },
        _ => ObjectiveMethod::Remove,
    };
    ScoreboardPacket::Objective(CSetObjective {
        objective_name: objective.name.clone(),
        method,
    })
}

/// Vanilla `ClientboundSetDisplayObjectivePacket(slot, objective)`.
pub(super) fn display_packet(slot: DisplaySlot, objective: Option<&str>) -> ScoreboardPacket {
    ScoreboardPacket::Display(CSetDisplayObjective {
        slot: slot.id(),
        objective_name: objective.unwrap_or_default().to_owned(),
    })
}

/// Vanilla `createAddOrModifyPacket`.
pub(super) fn team_add_or_modify_packet(team: &PlayerTeam, create_new: bool) -> ScoreboardPacket {
    let method = if create_new {
        TeamMethod::Add(team.parameters(), team.players.iter().cloned().collect())
    } else {
        TeamMethod::Change(team.parameters())
    };
    ScoreboardPacket::Team(CSetPlayerTeam {
        name: team.name.clone(),
        method,
    })
}

/// Vanilla `createRemovePacket`.
pub(super) fn team_remove_packet(team: &str) -> ScoreboardPacket {
    ScoreboardPacket::Team(CSetPlayerTeam {
        name: team.to_owned(),
        method: TeamMethod::Remove,
    })
}

/// Vanilla `createPlayerPacket` with `ADD` (`join`) or `REMOVE`.
pub(super) fn team_player_packet(team: &str, player: &str, join: bool) -> ScoreboardPacket {
    let players = vec![player.to_owned()];
    ScoreboardPacket::Team(CSetPlayerTeam {
        name: team.to_owned(),
        method: if join {
            TeamMethod::Join(players)
        } else {
            TeamMethod::Leave(players)
        },
    })
}

impl ScoreboardState {
    /// Vanilla `getStartTrackingPackets`.
    pub(super) fn start_tracking_packets(
        &self,
        objective: &ScoreboardObjective,
    ) -> Vec<ScoreboardPacket> {
        let mut packets = vec![objective_packet(objective, 0)];
        for slot in DisplaySlot::ALL {
            if self.display_objective_name(slot) == Some(objective.name.as_str()) {
                packets.push(display_packet(slot, Some(&objective.name)));
            }
        }
        for (owner, scores) in &self.scores {
            if let Some(score) = scores.get(&objective.name) {
                packets.push(ScoreboardPacket::Score(
                    score.packet(owner, &objective.name),
                ));
            }
        }
        packets
    }

    /// Vanilla `getStopTrackingPackets`.
    pub(super) fn stop_tracking_packets(
        &self,
        objective: &ScoreboardObjective,
    ) -> Vec<ScoreboardPacket> {
        let mut packets = vec![objective_packet(objective, 1)];
        for slot in DisplaySlot::ALL {
            if self.display_objective_name(slot) == Some(objective.name.as_str()) {
                packets.push(display_packet(slot, Some(&objective.name)));
            }
        }
        packets
    }

    /// Vanilla `PlayerList.updateEntireScoreboard`: every team, then each
    /// displayed objective once.
    pub(super) fn entire_scoreboard_packets(&self) -> Vec<ScoreboardPacket> {
        let mut packets: Vec<_> = self
            .teams
            .values()
            .map(|team| team_add_or_modify_packet(team, true))
            .collect();
        let mut sent = Vec::new();
        for slot in DisplaySlot::ALL {
            let Some(name) = self.display_objective_name(slot) else {
                continue;
            };
            if sent.contains(&name) {
                continue;
            }
            if let Some(objective) = self.objectives.get(name) {
                packets.extend(self.start_tracking_packets(objective));
            }
            sent.push(name);
        }
        packets
    }

    /// What a client must forget when it leaves this scoreboard's domain:
    /// every displayed objective and every team.
    ///
    /// Vanilla never moves a connected player to another scoreboard. Steel
    /// domains each have their own, and a domain switch keeps the
    /// connection, so the old one is taken down before the new one is sent.
    pub(super) fn leave_packets(&self) -> Vec<ScoreboardPacket> {
        let mut packets: Vec<_> = self
            .tracked
            .iter()
            .filter_map(|name| self.objectives.get(name))
            .map(|objective| objective_packet(objective, 1))
            .collect();
        packets.extend(self.teams.keys().map(|team| team_remove_packet(team)));
        packets
    }
}

/// The online players of one domain who see its scoreboard.
#[derive(Default)]
pub(super) struct Viewers {
    players: Vec<Weak<Player>>,
}

impl Viewers {
    pub(super) fn add(&mut self, player: &Arc<Player>) {
        self.players.retain(|viewer| {
            viewer
                .upgrade()
                .is_some_and(|viewer| !Arc::ptr_eq(&viewer, player))
        });
        self.players.push(Arc::downgrade(player));
    }

    pub(super) fn remove(&mut self, player: &Player) {
        self.players.retain(|viewer| {
            viewer
                .upgrade()
                .is_some_and(|viewer| !ptr::eq(Arc::as_ptr(&viewer), player))
        });
    }

    pub(super) fn online(&self) -> Vec<Arc<Player>> {
        self.players.iter().filter_map(Weak::upgrade).collect()
    }
}
