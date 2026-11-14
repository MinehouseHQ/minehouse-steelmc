//! Vanilla `SculkSpreader` with the `SculkBehaviour` of sculk, sculk veins and
//! every other block: charge cursors that crawl over sculk, turn the blocks
//! they touch into sculk and grow sensors and shriekers.
//!
//! The same spreader runs in a live world, where a sculk catalyst feeds it the
//! experience of mobs dying nearby, and in world generation, where sculk
//! patches use it with their own settings.

use std::mem;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use simdnbt::borrow::NbtCompound as NbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction};
use steel_registry::fluid::FluidStateExt as _;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::{level_events, sound_events, vanilla_blocks};
use steel_utils::random::Random;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, ChunkPos, Identifier};

use crate::behavior::block::push_entities_up;
use crate::behavior::blocks::{MultifaceBlock, multiface_face_property};
use crate::fluid::state::get_fluid_state_from_block;
use crate::world::{LevelAccessor, World};

/// Vanilla `SculkSpreader.MAX_GROWTH_RATE_RADIUS`.
const MAX_GROWTH_RATE_RADIUS: i32 = 24;
/// Vanilla `SculkSpreader.MAX_CHARGE`.
pub const MAX_CHARGE: i32 = 1000;
/// Vanilla `SculkSpreader.MAX_CURSORS`.
pub const MAX_CURSORS: usize = 32;
/// Vanilla `SculkSpreader.SHRIEKER_PLACEMENT_RATE`.
const SHRIEKER_PLACEMENT_RATE: i32 = 11;
/// Vanilla `SculkSpreader.MAX_CURSOR_DISTANCE`.
const MAX_CURSOR_DISTANCE: i32 = 1024;
/// Vanilla `ChargeCursor.MAX_CURSOR_DECAY_DELAY`.
const MAX_CURSOR_DECAY_DELAY: i32 = 1;
/// Vanilla `ChargeCursor.MAX_WORLDGEN_SPREAD`, squared.
const MAX_WORLDGEN_SPREAD_SQUARED: i64 = 12 * 12;
/// Vanilla `SculkBehaviour.getSculkSpreadDelay`, the same for every block.
const SCULK_SPREAD_DELAY: i32 = 1;
/// Vanilla `SculkBlock.GROWTH_INHIBITOR_RANGE`.
const GROWTH_INHIBITOR_RANGE: i32 = 4;

const CURSORS_TAG: &str = "cursors";

/// The level a sculk spreader runs in.
///
/// Vanilla passes a `LevelAccessor` and asks it, where it matters, whether it
/// is a server level; these hooks are the places where a live world and a
/// world generation region part ways.
pub trait SculkLevel: LevelAccessor + Sized {
    /// Vanilla `ServerLevel.shouldTickBlocksAt`, asked of the spreader's
    /// origin before a live cursor moves.
    fn should_tick_blocks_at(&self, pos: BlockPos) -> bool;

    /// Vanilla `levelEvent(3006, pos, data)`: the charge particles of a
    /// cursor.
    fn sculk_charge_event(&self, _pos: BlockPos, _data: i32) {}

    /// Vanilla `getChunk(pos).markPosForPostprocessing(pos)`, asked for veins
    /// a world generation spreader places.
    fn mark_pos_for_postprocessing(&self, _pos: BlockPos) {}

    /// Vanilla `Block.pushEntitiesUp` when a block turns into sculk.
    fn push_entities_up(&self, _old_state: BlockStateId, _new_state: BlockStateId, _pos: BlockPos) {
    }

    /// Called after a sensor or shrieker grows, for levels whose
    /// `setBlock` does not create the block entity itself.
    fn on_growth_placed(&self, _pos: BlockPos, _state: BlockStateId) {}
}

impl SculkLevel for Arc<World> {
    fn should_tick_blocks_at(&self, pos: BlockPos) -> bool {
        self.chunk_map
            .is_block_ticking_full_chunk_simulated(ChunkPos::from_block_pos(pos))
    }

    fn sculk_charge_event(&self, pos: BlockPos, data: i32) {
        self.level_event(level_events::PARTICLES_SCULK_CHARGE, pos, data, None);
    }

    fn push_entities_up(&self, old_state: BlockStateId, new_state: BlockStateId, pos: BlockPos) {
        push_entities_up(old_state, new_state, self, pos);
    }
}

/// A set of faces, vanilla's `EnumSet<Direction>`: iterates in direction
/// order and packs into vanilla `MultifaceBlock.pack`'s bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Faces(u8);

impl Faces {
    const fn bit(direction: Direction) -> u8 {
        1 << direction.get_3d_data_value()
    }

    /// Vanilla `MultifaceBlock.availableFaces`: the faces a state has, none
    /// for a block without faces.
    #[must_use]
    pub fn of_state(state: BlockStateId) -> Self {
        let mut faces = Self::default();
        for direction in Direction::ALL {
            if has_face(state, direction) {
                faces.insert(direction);
            }
        }
        faces
    }

