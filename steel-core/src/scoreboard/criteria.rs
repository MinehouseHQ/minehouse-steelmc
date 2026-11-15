//! Vanilla `ObjectiveCriteria`, `TeamColor` and `DisplaySlot`.

use steel_protocol::packets::game::ObjectiveRenderType;
use steel_registry::{REGISTRY, RegistryExt as _, stat::Stat};
use steel_utils::Identifier;
use text_components::format::Color;

/// Vanilla `TeamColor`, in id order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TeamColor {
    /// `black`.
    Black,
    /// `dark_blue`.
    DarkBlue,
    /// `dark_green`.
    DarkGreen,
    /// `dark_aqua`.
    DarkAqua,
    /// `dark_red`.
    DarkRed,
    /// `dark_purple`.
    DarkPurple,
    /// `gold`.
    Gold,
    /// `gray`.
    Gray,
    /// `dark_gray`.
    DarkGray,
    /// `blue`.
    Blue,
    /// `green`.
    Green,
    /// `aqua`.
    Aqua,
    /// `red`.
    Red,
    /// `light_purple`.
    LightPurple,
    /// `yellow`.
    Yellow,
    /// `white`.
    White,
}

impl TeamColor {
    /// Vanilla `TeamColor.VALUES`.
    pub const ALL: [Self; 16] = [
        Self::Black,
        Self::DarkBlue,
        Self::DarkGreen,
        Self::DarkAqua,
        Self::DarkRed,
        Self::DarkPurple,
        Self::Gold,
        Self::Gray,
        Self::DarkGray,
        Self::Blue,
        Self::Green,
        Self::Aqua,
        Self::Red,
        Self::LightPurple,
        Self::Yellow,
        Self::White,
    ];

    /// The network id.
    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }

    /// The serialized name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Black => "black",
            Self::DarkBlue => "dark_blue",
            Self::DarkGreen => "dark_green",
            Self::DarkAqua => "dark_aqua",
            Self::DarkRed => "dark_red",
            Self::DarkPurple => "dark_purple",
            Self::Gold => "gold",
            Self::Gray => "gray",
            Self::DarkGray => "dark_gray",
            Self::Blue => "blue",
            Self::Green => "green",
            Self::Aqua => "aqua",
            Self::Red => "red",
            Self::LightPurple => "light_purple",
            Self::Yellow => "yellow",
            Self::White => "white",
        }
    }

    /// Vanilla `TeamColor.byName`.
    #[must_use]
    pub fn by_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|color| color.name() == name)
    }

    /// Vanilla `textColor`.
    #[must_use]
    pub const fn text_color(self) -> Color {
        match self {
            Self::Black => Color::Black,
            Self::DarkBlue => Color::DarkBlue,
            Self::DarkGreen => Color::DarkGreen,
            Self::DarkAqua => Color::DarkAqua,
            Self::DarkRed => Color::DarkRed,
            Self::DarkPurple => Color::DarkPurple,
            Self::Gold => Color::Gold,
            Self::Gray => Color::Gray,
            Self::DarkGray => Color::DarkGray,
            Self::Blue => Color::Blue,
            Self::Green => Color::Green,
            Self::Aqua => Color::Aqua,
            Self::Red => Color::Red,
            Self::LightPurple => Color::LightPurple,
            Self::Yellow => Color::Yellow,
            Self::White => Color::White,
        }
    }

    /// Vanilla `displaySlot`: the team's sidebar.
    #[must_use]
    pub const fn display_slot(self) -> DisplaySlot {
        DisplaySlot::TeamSidebar(self)
    }
}

/// Vanilla `DisplaySlot`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DisplaySlot {
    /// `list`, the player list.
    List,
    /// `sidebar`.
    Sidebar,
    /// `below_name`.
    BelowName,
    /// `sidebar.team.<color>`
    TeamSidebar(TeamColor),
}

impl DisplaySlot {
    /// Vanilla `DisplaySlot.values()`, in id order.
    pub const ALL: [Self; 19] = {
        let mut slots = [Self::List; 19];
        slots[1] = Self::Sidebar;
        slots[2] = Self::BelowName;
        let mut index = 0;
        while index < TeamColor::ALL.len() {
            slots[index + 3] = Self::TeamSidebar(TeamColor::ALL[index]);
            index += 1;
        }
        slots
    };

    /// The network id.
    #[must_use]
    pub const fn id(self) -> i32 {
        match self {
            Self::List => 0,
            Self::Sidebar => 1,
            Self::BelowName => 2,
            Self::TeamSidebar(color) => 3 + color.id(),
        }
    }

    /// Index into [`Self::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        self.id() as usize
    }

    /// The serialized name.
    #[must_use]
    pub fn name(self) -> String {
        match self {
            Self::List => "list".to_owned(),
            Self::Sidebar => "sidebar".to_owned(),
            Self::BelowName => "below_name".to_owned(),
            Self::TeamSidebar(color) => format!("sidebar.team.{}", color.name()),
        }
    }

    /// Looks a slot up by its serialized name.
    #[must_use]
    pub fn by_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|slot| slot.name() == name)
    }
}

