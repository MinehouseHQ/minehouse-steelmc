//! The warden's grudges (vanilla `AngerLevel` and `AngerManagement`).

use std::cmp::Ordering;
use std::mem;

use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::{sound_events, vanilla_entities};
use steel_utils::UuidExt as _;
use uuid::Uuid;

use crate::entity::callback::RemovalReason;
use crate::entity::{Entity, SharedEntity};
use crate::world::World;

/// Vanilla `AngerManagement.CONVERSION_DELAY`: ticks between two lookups of
/// suspects saved by UUID.
const CONVERSION_DELAY: i32 = 2;
/// Vanilla `AngerManagement.MAX_ANGER`.
pub(super) const MAX_ANGER: i32 = 150;
/// Vanilla `AngerManagement.DEFAULT_ANGER_DECREASE`.
const DEFAULT_ANGER_DECREASE: i32 = 1;

/// How angry the warden is (vanilla `AngerLevel`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AngerLevel {
    /// Below 40 anger.
    Calm,
    /// From 40 anger.
    Agitated,
    /// From 80 anger: the warden hunts.
    Angry,
}

impl AngerLevel {
    /// Vanilla `getMinimumAnger`.
    #[must_use]
    pub const fn minimum_anger(self) -> i32 {
        match self {
            Self::Calm => 0,
            Self::Agitated => 40,
            Self::Angry => 80,
        }
    }

    /// Vanilla `getAmbientSound`.
    #[must_use]
    pub fn ambient_sound(self) -> SoundEventRef {
        match self {
            Self::Calm => &sound_events::ENTITY_WARDEN_AMBIENT,
            Self::Agitated => &sound_events::ENTITY_WARDEN_AGITATED,
            Self::Angry => &sound_events::ENTITY_WARDEN_ANGRY,
        }
    }

    /// Vanilla `getListeningSound`.
    #[must_use]
    pub fn listening_sound(self) -> SoundEventRef {
        match self {
            Self::Calm => &sound_events::ENTITY_WARDEN_LISTENING,
            Self::Agitated | Self::Angry => &sound_events::ENTITY_WARDEN_LISTENING_ANGRY,
        }
    }

    /// Vanilla `byAnger`: the highest level whose minimum `anger` reaches.
    #[must_use]
    pub const fn by_anger(anger: i32) -> Self {
        if anger >= Self::Angry.minimum_anger() {
            Self::Angry
        } else if anger >= Self::Agitated.minimum_anger() {
            Self::Agitated
        } else {
            Self::Calm
        }
    }

    /// Vanilla `isAngry`.
    #[must_use]
    pub fn is_angry(self) -> bool {
        self == Self::Angry
    }
}

/// One suspect with the warden's anger at it.
struct Suspect {
    entity: SharedEntity,
    anger: i32,
}

/// The warden's anger at each suspect (vanilla `AngerManagement`).
///
/// Vanilla keeps `suspects` and `angerBySuspect` side by side; Steel keeps
/// each suspect with its anger, in the sorted `suspects` order.
pub struct AngerManagement {
    conversion_delay: i32,
    highest_anger: i32,
    suspects: Vec<Suspect>,
    /// Saved anger at suspects not found in the level yet, in insertion order.
    anger_by_uuid: Vec<(Uuid, i32)>,
}

impl Default for AngerManagement {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl AngerManagement {
    /// Vanilla `AngerManagement(filter, angerByUuid)`.
    #[must_use]
    pub fn new(anger_by_uuid: Vec<(Uuid, i32)>) -> Self {
        Self {
            conversion_delay: rand::random_range(0..=CONVERSION_DELAY),
            highest_anger: 0,
            suspects: Vec::new(),
            anger_by_uuid,
        }
    }