    /// Adds a face.
    pub const fn insert(&mut self, direction: Direction) {
        self.0 |= Self::bit(direction);
    }

    /// Whether the set holds `direction`.
    #[must_use]
    pub const fn contains(self, direction: Direction) -> bool {
        self.0 & Self::bit(direction) != 0
    }

    /// Whether the set is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Vanilla `MultifaceBlock.pack`.
    #[must_use]
    pub const fn pack(self) -> i32 {
        self.0 as i32
    }

    /// The faces in direction order.
    pub fn iter(self) -> impl Iterator<Item = Direction> {
        Direction::ALL
            .into_iter()
            .filter(move |&direction| self.contains(direction))
    }
}

/// Vanilla `SculkSpreader.ChargeCursor`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChargeCursor {
    pos: BlockPos,
    charge: i32,
    update_delay: i32,
    decay_delay: i32,
    facings: Option<Faces>,
}

impl ChargeCursor {
    /// A fresh cursor at `pos` holding `charge`.
    #[must_use]
    pub const fn new(pos: BlockPos, charge: i32) -> Self {
        Self {
            pos,
            charge,
            update_delay: 0,
            decay_delay: 1,
            facings: None,
        }
    }

    /// The block the cursor sits on.
    #[must_use]
    pub const fn pos(&self) -> BlockPos {
        self.pos
    }

    /// The charge left.
    #[must_use]
    pub const fn charge(&self) -> i32 {
        self.charge
    }

    /// The faces of the vein the cursor last sat on.
    #[must_use]
    pub const fn facings(&self) -> Option<Faces> {
        self.facings
    }

    fn is_pos_unreasonable(&self, origin: BlockPos) -> bool {
        (self.pos.x() - origin.x())
            .abs()
            .max((self.pos.y() - origin.y()).abs())
            .max((self.pos.z() - origin.z()).abs())
            > MAX_CURSOR_DISTANCE
    }

    fn should_update<L: SculkLevel>(&self, level: &L, origin: BlockPos, world_gen: bool) -> bool {
        self.charge > 0 && (world_gen || level.should_tick_blocks_at(origin))
    }

    fn update<L: SculkLevel, R: Random>(
        &mut self,
        level: &L,
        origin: BlockPos,
        random: &mut R,
        spreader: &SculkSpreader,
        spread_veins: bool,
    ) {
        if !self.should_update(level, origin, spreader.is_world_generation) {
            return;
        }
        if self.update_delay > 0 {
            self.update_delay -= 1;
            return;
        }

        let mut current_state = level.get_block_state(self.pos);
        let mut behavior = SculkBehavior::of(current_state);
        if spread_veins
            && behavior.attempt_spread_vein(
                level,
                self.pos,
                current_state,
                self.facings,
                spreader.is_world_generation,
            )
        {
            if behavior.can_change_block_state_on_spread() {
                current_state = level.get_block_state(self.pos);
                behavior = SculkBehavior::of(current_state);
            }
            level.play_block_sound(&sound_events::BLOCK_SCULK_SPREAD, self.pos, 1.0, 1.0, None);
        }

        self.charge =
            behavior.attempt_use_charge(self, level, origin, random, spreader, spread_veins);
        if self.charge <= 0 {
            behavior.on_discharged(level, current_state, self.pos);
            return;
        }

        if let Some(transfer_pos) = valid_movement_pos(level, self.pos, random, origin, spreader) {
            behavior.on_discharged(level, current_state, self.pos);
            self.pos = transfer_pos;
            current_state = level.get_block_state(transfer_pos);
        } else if spreader.is_world_generation {
            behavior.on_discharged(level, current_state, self.pos);
            self.charge = 0;
            return;
        }

        if SculkBehavior::of(current_state) != SculkBehavior::Default {
            self.facings = Some(Faces::of_state(current_state));
        }
        self.decay_delay = behavior.update_decay_delay(self.decay_delay);
        self.update_delay = SCULK_SPREAD_DELAY;
    }

    const fn merge_with(&mut self, other: &mut Self) {
        self.charge += other.charge;
        other.charge = 0;
        if other.update_delay < self.update_delay {
            self.update_delay = other.update_delay;
        }
    }

    fn save(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        nbt.insert(
            "pos",
            NbtTag::IntArray(vec![self.pos.x(), self.pos.y(), self.pos.z()]),
        );
        if self.charge != 0 {
            nbt.insert("charge", self.charge);
        }
        if self.decay_delay != 1 {
            nbt.insert("decay_delay", self.decay_delay);
        }
        if self.update_delay != 0 {
            nbt.insert("update_delay", self.update_delay);
        }
        if let Some(facings) = self.facings {
            let names = facings
                .iter()
                .map(|direction| direction.as_str().into())
                .collect::<Vec<_>>();
            nbt.insert(
                "facings",
                if names.is_empty() {
                    NbtList::Empty
                } else {
                    NbtList::String(names)
                },
            );
        }
        nbt
    }