/// Vanilla's custom (non-stat) criteria.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CustomCriteria {
    /// `dummy`.
    Dummy,
    /// `trigger`, which `/trigger` may change.
    Trigger,
    /// `deathCount`.
    DeathCount,
    /// `playerKillCount`.
    PlayerKillCount,
    /// `totalKillCount`.
    TotalKillCount,
    /// `health`, shown as hearts.
    Health,
    /// `food`.
    Food,
    /// `air`.
    Air,
    /// `armor`.
    Armor,
    /// `xp`.
    Experience,
    /// `level`.
    Level,
    /// `teamkill.<color>`.
    TeamKill(TeamColor),
    /// `killedByTeam.<color>`.
    KilledByTeam(TeamColor),
}

impl CustomCriteria {
    /// Every custom criterion, in vanilla registration order.
    #[must_use]
    pub fn all() -> Vec<Self> {
        let mut all = vec![
            Self::Dummy,
            Self::Trigger,
            Self::DeathCount,
            Self::PlayerKillCount,
            Self::TotalKillCount,
            Self::Health,
            Self::Food,
            Self::Air,
            Self::Armor,
            Self::Experience,
            Self::Level,
        ];
        all.extend(TeamColor::ALL.map(Self::TeamKill));
        all.extend(TeamColor::ALL.map(Self::KilledByTeam));
        all
    }

    fn name(self) -> String {
        let name = match self {
            Self::Dummy => "dummy",
            Self::Trigger => "trigger",
            Self::DeathCount => "deathCount",
            Self::PlayerKillCount => "playerKillCount",
            Self::TotalKillCount => "totalKillCount",
            Self::Health => "health",
            Self::Food => "food",
            Self::Air => "air",
            Self::Armor => "armor",
            Self::Experience => "xp",
            Self::Level => "level",
            Self::TeamKill(color) => return format!("teamkill.{}", color.name()),
            Self::KilledByTeam(color) => return format!("killedByTeam.{}", color.name()),
        };
        name.to_owned()
    }

    const fn is_read_only(self) -> bool {
        matches!(
            self,
            Self::Health | Self::Food | Self::Air | Self::Armor | Self::Experience | Self::Level
        )
    }

    const fn render_type(self) -> ObjectiveRenderType {
        if matches!(self, Self::Health) {
            ObjectiveRenderType::Hearts
        } else {
            ObjectiveRenderType::Integer
        }
    }
}

/// Vanilla `ObjectiveCriteria`: a custom criterion or a statistic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ObjectiveCriteria {
    /// A criterion that is not a statistic.
    Custom(CustomCriteria),
    /// Vanilla `Stat`, which extends `ObjectiveCriteria`.
    Stat(Stat),
}

impl ObjectiveCriteria {
    /// Vanilla `ObjectiveCriteria.DUMMY`.
    pub const DUMMY: Self = Self::Custom(CustomCriteria::Dummy);

    /// Vanilla `getName`; a stat's name is `<type>:<value>` with `.` for `:`.
    #[must_use]
    pub fn name(&self) -> String {
        match self {
            Self::Custom(custom) => custom.name(),
            Self::Stat(stat) => stat.to_string(),
        }
    }

    /// Vanilla `isReadOnly`.
    #[must_use]
    pub const fn is_read_only(&self) -> bool {
        match self {
            Self::Custom(custom) => custom.is_read_only(),
            Self::Stat(_) => false,
        }
    }

    /// Vanilla `getDefaultRenderType`.
    #[must_use]
    pub const fn default_render_type(&self) -> ObjectiveRenderType {
        match self {
            Self::Custom(custom) => custom.render_type(),
            Self::Stat(_) => ObjectiveRenderType::Integer,
        }
    }

    /// Vanilla `byName`: a custom criterion, or `<type>:<value>` with each
    /// half an id whose namespace separator is `.`.
    #[must_use]
    pub fn by_name(name: &str) -> Option<Self> {
        if let Some(custom) = CustomCriteria::all()
            .into_iter()
            .find(|custom| custom.name() == name)
        {
            return Some(Self::Custom(custom));
        }
        let (stat_type, value) = name.split_once(':')?;
        let stat_type = REGISTRY
            .stat_types
            .by_key(&identifier_by_separator(stat_type)?)?;
        let value = stat_type.value_from_key(&identifier_by_separator(value)?)?;
        Some(Self::Stat(Stat::from_erased(stat_type, value)))
    }
}

/// Vanilla `Identifier.bySeparator(name, '.')`.
fn identifier_by_separator(name: &str) -> Option<Identifier> {
    let id = match name.split_once('.') {
        Some((namespace, path)) => format!("{namespace}:{path}"),
        None => name.to_owned(),
    };
    id.parse().ok()
}
