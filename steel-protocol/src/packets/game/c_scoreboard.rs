//! Vanilla scoreboard packets: objectives, display slots, scores and teams.

use std::io::{Result, Write};

use simdnbt::owned::{NbtCompound, NbtTag};
use steel_macros::ClientPacket;
use steel_registry::packets::play::{
    C_RESET_SCORE, C_SET_DISPLAY_OBJECTIVE, C_SET_OBJECTIVE, C_SET_PLAYER_TEAM, C_SET_SCORE,
};
use steel_utils::{
    codec::VarInt,
    serial::{PrefixedWrite as _, WriteTo},
};
use text_components::TextComponent;

/// Vanilla `ObjectiveCriteria.RenderType`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ObjectiveRenderType {
    /// `integer`
    #[default]
    Integer,
    /// `hearts`
    Hearts,
}

impl ObjectiveRenderType {
    /// Vanilla's values, in network order.
    pub const ALL: [Self; 2] = [Self::Integer, Self::Hearts];

    /// The serialized name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Integer => "integer",
            Self::Hearts => "hearts",
        }
    }

    /// Vanilla `RenderType.byId`'s exact lookup.
    #[must_use]
    pub fn by_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|render_type| render_type.name() == name)
    }
}

/// Vanilla `NumberFormat`: how the client shows a score.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NumberFormat {
    /// Vanilla `BlankFormat`: no number.
    Blank,
    /// Vanilla `StyledFormat`: the number in a style, carried as the style of
    /// an empty text component.
    Styled(TextComponent),
    /// Vanilla `FixedFormat`: a fixed component instead of the number.
    Fixed(TextComponent),
}

impl NumberFormat {
    /// Vanilla `NUMBER_FORMAT_TYPE` registry id.
    const fn type_id(&self) -> i32 {
        match self {
            Self::Blank => 0,
            Self::Styled(_) => 1,
            Self::Fixed(_) => 2,
        }
    }
}

/// Vanilla `Style.Serializer` NBT: a component's style fields without its
/// content.
#[must_use]
pub fn style_nbt(style: &TextComponent) -> NbtCompound {
    match style.to_codec_nbt() {
        NbtTag::Compound(mut compound) => {
            compound.remove("text");
            compound.remove("type");
            compound
        }
        _ => NbtCompound::new(),
    }
}

impl WriteTo for NumberFormat {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        VarInt(self.type_id()).write(writer)?;
        match self {
            Self::Blank => Ok(()),
            Self::Styled(style) => WriteTo::write(&NbtTag::Compound(style_nbt(style)), writer),
            Self::Fixed(value) => value.write(writer),
        }
    }
}

/// `ClientboundSetObjectivePacket` methods.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObjectiveMethod {
    /// `METHOD_ADD`.
    Add {
        /// Objective display name.
        display_name: TextComponent,
        /// How scores render.
        render_type: ObjectiveRenderType,
        /// The objective's number format.
        number_format: Option<NumberFormat>,
    },
    /// `METHOD_REMOVE`.
    Remove,
    /// `METHOD_CHANGE`.
    Change {
        /// Objective display name.
        display_name: TextComponent,
        /// How scores render.
        render_type: ObjectiveRenderType,
        /// The objective's number format.
        number_format: Option<NumberFormat>,
    },
}

/// Adds, removes or changes a scoreboard objective on the client.
#[derive(ClientPacket, Clone, Debug, PartialEq, Eq)]
#[packet_id(Play = C_SET_OBJECTIVE)]
pub struct CSetObjective {
    /// Objective name.
    pub objective_name: String,
    /// What happens to it.
    pub method: ObjectiveMethod,
}

impl WriteTo for CSetObjective {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.objective_name.write_prefixed::<VarInt>(writer)?;
        let (method, fields) = match &self.method {
            ObjectiveMethod::Add {
                display_name,
                render_type,
                number_format,
            } => (0_i8, Some((display_name, render_type, number_format))),
            ObjectiveMethod::Remove => (1, None),
            ObjectiveMethod::Change {
                display_name,
                render_type,
                number_format,
            } => (2, Some((display_name, render_type, number_format))),
        };
        method.write(writer)?;
        if let Some((display_name, render_type, number_format)) = fields {
            display_name.write(writer)?;
            VarInt(*render_type as i32).write(writer)?;
            number_format.write(writer)?;
        }
        Ok(())
    }
}