    /// Vanilla `ChargeCursor.CODEC`; `None` fails the whole list, as vanilla
    /// does.
    fn load(nbt: &NbtCompoundView<'_, '_>) -> Option<Self> {
        let [x, y, z] = nbt.int_array("pos")?[..] else {
            return None;
        };
        let charge = optional_int(nbt, "charge", 0)?;
        let decay_delay = optional_int(nbt, "decay_delay", 1)?;
        let update_delay = optional_int(nbt, "update_delay", 0)?;
        if !(0..=MAX_CHARGE).contains(&charge)
            || !(0..=MAX_CURSOR_DECAY_DELAY).contains(&decay_delay)
            || update_delay < 0
        {
            return None;
        }
        Some(Self {
            pos: BlockPos::new(x, y, z),
            charge,
            update_delay,
            decay_delay,
            facings: load_facings(nbt),
        })
    }
}

/// An optional int field with a default; a field of another type fails.
fn optional_int(nbt: &NbtCompoundView<'_, '_>, name: &str, default: i32) -> Option<i32> {
    match nbt.get(name) {
        None => Some(default),
        Some(tag) => tag.int(),
    }
}

/// Vanilla's lenient `facings`: a list of direction names, or nothing when
/// it does not read.
fn load_facings(nbt: &NbtCompoundView<'_, '_>) -> Option<Faces> {
    let list = nbt.list("facings")?;
    let mut faces = Faces::default();
    if list.empty() {
        return Some(faces);
    }
    for name in list.strings()? {
        let name = name.to_str();
        let direction = Direction::ALL
            .into_iter()
            .find(|direction| direction.as_str() == name)?;
        faces.insert(direction);
    }
    Some(faces)
}

/// Vanilla `SculkSpreader`.
#[derive(Clone, Debug)]
pub struct SculkSpreader {
    is_world_generation: bool,
    replaceable_blocks: Identifier,
    growth_spawn_cost: i32,
    no_growth_radius: i32,
    charge_decay_rate: i32,
    additional_decay_rate: i32,
    cursors: Vec<ChargeCursor>,
}

impl SculkSpreader {
    /// Vanilla `createLevelSpreader`, a sculk catalyst's.
    #[must_use]
    pub const fn level() -> Self {
        Self {
            is_world_generation: false,
            replaceable_blocks: BlockTag::SCULK_REPLACEABLE,
            growth_spawn_cost: 10,
            no_growth_radius: 4,
            charge_decay_rate: 10,
            additional_decay_rate: 5,
            cursors: Vec::new(),
        }
    }

    /// Vanilla `createWorldGenSpreader`, a sculk patch's.
    #[must_use]
    pub const fn world_gen() -> Self {
        Self {
            is_world_generation: true,
            replaceable_blocks: BlockTag::SCULK_REPLACEABLE_WORLD_GEN,
            growth_spawn_cost: 50,
            no_growth_radius: 1,
            charge_decay_rate: 5,
            additional_decay_rate: 10,
            cursors: Vec::new(),
        }
    }

    /// The cursors still spreading.
    #[must_use]
    pub fn cursors(&self) -> &[ChargeCursor] {
        &self.cursors
    }

    /// Moves the cursors of `other` after this spreader's, up to
    /// [`MAX_CURSORS`].
    pub fn append(&mut self, other: &mut Self) {
        for cursor in other.cursors.drain(..) {
            self.add_cursor(cursor);
        }
    }

    /// Drops every cursor.
    pub fn clear(&mut self) {
        self.cursors.clear();
    }

    /// Vanilla `load`: a list longer than [`MAX_CURSORS`], or holding a
    /// cursor that does not read, loads as no cursors.
    pub fn load(&mut self, nbt: &NbtCompoundView<'_, '_>) {
        self.cursors.clear();
        let Some(list) = nbt.list(CURSORS_TAG) else {
            return;
        };
        if list.empty() {
            return;
        }
        let Some(compounds) = list.compounds() else {
            return;
        };
        if compounds.len() > MAX_CURSORS {
            return;
        }
        let Some(cursors) = compounds
            .into_iter()
            .map(|cursor| ChargeCursor::load(&cursor))
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        for cursor in cursors {
            self.add_cursor(cursor);
        }
    }

    /// Vanilla `save`.
    pub fn save(&self, nbt: &mut NbtCompound) {
        let cursors = self
            .cursors
            .iter()
            .map(ChargeCursor::save)
            .collect::<Vec<_>>();
        nbt.insert(
            CURSORS_TAG,
            if cursors.is_empty() {
                NbtList::Empty
            } else {
                NbtList::Compound(cursors)
            },
        );
    }

