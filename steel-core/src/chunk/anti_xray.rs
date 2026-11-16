//! Anti-xray: ores that touch nothing a player could see through are sent to clients as the
//! rock around them, so an x-ray client finds only stone.
//!
//! Hiding happens when a chunk is encoded for clients ([`hide_section`]); the server's world
//! keeps the real blocks. When a block changes, the chunk map shows players the real state of
//! the hidden blocks next to it ([`revealed_by`]), so mining into a vein shows the ore as soon
//! as its face opens, just as without anti-xray.
//!
//! A server installs the settings once at start ([`install`]); without them nothing is hidden.

use std::sync::{Arc, OnceLock};

use rustc_hash::FxHashMap;
use steel_registry::{REGISTRY, RegistryExt as _, blocks::block_state_ext::BlockStateExt as _};
use steel_utils::{BlockPos, BlockStateId, Direction, Identifier};

use crate::chunk::{chunk_holder::ChunkHolder, paletted_container::BlockPalette};

/// What one dimension hides: a block state and what clients see in its place.
#[derive(Debug, Default)]
pub struct HiddenBlocks {
    /// Indexed by state id; `0` (air) means the state is shown as it is.
    shown_as: Box<[u16]>,
}

impl HiddenBlocks {
    /// What clients see instead of `state`, if it is hidden.
    #[must_use]
    #[inline]
    pub fn replacement(&self, state: BlockStateId) -> Option<BlockStateId> {
        match self.shown_as.get(state.0 as usize) {
            Some(&shown) if shown != 0 => Some(BlockStateId(shown)),
            _ => None,
        }
    }

    /// Whether the dimension hides anything at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shown_as.iter().all(|&shown| shown == 0)
    }

    /// Hides every state of each block as the default state of its replacement.
    ///
    /// # Errors
    /// Names the first block that is not in the registry.
    pub fn from_keys(pairs: &[(Identifier, Identifier)]) -> Result<Self, String> {
        let mut shown_as: Vec<u16> = Vec::new();
        for (hidden, shown) in pairs {
            let block = REGISTRY
                .blocks
                .by_key(hidden)
                .ok_or_else(|| format!("unknown block {hidden}"))?;
            let replacement = REGISTRY
                .blocks
                .by_key(shown)
                .ok_or_else(|| format!("unknown block {shown}"))?;
            let shown = REGISTRY.blocks.get_default_state_id(replacement).0;
            let base = REGISTRY.blocks.get_base_state_id(block).0;
            for offset in 0..block.state_count() {
                let id = usize::from(base + offset);
                if shown_as.len() <= id {
                    shown_as.resize(id + 1, 0);
                }
                shown_as[id] = shown;
            }
        }
        Ok(Self {
            shown_as: shown_as.into_boxed_slice(),
        })
    }
}

/// Anti-xray for every dimension that hides something, by dimension key.
#[derive(Debug, Default)]
pub struct AntiXray {
    worlds: FxHashMap<Identifier, Arc<HiddenBlocks>>,
    /// Indexed by state id: whether the state is a full opaque cube that hides its neighbours.
    opaque: Box<[bool]>,
}

static ANTI_XRAY: OnceLock<AntiXray> = OnceLock::new();

/// Turns anti-xray on with what each dimension hides. Returns `false` if it was already on.
pub fn install(worlds: impl IntoIterator<Item = (Identifier, HiddenBlocks)>) -> bool {
    let worlds: FxHashMap<Identifier, Arc<HiddenBlocks>> = worlds
        .into_iter()
        .filter(|(_, hidden)| !hidden.is_empty())
        .map(|(world, hidden)| (world, Arc::new(hidden)))
        .collect();
    let states = REGISTRY
        .blocks
        .iter()
        .map(|(_, block)| {
            usize::from(REGISTRY.blocks.get_base_state_id(block).0)
                + usize::from(block.state_count())
        })
        .max()
        .unwrap_or(0);
    let opaque = (0..states)
        .map(|id| BlockStateId(id as u16).is_solid_render())
        .collect();
    ANTI_XRAY.set(AntiXray { worlds, opaque }).is_ok()
}