/// Shows an objective in a display slot, or clears the slot.
#[derive(ClientPacket, Clone, Debug, PartialEq, Eq)]
#[packet_id(Play = C_SET_DISPLAY_OBJECTIVE)]
pub struct CSetDisplayObjective {
    /// Vanilla `DisplaySlot` id.
    pub slot: i32,
    /// The objective, empty to clear the slot.
    pub objective_name: String,
}

impl WriteTo for CSetDisplayObjective {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        VarInt(self.slot).write(writer)?;
        self.objective_name.write_prefixed::<VarInt>(writer)
    }
}

/// Sets one score on the client.
#[derive(ClientPacket, Clone, Debug, PartialEq, Eq)]
#[packet_id(Play = C_SET_SCORE)]
pub struct CSetScore {
    /// Score holder name.
    pub owner: String,
    /// Objective name.
    pub objective_name: String,
    /// Score value.
    pub score: i32,
    /// Display name override.
    pub display: Option<TextComponent>,
    /// Number format override.
    pub number_format: Option<NumberFormat>,
}

impl WriteTo for CSetScore {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.owner.write_prefixed::<VarInt>(writer)?;
        self.objective_name.write_prefixed::<VarInt>(writer)?;
        VarInt(self.score).write(writer)?;
        self.display.write(writer)?;
        self.number_format.write(writer)
    }
}

/// Removes one score, or every score of a holder, on the client.
#[derive(ClientPacket, Clone, Debug, PartialEq, Eq)]
#[packet_id(Play = C_RESET_SCORE)]
pub struct CResetScore {
    /// Score holder name.
    pub owner: String,
    /// The objective, `None` for all of them.
    pub objective_name: Option<String>,
}

impl WriteTo for CResetScore {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.owner.write_prefixed::<VarInt>(writer)?;
        match &self.objective_name {
            Some(name) => {
                true.write(writer)?;
                name.write_prefixed::<VarInt>(writer)
            }
            None => false.write(writer),
        }
    }
}

/// Vanilla `Team.Visibility`, in network id order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TeamVisibility {
    /// `always`
    #[default]
    Always,
    /// `never`
    Never,
    /// `hideForOtherTeams`
    HideForOtherTeams,
    /// `hideForOwnTeam`
    HideForOwnTeam,
}

impl TeamVisibility {
    /// Vanilla's values, in id order.
    pub const ALL: [Self; 4] = [
        Self::Always,
        Self::Never,
        Self::HideForOtherTeams,
        Self::HideForOwnTeam,
    ];

    /// The serialized name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Never => "never",
            Self::HideForOtherTeams => "hideForOtherTeams",
            Self::HideForOwnTeam => "hideForOwnTeam",
        }
    }

    /// Looks a value up by its serialized name.
    #[must_use]
    pub fn by_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.name() == name)
    }
}

/// Vanilla `Team.CollisionRule`, in network id order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TeamCollisionRule {
    /// `always`
    #[default]
    Always,
    /// `never`
    Never,
    /// `pushOtherTeams`
    PushOtherTeams,
    /// `pushOwnTeam`
    PushOwnTeam,
}

impl TeamCollisionRule {
    /// Vanilla's values, in id order.
    pub const ALL: [Self; 4] = [
        Self::Always,
        Self::Never,
        Self::PushOtherTeams,
        Self::PushOwnTeam,
    ];

    /// The serialized name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Never => "never",
            Self::PushOtherTeams => "pushOtherTeams",
            Self::PushOwnTeam => "pushOwnTeam",
        }
    }

    /// Looks a value up by its serialized name.
    #[must_use]
    pub fn by_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.name() == name)
    }
}

/// Vanilla `ClientboundSetPlayerTeamPacket.Parameters`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TeamParameters {
    /// Team display name.
    pub display_name: TextComponent,
    /// Member name prefix.
    pub player_prefix: TextComponent,
    /// Member name suffix.
    pub player_suffix: TextComponent,
    /// Name tag visibility.
    pub name_tag_visibility: TeamVisibility,
    /// Collision rule.
    pub collision_rule: TeamCollisionRule,
    /// Vanilla `TeamColor` id, if the team has a color.
    pub color: Option<i32>,
    /// Vanilla `PlayerTeam.packOptions`.
    pub options: u8,
}