    /// Vanilla `addCursors`: `charge` split into cursors of at most
    /// [`MAX_CHARGE`].
    pub fn add_cursors(&mut self, start: BlockPos, mut charge: i32) {
        while charge > 0 {
            let current = charge.min(MAX_CHARGE);
            self.add_cursor(ChargeCursor::new(start, current));
            charge -= current;
        }
    }

    fn add_cursor(&mut self, cursor: ChargeCursor) {
        if self.cursors.len() < MAX_CURSORS {
            self.cursors.push(cursor);
        }
    }

    /// Vanilla `updateCursors`: moves every cursor once, then merges the
    /// cursors that met.
    pub fn update_cursors<L: SculkLevel, R: Random>(
        &mut self,
        level: &L,
        origin: BlockPos,
        random: &mut R,
        spread_veins: bool,
    ) {
        if self.cursors.is_empty() {
            return;
        }

        let cursors = mem::take(&mut self.cursors);
        let mut processed: Vec<ChargeCursor> = Vec::with_capacity(cursors.len());
        // Index into `processed` of the cursor that later ones at a position merge into.
        let mut mergeable: FxHashMap<BlockPos, usize> = FxHashMap::default();
        // Total charge per position, in the order the positions first come up.
        let mut charges: Vec<(BlockPos, i32)> = Vec::new();

        for mut cursor in cursors {
            if cursor.is_pos_unreasonable(origin) {
                continue;
            }
            cursor.update(level, origin, random, self, spread_veins);
            if cursor.charge <= 0 {
                level.sculk_charge_event(cursor.pos, 0);
                continue;
            }

            let pos = cursor.pos;
            match charges.iter_mut().find(|(at, _)| *at == pos) {
                Some((_, total)) => *total += cursor.charge,
                None => charges.push((pos, cursor.charge)),
            }
            match mergeable.get(&pos).copied() {
                None => {
                    mergeable.insert(pos, processed.len());
                    processed.push(cursor);
                }
                Some(existing)
                    if !self.is_world_generation
                        && cursor.charge + processed[existing].charge <= MAX_CHARGE =>
                {
                    processed[existing].merge_with(&mut cursor);
                }
                Some(existing) => {
                    let smaller = cursor.charge < processed[existing].charge;
                    if smaller {
                        mergeable.insert(pos, processed.len());
                    }
                    processed.push(cursor);
                }
            }
        }

        for (pos, charge) in charges {
            let faces = mergeable
                .get(&pos)
                .and_then(|&index| processed[index].facings);
            if charge > 0
                && let Some(faces) = faces
            {
                let particles = (f64::from(charge).ln_1p() / f64::from(2.3_f32)) as i32 + 1;
                level.sculk_charge_event(pos, (particles << 6) + faces.pack());
            }
        }

        self.cursors = processed;
    }
}

/// Vanilla `SculkBehaviour`, by block: sculk, sculk veins and `DEFAULT` for
/// every other block.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SculkBehavior {
    Default,
    Sculk,
    Vein,
}

impl SculkBehavior {
    fn of(state: BlockStateId) -> Self {
        let block = state.get_block();
        if block == &vanilla_blocks::SCULK {
            Self::Sculk
        } else if block == &vanilla_blocks::SCULK_VEIN {
            Self::Vein
        } else {
            Self::Default
        }
    }

    const fn can_change_block_state_on_spread(self) -> bool {
        !matches!(self, Self::Sculk)
    }

    fn update_decay_delay(self, age: i32) -> i32 {
        match self {
            Self::Default => (age - 1).max(0),
            Self::Sculk | Self::Vein => 1,
        }
    }

    fn attempt_spread_vein<L: SculkLevel>(
        self,
        level: &L,
        pos: BlockPos,
        state: BlockStateId,
        facings: Option<Faces>,
        post_process: bool,
    ) -> bool {
        if self == Self::Default {
            match facings {
                None => {
                    return vein_spread_all(
                        level,
                        level.get_block_state(pos),
                        pos,
                        post_process,
                        SAME_SPACE_SPREAD_TYPES,
                    ) > 0;
                }
                Some(faces) if !faces.is_empty() => {
                    if !state.is_air() && !get_fluid_state_from_block(state).is_water() {
                        return false;
                    }
                    return vein_regrow(level, pos, state, faces);
                }
                Some(_) => {}
            }
        }
        vein_spread_all(level, state, pos, post_process, DEFAULT_SPREAD_TYPES) > 0
    }