/// What the dimension `world` hides, when anti-xray is on there.
#[must_use]
pub fn for_world(world: &Identifier) -> Option<Arc<HiddenBlocks>> {
    ANTI_XRAY.get()?.worlds.get(world).cloned()
}

/// Whether `state` hides what is behind it.
#[must_use]
#[inline]
fn opaque(state: BlockStateId) -> bool {
    ANTI_XRAY
        .get()
        .and_then(|anti_xray| anti_xray.opaque.get(state.0 as usize).copied())
        .unwrap_or(false)
}

/// The four chunks around one being sent, for the blocks on its edges.
#[derive(Default)]
pub struct Neighbours {
    /// West (-x), east (+x), north (-z), south (+z).
    pub holders: [Option<Arc<ChunkHolder>>; 4],
}

/// One chunk's hiding: what its dimension hides and the chunks around it.
pub struct ChunkHiding {
    /// What the dimension hides.
    pub hidden: Arc<HiddenBlocks>,
    /// The chunks around it.
    pub neighbours: Neighbours,
}

/// Where a block next to a section sits, for [`hide_section`]'s lookups.
pub trait BlockSource {
    /// The state at `pos`, or `None` when it is not known (an unloaded chunk).
    fn state(&self, pos: BlockPos) -> Option<BlockStateId>;
}

/// The section `states` (its lowest corner at `origin`) as clients should see it, or `None`
/// when it shows everything as it is. `outside` answers for blocks beyond the section.
///
/// A hidden block stays visible when one of its six neighbours lets light through; a neighbour
/// nobody knows counts as rock, so an ore at the border of an unloaded chunk stays hidden.
#[must_use]
pub fn hide_section(
    hidden: &HiddenBlocks,
    states: &BlockPalette,
    origin: BlockPos,
    outside: &dyn BlockSource,
) -> Option<BlockPalette> {
    if !states.maybe_has(|state| hidden.replacement(state).is_some()) {
        return None;
    }
    let size = BlockPalette::SIZE as i32;
    let opaque_at = |x: i32, y: i32, z: i32| -> bool {
        if (0..size).contains(&x) && (0..size).contains(&y) && (0..size).contains(&z) {
            opaque(states.get(x as usize, y as usize, z as usize))
        } else {
            outside
                .state(BlockPos::new(
                    origin.x() + x,
                    origin.y() + y,
                    origin.z() + z,
                ))
                .is_none_or(opaque)
        }
    };
    let mut shown = states.clone();
    let mut changed = false;
    for y in 0..size {
        for z in 0..size {
            for x in 0..size {
                let state = states.get(x as usize, y as usize, z as usize);
                let Some(replacement) = hidden.replacement(state) else {
                    continue;
                };
                let enclosed = opaque_at(x - 1, y, z)
                    && opaque_at(x + 1, y, z)
                    && opaque_at(x, y - 1, z)
                    && opaque_at(x, y + 1, z)
                    && opaque_at(x, y, z - 1)
                    && opaque_at(x, y, z + 1);
                if enclosed {
                    shown.set(x as usize, y as usize, z as usize, replacement);
                    changed = true;
                }
            }
        }
    }
    changed.then_some(shown)
}