impl WriteTo for TeamParameters {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.display_name.write(writer)?;
        self.player_prefix.write(writer)?;
        self.player_suffix.write(writer)?;
        VarInt(self.name_tag_visibility as i32).write(writer)?;
        VarInt(self.collision_rule as i32).write(writer)?;
        self.color.map(VarInt).write(writer)?;
        self.options.write(writer)
    }
}

/// `ClientboundSetPlayerTeamPacket` methods.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TeamMethod {
    /// `METHOD_ADD`: a new team and its players.
    Add(TeamParameters, Vec<String>),
    /// `METHOD_REMOVE`.
    Remove,
    /// `METHOD_CHANGE`.
    Change(TeamParameters),
    /// `METHOD_JOIN`.
    Join(Vec<String>),
    /// `METHOD_LEAVE`.
    Leave(Vec<String>),
}

/// Adds, removes or changes a team, or its players, on the client.
#[derive(ClientPacket, Clone, Debug, PartialEq, Eq)]
#[packet_id(Play = C_SET_PLAYER_TEAM)]
pub struct CSetPlayerTeam {
    /// Team name.
    pub name: String,
    /// What happens to it.
    pub method: TeamMethod,
}

impl WriteTo for CSetPlayerTeam {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.name.write_prefixed::<VarInt>(writer)?;
        match &self.method {
            TeamMethod::Add(parameters, players) => {
                0_i8.write(writer)?;
                parameters.write(writer)?;
                write_players(players, writer)
            }
            TeamMethod::Remove => 1_i8.write(writer),
            TeamMethod::Change(parameters) => {
                2_i8.write(writer)?;
                parameters.write(writer)
            }
            TeamMethod::Join(players) => {
                3_i8.write(writer)?;
                write_players(players, writer)
            }
            TeamMethod::Leave(players) => {
                4_i8.write(writer)?;
                write_players(players, writer)
            }
        }
    }
}

fn write_players(players: &[String], writer: &mut impl Write) -> Result<()> {
    VarInt(i32::try_from(players.len()).unwrap_or(i32::MAX)).write(writer)?;
    for player in players {
        player.write_prefixed::<VarInt>(writer)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(packet: &impl WriteTo) -> Vec<u8> {
        let mut bytes = Vec::new();
        packet.write(&mut bytes).expect("writing to a vec succeeds");
        bytes
    }

    #[test]
    fn remove_objective_has_no_fields_after_the_method() {
        let packet = CSetObjective {
            objective_name: "kills".to_owned(),
            method: ObjectiveMethod::Remove,
        };
        assert_eq!(bytes(&packet), [5, b'k', b'i', b'l', b'l', b's', 1]);
    }

    #[test]
    fn reset_score_writes_a_nullable_objective() {
        let all = CResetScore {
            owner: "a".to_owned(),
            objective_name: None,
        };
        assert_eq!(bytes(&all), [1, b'a', 0]);
        let one = CResetScore {
            owner: "a".to_owned(),
            objective_name: Some("b".to_owned()),
        };
        assert_eq!(bytes(&one), [1, b'a', 1, 1, b'b']);
    }

    #[test]
    fn team_join_and_remove_follow_vanilla_methods() {
        let join = CSetPlayerTeam {
            name: "t".to_owned(),
            method: TeamMethod::Join(vec!["p".to_owned()]),
        };
        assert_eq!(bytes(&join), [1, b't', 3, 1, 1, b'p']);
        let remove = CSetPlayerTeam {
            name: "t".to_owned(),
            method: TeamMethod::Remove,
        };
        assert_eq!(bytes(&remove), [1, b't', 1]);
    }

    #[test]
    fn display_slot_and_blank_number_format_are_var_ints() {
        let display = CSetDisplayObjective {
            slot: 1,
            objective_name: String::new(),
        };
        assert_eq!(bytes(&display), [1, 0]);
        assert_eq!(bytes(&NumberFormat::Blank), [0]);
    }
}
