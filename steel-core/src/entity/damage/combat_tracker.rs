//! Vanilla `CombatTracker`: the damage log behind death messages and the
//! player combat enter and end packets.

use std::mem;
use std::sync::LazyLock;

use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::damage_type::{DamageType, DeathMessageType};
use steel_registry::data_components::vanilla_components::CUSTOM_NAME;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_damage_type_tags::DamageTypeTag;
use steel_registry::{REGISTRY, TaggedRegistryExt as _};
use steel_utils::{Identifier, translations};
use text_components::TextComponent;
use text_components::interactivity::{ClickEvent, HoverEvent, Interactivity};
use text_components::translation::TranslatedMessage;

use super::DamageSource;
use crate::behavior::ITEM_BEHAVIORS;
use crate::entity::{Entity, LivingEntity, get_kill_credit};
use crate::inventory::equipment::EquipmentSlot;

/// Vanilla `CombatTracker.RESET_DAMAGE_STATUS_TIME`.
pub const RESET_DAMAGE_STATUS_TIME: i32 = 100;
/// Vanilla `CombatTracker.RESET_COMBAT_STATUS_TIME`.
pub const RESET_COMBAT_STATUS_TIME: i32 = 300;

/// Vanilla `CombatTracker.INTENTIONAL_GAME_DESIGN_STYLE`.
static INTENTIONAL_GAME_DESIGN_STYLE: LazyLock<Interactivity> = LazyLock::new(|| Interactivity {
    click: Some(ClickEvent::open_url(
        "https://bugs.mojang.com/browse/MCPE-28723",
    )),
    hover: Some(HoverEvent::show_text("MCPE-28723")),
    ..Interactivity::new()
});

/// Vanilla `FallLocation`: where the victim was when it started falling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallLocation {
    /// `generic`
    Generic,
    /// `ladder` (ladders and trapdoors)
    Ladder,
    /// `vines`
    Vines,
    /// `weeping_vines`
    WeepingVines,
    /// `twisting_vines`
    TwistingVines,
    /// `scaffolding`
    Scaffolding,
    /// `other_climbable`
    OtherClimbable,
    /// `water`
    Water,
}

impl FallLocation {
    /// Vanilla `FallLocation.id`.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::Ladder => "ladder",
            Self::Vines => "vines",
            Self::WeepingVines => "weeping_vines",
            Self::TwistingVines => "twisting_vines",
            Self::Scaffolding => "scaffolding",
            Self::OtherClimbable => "other_climbable",
            Self::Water => "water",
        }
    }

    /// Vanilla `FallLocation.languageKey`.
    #[must_use]
    pub fn language_key(self) -> String {
        format!("death.fell.accident.{}", self.id())
    }

    /// Vanilla `FallLocation.blockToFallLocation`.
    #[must_use]
    pub fn from_block(block: BlockRef) -> Self {
        if block == &vanilla_blocks::LADDER || block.has_tag(&BlockTag::TRAPDOORS) {
            Self::Ladder
        } else if block == &vanilla_blocks::VINE {
            Self::Vines
        } else if block == &vanilla_blocks::WEEPING_VINES
            || block == &vanilla_blocks::WEEPING_VINES_PLANT
        {
            Self::WeepingVines
        } else if block == &vanilla_blocks::TWISTING_VINES
            || block == &vanilla_blocks::TWISTING_VINES_PLANT
        {
            Self::TwistingVines
        } else if block == &vanilla_blocks::SCAFFOLDING {
            Self::Scaffolding
        } else {
            Self::OtherClimbable
        }
    }

    /// Vanilla `FallLocation.getCurrentFallLocation`.
    #[must_use]
    pub fn current<E: LivingEntity + ?Sized>(mob: &E) -> Option<Self> {
        if let Some(pos) = mob.last_climbable_pos() {
            let world = mob.level()?;
            return Some(Self::from_block(world.get_block_state(pos).get_block()));
        }
        mob.is_in_water().then_some(Self::Water)
    }
}

/// The parts of an attacker a death message names, captured when the damage
/// is recorded.
///
/// **Deviation from vanilla:** vanilla keeps the attacking entities alive in
/// the log and reads their names when the message is built. Steel stores the
/// names instead so the log never keeps an entity (or a reference cycle
/// between two fighters) alive.
#[derive(Debug, Clone, PartialEq)]
struct AttackerName {
    /// Vanilla `Entity.getDisplayName`.
    name: TextComponent,
    /// Display name of the attacker's main hand item when it has a custom name.
    named_item: Option<TextComponent>,
}

