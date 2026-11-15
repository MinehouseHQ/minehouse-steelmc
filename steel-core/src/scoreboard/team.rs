//! Vanilla `PlayerTeam`.

use std::collections::BTreeSet;

use steel_protocol::packets::game::{TeamCollisionRule, TeamParameters, TeamVisibility};
use steel_utils::translations;
use text_components::interactivity::HoverEvent;
use text_components::{Modifier as _, TextComponent};

use super::criteria::TeamColor;

/// Vanilla `PlayerTeam`, as the scoreboard stores it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerTeam {
    pub(super) name: String,
    pub(super) players: BTreeSet<String>,
    pub(super) display_name: TextComponent,
    pub(super) player_prefix: TextComponent,
    pub(super) player_suffix: TextComponent,
    pub(super) allow_friendly_fire: bool,
    pub(super) see_friendly_invisibles: bool,
    pub(super) name_tag_visibility: TeamVisibility,
    pub(super) death_message_visibility: TeamVisibility,
    pub(super) color: Option<TeamColor>,
    pub(super) collision_rule: TeamCollisionRule,
}

/// One property `/team modify` and save data can set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TeamProperty {
    /// Sets the display name.
    DisplayName(TextComponent),
    /// Sets the color.
    Color(Option<TeamColor>),
    /// Sets whether members may hurt each other.
    AllowFriendlyFire(bool),
    /// Sets whether members see invisible members.
    SeeFriendlyInvisibles(bool),
    /// Sets the player prefix.
    PlayerPrefix(TextComponent),
    /// Sets the player suffix.
    PlayerSuffix(TextComponent),
    /// Sets the name tag visibility.
    NameTagVisibility(TeamVisibility),
    /// Sets the death message visibility.
    DeathMessageVisibility(TeamVisibility),
    /// Sets the collision rule.
    CollisionRule(TeamCollisionRule),
}

impl PlayerTeam {
    /// Vanilla `new PlayerTeam(scoreboard, name)`.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            display_name: TextComponent::plain(name.clone()),
            name,
            players: BTreeSet::new(),
            player_prefix: TextComponent::new(),
            player_suffix: TextComponent::new(),
            allow_friendly_fire: true,
            see_friendly_invisibles: true,
            name_tag_visibility: TeamVisibility::Always,
            death_message_visibility: TeamVisibility::Always,
            color: None,
            collision_rule: TeamCollisionRule::Always,
        }
    }

    /// Applies one property, like vanilla's setters.
    pub(super) fn apply(&mut self, property: TeamProperty) {
        match property {
            TeamProperty::DisplayName(name) => self.display_name = name,
            TeamProperty::Color(color) => self.color = color,
            TeamProperty::AllowFriendlyFire(allow) => self.allow_friendly_fire = allow,
            TeamProperty::SeeFriendlyInvisibles(see) => self.see_friendly_invisibles = see,
            TeamProperty::PlayerPrefix(prefix) => self.player_prefix = prefix,
            TeamProperty::PlayerSuffix(suffix) => self.player_suffix = suffix,
            TeamProperty::NameTagVisibility(visibility) => self.name_tag_visibility = visibility,
            TeamProperty::DeathMessageVisibility(visibility) => {
                self.death_message_visibility = visibility;
            }
            TeamProperty::CollisionRule(rule) => self.collision_rule = rule,
        }
    }

    /// The name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The members' scoreboard names.
    #[must_use]
    pub const fn players(&self) -> &BTreeSet<String> {
        &self.players
    }

    /// The display name.
    #[must_use]
    pub const fn display_name(&self) -> &TextComponent {
        &self.display_name
    }

    /// The player prefix.
    #[must_use]
    pub const fn player_prefix(&self) -> &TextComponent {
        &self.player_prefix
    }

    /// The player suffix.
    #[must_use]
    pub const fn player_suffix(&self) -> &TextComponent {
        &self.player_suffix
    }

    /// Vanilla `isAllowFriendlyFire`.
    #[must_use]
    pub const fn allows_friendly_fire(&self) -> bool {
        self.allow_friendly_fire
    }

    /// Vanilla `canSeeFriendlyInvisibles`.
    #[must_use]
    pub const fn can_see_friendly_invisibles(&self) -> bool {
        self.see_friendly_invisibles
    }

    /// The name tag visibility.
    #[must_use]
    pub const fn name_tag_visibility(&self) -> TeamVisibility {
        self.name_tag_visibility
    }

    /// The death message visibility.
    #[must_use]
    pub const fn death_message_visibility(&self) -> TeamVisibility {
        self.death_message_visibility
    }

    /// The color.
    #[must_use]
    pub const fn color(&self) -> Option<TeamColor> {
        self.color
    }

    /// The collision rule.
    #[must_use]
    pub const fn collision_rule(&self) -> TeamCollisionRule {
        self.collision_rule
    }

    fn apply_color(&self, component: TextComponent) -> TextComponent {
        match self.color {
            Some(color) => component.color(color.text_color()),
            None => component,
        }
    }

    /// Vanilla `getFormattedDisplayName`: the display name in square brackets,
    /// inserting and showing the team name, in the team color.
    #[must_use]
    pub fn formatted_display_name(&self) -> TextComponent {
        let name = self
            .display_name
            .clone()
            .insertion(self.name.clone())
            .hover_event(HoverEvent::show_text(TextComponent::plain(
                self.name.clone(),
            )));
        self.apply_color(
            translations::CHAT_SQUARE_BRACKETS
                .message([name])
                .component(),
        )
    }

    /// Vanilla `getFormattedName`: a member's name between the prefix and
    /// suffix, in the team color.
    #[must_use]
    pub fn formatted_name(&self, member_name: TextComponent) -> TextComponent {
        self.apply_color(
            TextComponent::new()
                .add_child(self.player_prefix.clone())
                .add_child(member_name)
                .add_child(self.player_suffix.clone()),
        )
    }

    /// Vanilla `packOptions`.
    #[must_use]
    pub const fn pack_options(&self) -> u8 {
        let mut options = 0;
        if self.allow_friendly_fire {
            options |= 1;
        }
        if self.see_friendly_invisibles {
            options |= 2;
        }
        options
    }

    /// Vanilla `ClientboundSetPlayerTeamPacket.Parameters(team)`.
    #[must_use]
    pub fn parameters(&self) -> TeamParameters {
        TeamParameters {
            display_name: self.display_name.clone(),
            player_prefix: self.player_prefix.clone(),
            player_suffix: self.player_suffix.clone(),
            name_tag_visibility: self.name_tag_visibility,
            collision_rule: self.collision_rule,
            color: self.color.map(TeamColor::id),
            options: self.pack_options(),
        }
    }
}