/// The hidden blocks a change at `changed` may have uncovered: its six neighbours that
/// `hidden` hides, with their real states. `state_at` reads the world.
pub fn revealed_by(
    hidden: &HiddenBlocks,
    changed: BlockPos,
    state_at: impl Fn(BlockPos) -> BlockStateId,
    out: &mut impl Extend<BlockPos>,
) {
    if opaque(state_at(changed)) {
        return;
    }
    for direction in Direction::ALL {
        let next = changed.relative(direction);
        if hidden.replacement(state_at(next)).is_some() {
            out.extend([next]);
        }
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_blocks};

    use rustc_hash::FxHashSet;

    use super::*;
    use crate::behavior::init_behaviors;

    struct Rock;

    impl BlockSource for Rock {
        fn state(&self, _pos: BlockPos) -> Option<BlockStateId> {
            Some(vanilla_blocks::STONE.default_state())
        }
    }

    struct Air;

    impl BlockSource for Air {
        fn state(&self, _pos: BlockPos) -> Option<BlockStateId> {
            Some(vanilla_blocks::AIR.default_state())
        }
    }

    fn setup() -> HiddenBlocks {
        init_vanilla_registry();
        init_behaviors();
        let mut worlds = FxHashMap::default();
        worlds.insert(
            Identifier::vanilla_static("overworld"),
            HiddenBlocks::from_keys(&[(
                Identifier::vanilla_static("diamond_ore"),
                Identifier::vanilla_static("stone"),
            )])
            .expect("vanilla blocks"),
        );
        install(worlds);
        HiddenBlocks::from_keys(&[(
            Identifier::vanilla_static("diamond_ore"),
            Identifier::vanilla_static("stone"),
        )])
        .expect("vanilla blocks")
    }

    #[test]
    fn an_ore_inside_rock_is_sent_as_stone_and_one_by_a_cave_is_not() {
        let hidden = setup();
        let stone = vanilla_blocks::STONE.default_state();
        let ore = vanilla_blocks::DIAMOND_ORE.default_state();
        let air = vanilla_blocks::AIR.default_state();
        let mut states = BlockPalette::Homogeneous(stone);
        states.set(5, 5, 5, ore);
        states.set(9, 9, 9, ore);
        states.set(9, 10, 9, air);
        let shown = hide_section(&hidden, &states, BlockPos::new(0, 0, 0), &Rock)
            .expect("an enclosed ore is hidden");
        assert_eq!(shown.get(5, 5, 5), stone);
        assert_eq!(shown.get(9, 9, 9), ore);
        assert_eq!(shown.get(9, 10, 9), air);
    }

    #[test]
    fn ores_on_the_edge_follow_the_blocks_beyond_it() {
        let hidden = setup();
        let stone = vanilla_blocks::STONE.default_state();
        let ore = vanilla_blocks::DIAMOND_ORE.default_state();
        let mut states = BlockPalette::Homogeneous(stone);
        states.set(0, 3, 3, ore);
        let in_rock = hide_section(&hidden, &states, BlockPos::new(0, 0, 0), &Rock)
            .expect("hidden behind rock");
        assert_eq!(in_rock.get(0, 3, 3), stone);
        assert!(hide_section(&hidden, &states, BlockPos::new(0, 0, 0), &Air).is_none());
    }

    #[test]
    fn a_section_without_hidden_blocks_is_sent_as_it_is() {
        let hidden = setup();
        let states = BlockPalette::Homogeneous(vanilla_blocks::STONE.default_state());
        assert!(hide_section(&hidden, &states, BlockPos::new(0, 0, 0), &Rock).is_none());
    }

    #[test]
    fn breaking_next_to_an_ore_reveals_it() {
        let hidden = setup();
        let ore_at = BlockPos::new(1, 0, 0);
        let air = vanilla_blocks::AIR.default_state();
        let stone = vanilla_blocks::STONE.default_state();
        let ore = vanilla_blocks::DIAMOND_ORE.default_state();
        let mut out = FxHashSet::default();
        revealed_by(
            &hidden,
            BlockPos::new(0, 0, 0),
            |pos| {
                if pos == ore_at {
                    ore
                } else if pos == BlockPos::new(0, 0, 0) {
                    air
                } else {
                    stone
                }
            },
            &mut out,
        );
        assert_eq!(out.into_iter().collect::<Vec<_>>(), [ore_at]);

        let mut none = FxHashSet::default();
        revealed_by(&hidden, BlockPos::new(0, 0, 0), |_| stone, &mut none);
        assert!(none.is_empty());
    }
}