impl AttackerName {
    fn of(entity: &dyn Entity) -> Self {
        let named_item = entity.as_living_entity().and_then(|living| {
            let held = living.equipment_in_slot(EquipmentSlot::MainHand);
            (!held.is_empty() && held.has(CUSTOM_NAME)).then(|| ITEM_BEHAVIORS.display_name(&held))
        });
        Self {
            name: entity.display_name(),
            named_item,
        }
    }
}

/// Vanilla `CombatEntry`: one recorded hit.
#[derive(Debug, Clone)]
struct CombatEntry {
    damage_type: &'static DamageType,
    /// `source.getEntity()`: the causing entity.
    causing: Option<AttackerName>,
    /// `source.getDirectEntity()`.
    direct: Option<AttackerName>,
    damage: f32,
    fall_location: Option<FallLocation>,
    fall_distance: f32,
}

impl CombatEntry {
    fn is(&self, tag: &Identifier) -> bool {
        REGISTRY.damage_types.is_in_tag(self.damage_type, tag)
    }
}

/// Vanilla `CombatTracker` state, stored on every living entity.
#[derive(Debug, Clone, Default)]
pub struct CombatTracker {
    entries: Vec<CombatEntry>,
    last_damage_time: i32,
    combat_start_time: i32,
    combat_end_time: i32,
    in_combat: bool,
    taking_damage: bool,
}

impl CombatTracker {
    /// Whether the entity is in combat (hit by a living entity recently).
    #[must_use]
    pub const fn in_combat(&self) -> bool {
        self.in_combat
    }

    /// Vanilla `getCombatDuration`.
    #[must_use]
    pub const fn combat_duration(&self, tick_count: i32) -> i32 {
        if self.in_combat {
            tick_count - self.combat_start_time
        } else {
            self.combat_end_time - self.combat_start_time
        }
    }

    /// Vanilla `getMostSignificantFall`.
    fn most_significant_fall(&self) -> Option<&CombatEntry> {
        let mut result = None;
        let mut alternative: Option<&CombatEntry> = None;
        let mut alt_damage = 0.0_f32;
        let mut best_fall = 0.0_f32;

        for (i, entry) in self.entries.iter().enumerate() {
            let is_fake_fall = entry.is(&DamageTypeTag::ALWAYS_MOST_SIGNIFICANT_FALL);
            let fall_distance = if is_fake_fall {
                f32::MAX
            } else {
                entry.fall_distance
            };
            if (entry.is(&DamageTypeTag::IS_FALL) || is_fake_fall)
                && fall_distance > 0.0
                && (result.is_none() || fall_distance > best_fall)
            {
                result = Some(if i > 0 { &self.entries[i - 1] } else { entry });
                best_fall = fall_distance;
            }

            if entry.fall_location.is_some() && (alternative.is_none() || entry.damage > alt_damage)
            {
                alternative = Some(entry);
                alt_damage = entry.damage;
            }
        }

        if best_fall > 5.0 && result.is_some() {
            result
        } else if alt_damage > 5.0 {
            alternative
        } else {
            None
        }
    }
}

fn translated(key: impl Into<String>, args: Vec<TextComponent>) -> TextComponent {
    TranslatedMessage {
        key: key.into().into(),
        fallback: None,
        args: Some(args.into_boxed_slice()),
    }
    .component()
}

/// Vanilla `CombatTracker.recordDamage`.
pub fn record_damage<E: LivingEntity + ?Sized>(mob: &E, source: &DamageSource, damage: f32) {
    recheck_status(mob);
    let entry = CombatEntry {
        damage_type: source.damage_type,
        causing: source
            .causing_entity()
            .map(|entity| AttackerName::of(entity.as_ref())),
        direct: source
            .direct_entity()
            .map(|entity| AttackerName::of(entity.as_ref())),
        damage,
        fall_location: FallLocation::current(mob),
        fall_distance: mob.fall_distance() as f32,
    };
    let enters_combat = source
        .causing_entity()
        .is_some_and(|entity| entity.is_living_entity());
    let tick_count = mob.tick_count();
    let entered = {
        let mut tracker = mob.living_base().combat_tracker().lock();
        tracker.entries.push(entry);
        tracker.last_damage_time = tick_count;
        tracker.taking_damage = true;
        let entered = !tracker.in_combat && Entity::is_alive(mob) && enters_combat;
        if entered {
            tracker.in_combat = true;
            tracker.combat_start_time = tick_count;
            tracker.combat_end_time = tick_count;
        }
        entered
    };
    if entered {
        mob.on_enter_combat();
    }
}