    /// Vanilla `tick`: finds saved suspects, cools down, and drops suspects
    /// that `valid_entity` rejects or that left.
    pub fn tick(&mut self, world: &World, valid_entity: impl Fn(&SharedEntity) -> bool) {
        self.conversion_delay -= 1;
        if self.conversion_delay <= 0 {
            self.convert_from_uuids(world);
            self.conversion_delay = CONVERSION_DELAY;
        }

        self.anger_by_uuid.retain_mut(|(_, anger)| {
            if *anger <= DEFAULT_ANGER_DECREASE {
                return false;
            }
            *anger -= DEFAULT_ANGER_DECREASE;
            true
        });

        let mut kept = Vec::with_capacity(self.suspects.len());
        for mut suspect in mem::take(&mut self.suspects) {
            let removal_reason = suspect.entity.removal_reason();
            if suspect.anger > DEFAULT_ANGER_DECREASE
                && valid_entity(&suspect.entity)
                && removal_reason.is_none()
            {
                suspect.anger -= DEFAULT_ANGER_DECREASE;
                kept.push(suspect);
            } else if suspect.anger > DEFAULT_ANGER_DECREASE
                && removal_reason.is_some_and(keeps_anger_by_uuid)
            {
                self.put_anger_by_uuid(
                    suspect.entity.uuid(),
                    suspect.anger - DEFAULT_ANGER_DECREASE,
                );
            }
        }
        self.suspects = kept;
        self.sort_and_update_highest_anger();
    }

    fn put_anger_by_uuid(&mut self, uuid: Uuid, anger: i32) {
        match self.anger_by_uuid.iter_mut().find(|(id, _)| *id == uuid) {
            Some((_, saved)) => *saved = anger,
            None => self.anger_by_uuid.push((uuid, anger)),
        }
    }

    /// Vanilla `sortAndUpdateHighestAnger`. Vanilla's sorter raises
    /// `highestAnger` to every compared anger, and a sort of two or more
    /// suspects compares each of them, so it ends as the highest anger.
    fn sort_and_update_highest_anger(&mut self) {
        self.suspects.sort_by(|a, b| {
            if a.entity.id() == b.entity.id() {
                return Ordering::Equal;
            }
            let angry_at_a = AngerLevel::by_anger(a.anger).is_angry();
            let angry_at_b = AngerLevel::by_anger(b.anger).is_angry();
            if angry_at_a != angry_at_b {
                return angry_at_b.cmp(&angry_at_a);
            }
            let a_is_player = a.entity.entity_type() == &vanilla_entities::PLAYER;
            let b_is_player = b.entity.entity_type() == &vanilla_entities::PLAYER;
            if a_is_player != b_is_player {
                return b_is_player.cmp(&a_is_player);
            }
            b.anger.cmp(&a.anger)
        });
        self.highest_anger = self
            .suspects
            .iter()
            .map(|suspect| suspect.anger)
            .max()
            .unwrap_or(0);
    }

    /// Vanilla `convertFromUuids`.
    fn convert_from_uuids(&mut self, world: &World) {
        let mut remaining = Vec::with_capacity(self.anger_by_uuid.len());
        for (uuid, anger) in mem::take(&mut self.anger_by_uuid) {
            match world.get_entity_by_uuid(&uuid) {
                Some(entity) => {
                    match self
                        .suspects
                        .iter_mut()
                        .find(|suspect| suspect.entity.id() == entity.id())
                    {
                        Some(suspect) => suspect.anger = anger,
                        None => self.suspects.push(Suspect { entity, anger }),
                    }
                }
                None => remaining.push((uuid, anger)),
            }
        }
        self.anger_by_uuid = remaining;
    }

    /// Vanilla `increaseAnger`: returns the new anger at `entity`.
    pub fn increase_anger(&mut self, entity: &SharedEntity, increment: i32) -> i32 {
        let current = if let Some(suspect) = self
            .suspects
            .iter_mut()
            .find(|suspect| suspect.entity.id() == entity.id())
        {
            suspect.anger = (suspect.anger + increment).min(MAX_ANGER);
            suspect.anger
        } else {
            let uuid = entity.uuid();
            let serialized = self
                .anger_by_uuid
                .iter()
                .position(|(id, _)| *id == uuid)
                .map_or(0, |index| self.anger_by_uuid.remove(index).1);
            let anger = increment.min(MAX_ANGER) + serialized;
            self.suspects.push(Suspect {
                entity: SharedEntity::clone(entity),
                anger,
            });
            anger
        };
        self.sort_and_update_highest_anger();
        current
    }