    fn attempt_use_charge<L: SculkLevel, R: Random>(
        self,
        cursor: &ChargeCursor,
        level: &L,
        origin: BlockPos,
        random: &mut R,
        spreader: &SculkSpreader,
        spread_veins: bool,
    ) -> i32 {
        match self {
            Self::Default => {
                if cursor.decay_delay > 0 {
                    cursor.charge
                } else {
                    0
                }
            }
            Self::Sculk => sculk_attempt_use_charge(cursor, level, origin, random, spreader),
            Self::Vein => {
                if spread_veins && vein_attempt_place_sculk(spreader, level, cursor.pos, random) {
                    cursor.charge - 1
                } else if random.next_i32_bounded(spreader.charge_decay_rate) == 0 {
                    (cursor.charge as f32 * 0.5).floor() as i32
                } else {
                    cursor.charge
                }
            }
        }
    }

    fn on_discharged<L: SculkLevel>(self, level: &L, state: BlockStateId, pos: BlockPos) {
        if self == Self::Vein {
            vein_on_discharged(level, state, pos);
        }
    }
}

/// Vanilla `SculkBlock.attemptUseCharge`.
fn sculk_attempt_use_charge<L: SculkLevel, R: Random>(
    cursor: &ChargeCursor,
    level: &L,
    origin: BlockPos,
    random: &mut R,
    spreader: &SculkSpreader,
) -> i32 {
    let charge = cursor.charge;
    if charge == 0 || random.next_i32_bounded(spreader.charge_decay_rate) != 0 {
        return charge;
    }

    let charge_pos = cursor.pos;
    let close_to_catalyst = distance_squared(charge_pos, origin)
        < i64::from(spreader.no_growth_radius) * i64::from(spreader.no_growth_radius);
    if !close_to_catalyst && can_place_growth(level, charge_pos) {
        let cost = spreader.growth_spawn_cost;
        if random.next_i32_bounded(cost) < charge {
            let growth_pos = charge_pos.above();
            let growth_state =
                random_growth_state(level, growth_pos, random, spreader.is_world_generation);
            if level.set_block_state(growth_pos, growth_state, UpdateFlags::UPDATE_ALL) {
                level.on_growth_placed(growth_pos, growth_state);
            }
            level.play_block_sound(
                growth_state.get_block().config.sound_type.place_sound,
                charge_pos,
                1.0,
                1.0,
                None,
            );
        }
        0.max(charge - cost)
    } else if random.next_i32_bounded(spreader.additional_decay_rate) != 0 {
        charge
    } else if close_to_catalyst {
        charge - 1
    } else {
        charge - decay_penalty(spreader, charge_pos, origin, charge)
    }
}

/// Vanilla `SculkBlock.getDecayPenalty`.
fn decay_penalty(spreader: &SculkSpreader, pos: BlockPos, origin: BlockPos, charge: i32) -> i32 {
    let no_growth_radius = spreader.no_growth_radius;
    let outer_distance =
        (distance_squared(pos, origin) as f64).sqrt() as f32 - no_growth_radius as f32;
    let max_reach = (MAX_GROWTH_RATE_RADIUS - no_growth_radius) as f32;
    let distance_factor = (outer_distance * outer_distance / (max_reach * max_reach)).min(1.0);
    1.max((charge as f32 * distance_factor * 0.5) as i32)
}

/// Vanilla `SculkBlock.getRandomGrowthState`.
fn random_growth_state<L: SculkLevel, R: Random>(
    level: &L,
    pos: BlockPos,
    random: &mut R,
    world_gen: bool,
) -> BlockStateId {
    let state = if random.next_i32_bounded(SHRIEKER_PLACEMENT_RATE) == 0 {
        vanilla_blocks::SCULK_SHRIEKER
            .default_state()
            .set_value(&BlockStateProperties::CAN_SUMMON, world_gen)
    } else {
        vanilla_blocks::SCULK_SENSOR.default_state()
    };
    if state
        .try_get_value(&BlockStateProperties::WATERLOGGED)
        .is_some()
        && !get_fluid_state_from_block(level.get_block_state(pos)).is_empty()
    {
        state.set_value(&BlockStateProperties::WATERLOGGED, true)
    } else {
        state
    }
}