/// Vanilla `CombatTracker.recheckStatus`: forgets the log once the entity
/// is dead or has not been hurt for a while.
pub fn recheck_status<E: LivingEntity + ?Sized>(mob: &E) {
    let alive = Entity::is_alive(mob);
    let tick_count = mob.tick_count();
    let (left_combat, cleared) = {
        let mut tracker = mob.living_base().combat_tracker().lock();
        let reset = if tracker.in_combat {
            RESET_COMBAT_STATUS_TIME
        } else {
            RESET_DAMAGE_STATUS_TIME
        };
        if !tracker.taking_damage || (alive && tick_count - tracker.last_damage_time <= reset) {
            return;
        }
        let was_in_combat = tracker.in_combat;
        tracker.taking_damage = false;
        tracker.in_combat = false;
        tracker.combat_end_time = tick_count;
        (was_in_combat, mem::take(&mut tracker.entries))
    };
    if left_combat {
        mob.on_leave_combat();
    }
    drop(cleared);
}

/// Vanilla `CombatTracker.getDeathMessage`.
#[must_use]
pub fn death_message<E: LivingEntity + ?Sized>(mob: &E) -> TextComponent {
    let tracker = mob.living_base().combat_tracker().lock().clone();
    let victim = mob.display_name();
    let Some(killing_blow) = tracker.entries.last() else {
        return translated("death.attack.generic", vec![victim]);
    };
    let message_id = &killing_blow.damage_type.message_id;
    let message_type = killing_blow.damage_type.death_message_type;
    if message_type == DeathMessageType::FallVariants
        && let Some(knock_off) = tracker.most_significant_fall()
    {
        return fall_message(victim, knock_off, killing_blow.causing.as_ref());
    }
    match message_type {
        DeathMessageType::IntentionalGameDesign => {
            let death_msg = format!("death.attack.{message_id}");
            let mut link = translations::CHAT_SQUARE_BRACKETS
                .message([translated(format!("{death_msg}.link"), Vec::new())])
                .component();
            link.interactions = INTENTIONAL_GAME_DESIGN_STYLE.clone();
            translated(format!("{death_msg}.message"), vec![victim, link])
        }
        _ => localized_death_message(mob, victim, killing_blow),
    }
}

/// Vanilla `CombatTracker.getFallMessage`.
fn fall_message(
    victim: TextComponent,
    knock_off: &CombatEntry,
    killer: Option<&AttackerName>,
) -> TextComponent {
    if knock_off.is(&DamageTypeTag::IS_FALL)
        || knock_off.is(&DamageTypeTag::ALWAYS_MOST_SIGNIFICANT_FALL)
    {
        let location = knock_off.fall_location.unwrap_or(FallLocation::Generic);
        return translated(location.language_key(), vec![victim]);
    }
    match (knock_off.causing.as_ref(), killer) {
        (Some(attacker), killer) if killer.is_none_or(|killer| killer.name != attacker.name) => {
            assisted_fall_message(victim, attacker, "death.fell.assist")
        }
        (_, Some(killer)) => assisted_fall_message(victim, killer, "death.fell.finish"),
        (_, None) => translated("death.fell.killer", vec![victim]),
    }
}

/// Vanilla `CombatTracker.getMessageForAssistedFall`.
fn assisted_fall_message(
    victim: TextComponent,
    attacker: &AttackerName,
    key: &str,
) -> TextComponent {
    match &attacker.named_item {
        Some(item) => translated(
            format!("{key}.item"),
            vec![victim, attacker.name.clone(), item.clone()],
        ),
        None => translated(key, vec![victim, attacker.name.clone()]),
    }
}

/// Vanilla `DamageSource.getLocalizedDeathMessage`.
fn localized_death_message<E: LivingEntity + ?Sized>(
    mob: &E,
    victim: TextComponent,
    entry: &CombatEntry,
) -> TextComponent {
    let death_msg = format!("death.attack.{}", entry.damage_type.message_id);
    let Some(attacker) = entry.causing.as_ref().or(entry.direct.as_ref()) else {
        let credit = mob.level().and_then(|world| get_kill_credit(mob, &world));
        return match credit {
            Some(credit) => translated(
                format!("{death_msg}.player"),
                vec![victim, credit.display_name()],
            ),
            None => translated(death_msg, vec![victim]),
        };
    };
    // Only the causing entity's held item is named, as in vanilla.
    match entry
        .causing
        .as_ref()
        .and_then(|causing| causing.named_item.clone())
    {
        Some(item) => translated(
            format!("{death_msg}.item"),
            vec![victim, attacker.name.clone(), item],
        ),
        None => translated(death_msg, vec![victim, attacker.name.clone()]),
    }
}

#[cfg(test)]
mod tests;