    /// Vanilla `clearAnger`.
    pub fn clear_anger(&mut self, entity: &dyn Entity) {
        self.suspects
            .retain(|suspect| suspect.entity.id() != entity.id());
        self.sort_and_update_highest_anger();
    }

    /// Vanilla `getActiveAnger`: the anger at `current_target`, or the
    /// highest anger without a target.
    #[must_use]
    pub fn active_anger(&self, current_target: Option<&dyn Entity>) -> i32 {
        match current_target {
            None => self.highest_anger,
            Some(target) => self
                .suspects
                .iter()
                .find(|suspect| suspect.entity.id() == target.id())
                .map_or(0, |suspect| suspect.anger),
        }
    }

    /// Vanilla `getActiveEntity`: the first suspect `filter` accepts, if it
    /// is living.
    #[must_use]
    pub fn active_entity(&self, filter: impl Fn(&SharedEntity) -> bool) -> Option<SharedEntity> {
        self.suspects
            .iter()
            .map(|suspect| &suspect.entity)
            .find(|entity| filter(entity))
            .filter(|entity| entity.as_living_entity().is_some())
            .map(SharedEntity::clone)
    }

    /// The anger at `entity`, zero for no suspect (vanilla
    /// `angerBySuspect.getInt`).
    #[must_use]
    pub fn anger_at(&self, entity: &dyn Entity) -> i32 {
        self.active_anger(Some(entity))
    }

    /// Vanilla `codec` encode: `{suspects: [{uuid, anger}]}`, live suspects
    /// first, then the ones saved by UUID.
    #[must_use]
    pub fn save(&self) -> NbtCompound {
        let pairs = self
            .suspects
            .iter()
            .map(|suspect| (suspect.entity.uuid(), suspect.anger))
            .chain(self.anger_by_uuid.iter().copied());
        let suspects = pairs
            .map(|(uuid, anger)| {
                let mut compound = NbtCompound::new();
                compound.insert("uuid", NbtTag::IntArray(uuid.to_int_array().to_vec()));
                compound.insert("anger", anger);
                compound
            })
            .collect();
        let mut nbt = NbtCompound::new();
        nbt.insert("suspects", NbtTag::List(NbtList::Compound(suspects)));
        nbt
    }

    /// Vanilla `codec` decode. `suspects` is a lenient optional field: a
    /// malformed list, or a negative anger, loads as no suspects.
    #[must_use]
    pub fn load(nbt: BorrowedNbtCompoundView<'_, '_>) -> Self {
        let parsed = nbt
            .list("suspects")
            .and_then(|list| list.compounds())
            .and_then(|entries| {
                entries
                    .into_iter()
                    .map(|entry| {
                        let uuid = Uuid::from_int_array(&entry.int_array("uuid")?)?;
                        let anger = entry.int("anger").filter(|anger| *anger >= 0)?;
                        Some((uuid, anger))
                    })
                    .collect::<Option<Vec<_>>>()
            });
        let mut anger_by_uuid: Vec<(Uuid, i32)> = Vec::new();
        for (uuid, anger) in parsed.unwrap_or_default() {
            match anger_by_uuid.iter_mut().find(|(id, _)| *id == uuid) {
                Some((_, saved)) => *saved = anger,
                None => anger_by_uuid.push((uuid, anger)),
            }
        }
        Self::new(anger_by_uuid)
    }

    /// Whether the warden suspects `entity`.
    #[cfg(test)]
    pub(super) fn is_suspect(&self, entity: &dyn Entity) -> bool {
        self.suspects
            .iter()
            .any(|suspect| suspect.entity.id() == entity.id())
    }

    /// Saved anger at suspects not in the level yet.
    #[cfg(test)]
    pub(super) fn anger_by_uuid(&self) -> &[(Uuid, i32)] {
        &self.anger_by_uuid
    }
}

/// Vanilla keeps the anger at a suspect that changed dimension or unloaded
/// with its chunk or player, to find it again later.
const fn keeps_anger_by_uuid(reason: RemovalReason) -> bool {
    matches!(
        reason,
        RemovalReason::ChangedWorld
            | RemovalReason::UnloadedToChunk
            | RemovalReason::StoredWithPlayer
    )
}