/// Vanilla `SculkBlock.canPlaceGrowth`: open above, and at most two growth
/// inhibitors around.
fn can_place_growth<L: SculkLevel>(level: &L, pos: BlockPos) -> bool {
    let above = level.get_block_state(pos.above());
    let open = above.is_air()
        || (above.get_block() == &vanilla_blocks::WATER
            && get_fluid_state_from_block(above).is_water());
    if !open {
        return false;
    }
    let mut inhibitors = 0;
    for z in -GROWTH_INHIBITOR_RANGE..=GROWTH_INHIBITOR_RANGE {
        for y in 0..=2 {
            for x in -GROWTH_INHIBITOR_RANGE..=GROWTH_INHIBITOR_RANGE {
                if level
                    .get_block_state(pos.offset(x, y, z))
                    .get_block()
                    .has_tag(&BlockTag::SCULK_GROWTH_INHIBITORS)
                {
                    inhibitors += 1;
                    if inhibitors > 2 {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// Vanilla `MultifaceSpreader.SpreadType`.
#[derive(Clone, Copy)]
enum SpreadType {
    SamePosition,
    SamePlane,
    WrapAround,
}

/// Vanilla `MultifaceSpreader.DEFAULT_SPREAD_ORDER`, the vein spreader's.
const DEFAULT_SPREAD_TYPES: &[SpreadType] = &[
    SpreadType::SamePosition,
    SpreadType::SamePlane,
    SpreadType::WrapAround,
];
/// The sculk vein's same-space spreader.
const SAME_SPACE_SPREAD_TYPES: &[SpreadType] = &[SpreadType::SamePosition];

/// Vanilla `MultifaceSpreader.SpreadPos`.
struct SpreadPos {
    pos: BlockPos,
    face: Direction,
}

impl SpreadType {
    fn spread_pos(
        self,
        pos: BlockPos,
        spread_direction: Direction,
        from_face: Direction,
    ) -> SpreadPos {
        match self {
            Self::SamePosition => SpreadPos {
                pos,
                face: spread_direction,
            },
            Self::SamePlane => SpreadPos {
                pos: pos.relative(spread_direction),
                face: from_face,
            },
            Self::WrapAround => SpreadPos {
                pos: pos.relative(spread_direction).relative(from_face),
                face: spread_direction.opposite(),
            },
        }
    }
}

fn has_face(state: BlockStateId, direction: Direction) -> bool {
    state
        .try_get_value(multiface_face_property(direction))
        .unwrap_or(false)
}

fn is_vein(state: BlockStateId) -> bool {
    state.get_block() == &vanilla_blocks::SCULK_VEIN
}

/// Vanilla `MultifaceSpreader.spreadAll` with the sculk vein's config.
fn vein_spread_all<L: SculkLevel>(
    level: &L,
    state: BlockStateId,
    pos: BlockPos,
    post_process: bool,
    spread_types: &[SpreadType],
) -> i64 {
    let mut count = 0;
    for starting_face in Direction::ALL {
        // Vanilla `canSpreadFrom`: any face of another block, a vein's own faces.
        if is_vein(state) && !has_face(state, starting_face) {
            continue;
        }
        for spread_direction in Direction::ALL {
            if let Some(spread_pos) = vein_spread_target(
                level,
                state,
                pos,
                starting_face,
                spread_direction,
                spread_types,
            ) && vein_spread_to_face(level, &spread_pos, post_process)
            {
                count += 1;
            }
        }
    }
    count
}

/// Vanilla `getSpreadFromFaceTowardDirection`.
fn vein_spread_target<L: SculkLevel>(
    level: &L,
    state: BlockStateId,
    pos: BlockPos,
    starting_face: Direction,
    spread_direction: Direction,
    spread_types: &[SpreadType],
) -> Option<SpreadPos> {
    if spread_direction.axis() == starting_face.axis() {
        return None;
    }
    if is_vein(state) && (!has_face(state, starting_face) || has_face(state, spread_direction)) {
        return None;
    }
    spread_types
        .iter()
        .map(|spread_type| spread_type.spread_pos(pos, spread_direction, starting_face))
        .find(|spread_pos| vein_can_spread_into(level, pos, spread_pos))
}

/// Vanilla `SpreadConfig.canSpreadInto`.
fn vein_can_spread_into<L: SculkLevel>(
    level: &L,
    source: BlockPos,
    spread_pos: &SpreadPos,
) -> bool {
    let existing = level.get_block_state(spread_pos.pos);
    vein_state_can_be_replaced(level, source, spread_pos.pos, spread_pos.face, existing)
        && vein_is_valid_state_for_placement(level, existing, spread_pos.pos, spread_pos.face)
}

/// Vanilla `SculkVeinSpreaderConfig.stateCanBeReplaced`.
fn vein_state_can_be_replaced<L: SculkLevel>(
    level: &L,
    source: BlockPos,
    placement_pos: BlockPos,
    placement_direction: Direction,
    existing: BlockStateId,
) -> bool {
    let against = level
        .get_block_state(placement_pos.relative(placement_direction))
        .get_block();
    if against == &vanilla_blocks::SCULK
        || against == &vanilla_blocks::SCULK_CATALYST
        || against == &vanilla_blocks::MOVING_PISTON
    {
        return false;
    }
    if manhattan_distance(source, placement_pos) == 2 {
        let neighbor_pos = source.relative(placement_direction.opposite());
        if level.is_face_sturdy(
            level.get_block_state(neighbor_pos),
            neighbor_pos,
            placement_direction,
        ) {
            return false;
        }
    }
    let fluid = get_fluid_state_from_block(existing);
    if !fluid.is_empty() && !fluid.is_water() {
        return false;
    }
    if existing.get_block().has_tag(&BlockTag::FIRE) {
        return false;
    }
    existing.is_replaceable()
        || existing.is_air()
        || is_vein(existing)
        || (existing.get_block() == &vanilla_blocks::WATER
            && get_fluid_state_from_block(existing).is_source())
}

/// Vanilla `MultifaceBlock.isValidStateForPlacement` for the sculk vein.
fn vein_is_valid_state_for_placement<L: SculkLevel>(
    level: &L,
    old_state: BlockStateId,
    pos: BlockPos,
    direction: Direction,
) -> bool {
    if is_vein(old_state) && has_face(old_state, direction) {
        return false;
    }
    MultifaceBlock::can_attach_to(level, pos, direction)
}

/// Vanilla `MultifaceSpreader.spreadToFace`.
fn vein_spread_to_face<L: SculkLevel>(
    level: &L,
    spread_pos: &SpreadPos,
    post_process: bool,
) -> bool {
    let old_state = level.get_block_state(spread_pos.pos);
    if !vein_is_valid_state_for_placement(level, old_state, spread_pos.pos, spread_pos.face) {
        return false;
    }
    let new_state = if is_vein(old_state) {
        old_state
    } else {
        let fluid = get_fluid_state_from_block(old_state);
        vanilla_blocks::SCULK_VEIN.default_state().set_value(
            &BlockStateProperties::WATERLOGGED,
            fluid.is_water() && fluid.is_source(),
        )
    }
    .set_value(multiface_face_property(spread_pos.face), true);
    if post_process {
        level.mark_pos_for_postprocessing(spread_pos.pos);
    }
    level.set_block_state(spread_pos.pos, new_state, UpdateFlags::UPDATE_CLIENTS)
}

/// Vanilla `SculkVeinBlock.regrow`.
fn vein_regrow<L: SculkLevel>(
    level: &L,
    pos: BlockPos,
    existing: BlockStateId,
    faces: Faces,
) -> bool {
    let mut new_state = vanilla_blocks::SCULK_VEIN.default_state();
    let mut has_any = false;
    for face in faces.iter() {
        if MultifaceBlock::can_attach_to(level, pos, face) {
            new_state = new_state.set_value(multiface_face_property(face), true);
            has_any = true;
        }
    }
    if !has_any {
        return false;
    }
    if !get_fluid_state_from_block(existing).is_empty() {
        new_state = new_state.set_value(&BlockStateProperties::WATERLOGGED, true);
    }
    level.set_block_state(pos, new_state, UpdateFlags::UPDATE_ALL);
    true
}

/// Vanilla `SculkVeinBlock.onDischarged`: the vein drops the faces that now
/// rest on sculk.
fn vein_on_discharged<L: SculkLevel>(level: &L, mut state: BlockStateId, pos: BlockPos) {
    if !is_vein(state) {
        return;
    }
    for direction in Direction::ALL {
        if has_face(state, direction)
            && level.get_block_state(pos.relative(direction)).get_block() == &vanilla_blocks::SCULK
        {
            state = state.set_value(multiface_face_property(direction), false);
        }
    }
    if Faces::of_state(state).is_empty() {
        state = if get_fluid_state_from_block(level.get_block_state(pos)).is_empty() {
            vanilla_blocks::AIR.default_state()
        } else {
            vanilla_blocks::WATER.default_state()
        };
    }
    level.set_block_state(pos, state, UpdateFlags::UPDATE_ALL);
}

/// Vanilla `SculkVeinBlock.attemptPlaceSculk`: turns the block under one of
/// the vein's faces into sculk.
fn vein_attempt_place_sculk<L: SculkLevel, R: Random>(
    spreader: &SculkSpreader,
    level: &L,
    pos: BlockPos,
    random: &mut R,
) -> bool {
    let state = level.get_block_state(pos);
    for support in shuffled(Direction::ALL, random) {
        if !has_face(state, support) {
            continue;
        }
        let support_pos = pos.relative(support);
        let support_state = level.get_block_state(support_pos);
        if !support_state
            .get_block()
            .has_tag(&spreader.replaceable_blocks)
        {
            continue;
        }

        let sculk = vanilla_blocks::SCULK.default_state();
        level.set_block_state(support_pos, sculk, UpdateFlags::UPDATE_ALL);
        level.push_entities_up(support_state, sculk, support_pos);
        level.play_block_sound(
            &sound_events::BLOCK_SCULK_SPREAD,
            support_pos,
            1.0,
            1.0,
            None,
        );
        vein_spread_all(
            level,
            sculk,
            support_pos,
            spreader.is_world_generation,
            DEFAULT_SPREAD_TYPES,
        );
        let skip = support.opposite();
        for direction in Direction::ALL {
            if direction == skip {
                continue;
            }
            let vein_pos = support_pos.relative(direction);
            let possible_vein = level.get_block_state(vein_pos);
            if is_vein(possible_vein) {
                vein_on_discharged(level, possible_vein, vein_pos);
            }
        }
        return true;
    }
    false
}

/// Vanilla `SculkVeinBlock.hasSubstrateAccess`.
fn vein_has_substrate_access<L: SculkLevel>(level: &L, state: BlockStateId, pos: BlockPos) -> bool {
    is_vein(state)
        && Direction::ALL.into_iter().any(|direction| {
            has_face(state, direction)
                && level
                    .get_block_state(pos.relative(direction))
                    .get_block()
                    .has_tag(&BlockTag::SCULK_REPLACEABLE)
        })
}

/// Vanilla `ChargeCursor.getValidMovementPos`.
fn valid_movement_pos<L: SculkLevel, R: Random>(
    level: &L,
    pos: BlockPos,
    random: &mut R,
    origin: BlockPos,
    spreader: &SculkSpreader,
) -> Option<BlockPos> {
    let mut sculk_position = pos;
    for offset in shuffled(NON_CORNER_NEIGHBORS, random) {
        let neighbor = pos.offset(offset[0], offset[1], offset[2]);
        if spreader.is_world_generation {
            let dx = i64::from(origin.x()) - i64::from(neighbor.x());
            let dz = i64::from(origin.z()) - i64::from(neighbor.z());
            if dx * dx + dz * dz > MAX_WORLDGEN_SPREAD_SQUARED {
                continue;
            }
        }
        let transferee = level.get_block_state(neighbor);
        if SculkBehavior::of(transferee) == SculkBehavior::Default
            || !is_movement_unobstructed(level, pos, neighbor)
        {
            continue;
        }
        sculk_position = neighbor;
        if vein_has_substrate_access(level, transferee, neighbor) {
            break;
        }
    }
    (sculk_position != pos).then_some(sculk_position)
}

/// Vanilla's non-corner neighbor offsets of a charge cursor: the 18 neighbors sharing a
/// face or an edge, x fastest, then y, then z.
const NON_CORNER_NEIGHBORS: [[i32; 3]; 18] = {
    let mut offsets = [[0; 3]; 18];
    let mut count = 0;
    let mut z = -1;
    while z <= 1 {
        let mut y = -1;
        while y <= 1 {
            let mut x = -1;
            while x <= 1 {
                if (x == 0 || y == 0 || z == 0) && !(x == 0 && y == 0 && z == 0) {
                    offsets[count] = [x, y, z];
                    count += 1;
                }
                x += 1;
            }
            y += 1;
        }
        z += 1;
    }
    offsets
};

/// Vanilla `Util.shuffledCopy`.
fn shuffled<T, const N: usize, R: Random>(mut items: [T; N], random: &mut R) -> [T; N] {
    for i in (1..N).rev() {
        let bound = i32::try_from(i + 1).expect("shuffled arrays are small");
        let j = random.next_i32_bounded(bound) as usize;
        items.swap(i, j);
    }
    items
}

/// Vanilla `ChargeCursor.isMovementUnobstructed`.
fn is_movement_unobstructed<L: SculkLevel>(level: &L, from: BlockPos, to: BlockPos) -> bool {
    if manhattan_distance(from, to) == 1 {
        return true;
    }
    let dx = to.x() - from.x();
    let dy = to.y() - from.y();
    let dz = to.z() - from.z();
    let direction_x = if dx < 0 {
        Direction::West
    } else {
        Direction::East
    };
    let direction_y = if dy < 0 {
        Direction::Down
    } else {
        Direction::Up
    };
    let direction_z = if dz < 0 {
        Direction::North
    } else {
        Direction::South
    };
    if dx == 0 {
        is_unobstructed(level, from, direction_y) || is_unobstructed(level, from, direction_z)
    } else if dy == 0 {
        is_unobstructed(level, from, direction_x) || is_unobstructed(level, from, direction_z)
    } else {
        is_unobstructed(level, from, direction_x) || is_unobstructed(level, from, direction_y)
    }
}

/// Vanilla `ChargeCursor.isUnobstructed`.
fn is_unobstructed<L: SculkLevel>(level: &L, from: BlockPos, direction: Direction) -> bool {
    let test_pos = from.relative(direction);
    !level.is_face_sturdy(
        level.get_block_state(test_pos),
        test_pos,
        direction.opposite(),
    )
}

const fn manhattan_distance(left: BlockPos, right: BlockPos) -> i32 {
    (left.x() - right.x()).abs() + (left.y() - right.y()).abs() + (left.z() - right.z()).abs()
}

fn distance_squared(left: BlockPos, right: BlockPos) -> i64 {
    let dx = i64::from(left.x()) - i64::from(right.x());
    let dy = i64::from(left.y()) - i64::from(right.y());
    let dz = i64::from(left.z()) - i64::from(right.z());
    dx * dx + dy * dy + dz * dz
}
