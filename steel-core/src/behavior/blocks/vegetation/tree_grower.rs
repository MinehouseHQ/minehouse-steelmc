//! Vanilla `TreeGrower`: which tree features a sapling or azalea grows into.

use std::sync::Arc;

use rand::{Rng, RngExt};
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::feature::{ConfiguredFeature, ConfiguredFeatureKind, TrunkPlacer};
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_configured_features as features;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::blocks::vegetation::bonemealable::place_bonemeal_feature;
use crate::fluid::state::fluid_state_to_block;
use crate::world::{LevelReader, World};

/// A configured tree feature and its weight in a grower's list.
type WeightedFeature = (&'static ConfiguredFeature, u32);

/// Vanilla `setBlock` flags 818, which `TreeGrower.removeSapling` uses:
/// clients, known shape, suppressed drops, and no block entity or on-place
/// side effects.
const REMOVE_SAPLING_FLAGS: UpdateFlags = UpdateFlags::UPDATE_CLIENTS
    .union(UpdateFlags::UPDATE_KNOWN_SHAPE)
    .union(UpdateFlags::UPDATE_SUPPRESS_DROPS)
    .union(UpdateFlags::UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS)
    .union(UpdateFlags::UPDATE_SKIP_ON_PLACE);

/// Vanilla's named tree growers, as `classes.json` names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeGrower {
    /// Oak and fancy oak; bee nests near flowers.
    Oak,
    /// Spruce, or a mega spruce or pine from a 2x2.
    Spruce,
    /// Mangrove and tall mangrove.
    Mangrove,
    /// The azalea tree.
    Azalea,
    /// Birch; bee nests near flowers.
    Birch,
    /// Jungle tree without vines, or a mega jungle tree from a 2x2.
    Jungle,
    /// Acacia.
    Acacia,
    /// Cherry; bee nests near flowers.
    Cherry,
    /// Dark oak, only from a 2x2.
    DarkOak,
    /// Pale oak, only from a 2x2.
    PaleOak,
    /// Red, orange or yellow poplar.
    Poplar,
}

/// A 2x2 of saplings: the offset of its north-west corner from the grown
/// sapling, and the four positions with their states.
struct TwoByTwo {
    dx: i32,
    dz: i32,
    saplings: [(BlockStateId, BlockPos); 4],
}

impl TreeGrower {
    /// Vanilla `trees`: the single-sapling features.
    fn trees(self) -> Vec<WeightedFeature> {
        match self {
            Self::Oak => vec![(&*features::OAK, 9), (&*features::FANCY_OAK, 1)],
            Self::Spruce => vec![(&*features::SPRUCE, 1)],
            Self::Mangrove => vec![(&*features::MANGROVE, 15), (&*features::TALL_MANGROVE, 85)],
            Self::Azalea => vec![(&*features::AZALEA_TREE, 1)],
            Self::Birch => vec![(&*features::BIRCH, 1)],
            Self::Jungle => vec![(&*features::JUNGLE_TREE_NO_VINE, 1)],
            Self::Acacia => vec![(&*features::ACACIA, 1)],
            Self::Cherry => vec![(&*features::CHERRY, 1)],
            Self::DarkOak | Self::PaleOak => Vec::new(),
            Self::Poplar => vec![
                (&*features::RED_POPLAR, 1),
                (&*features::ORANGE_POPLAR, 1),
                (&*features::YELLOW_POPLAR, 1),
            ],
        }
    }

    /// Vanilla `megaTrees`: the features a 2x2 of saplings grows.
    fn mega_trees(self) -> Vec<WeightedFeature> {
        match self {
            Self::Spruce => vec![(&*features::MEGA_SPRUCE, 1), (&*features::MEGA_PINE, 1)],
            Self::Jungle => vec![(&*features::MEGA_JUNGLE_TREE, 1)],
            Self::DarkOak => vec![(&*features::DARK_OAK, 1)],
            Self::PaleOak => vec![(&*features::PALE_OAK_BONEMEAL, 1)],
            _ => Vec::new(),
        }
    }

    /// Vanilla `flowerTrees`: the features grown next to flowers.
    fn flower_trees(self) -> Vec<WeightedFeature> {
        match self {
            Self::Oak => vec![
                (&*features::OAK_BEES_005, 9),
                (&*features::FANCY_OAK_BEES_005, 1),
            ],
            Self::Birch => vec![(&*features::BIRCH_BEES_005, 1)],
            Self::Cherry => vec![(&*features::CHERRY_BEES_005, 1)],
            _ => Vec::new(),
        }
    }

    /// Vanilla `shortestTreeType`.
    fn shortest_tree_type(self) -> Option<&'static ConfiguredFeature> {
        match self {
            Self::Oak => Some(&*features::OAK),
            Self::Spruce => Some(&*features::SPRUCE),
            Self::Mangrove => Some(&*features::MANGROVE),
            Self::Azalea => Some(&*features::AZALEA_TREE),
            Self::Birch => Some(&*features::BIRCH),
            Self::Jungle => Some(&*features::JUNGLE_TREE_NO_VINE),
            Self::Acacia => Some(&*features::ACACIA),
            Self::Cherry => Some(&*features::CHERRY),
            Self::DarkOak | Self::PaleOak => None,
            Self::Poplar => Some(&*features::RED_POPLAR),
        }
    }

    /// Vanilla `getMinimumHeight`: the base trunk height of the shortest
    /// tree this grower makes, if it has one.
    #[must_use]
    pub fn minimum_height(self) -> Option<i32> {
        let ConfiguredFeatureKind::Tree(config) = &self.shortest_tree_type()?.kind else {
            return None;
        };
        Some(match &config.trunk_placer {
            TrunkPlacer::Straight(base)
            | TrunkPlacer::Giant(base)
            | TrunkPlacer::Fancy(base)
            | TrunkPlacer::Forking(base)
            | TrunkPlacer::DarkOak(base)
            | TrunkPlacer::MegaJungle(base) => base.base_height,
            TrunkPlacer::Poplar(placer) => placer.base_height,
            TrunkPlacer::Bending(placer) => placer.base_height,
            TrunkPlacer::UpwardsBranching(placer) => placer.base_height,
            TrunkPlacer::Cherry(placer) => placer.base_height,
        })
    }

    /// Vanilla `WeightedList.getRandom`.
    fn pick(list: &[WeightedFeature], rng: &mut dyn Rng) -> Option<&'static ConfiguredFeature> {
        let total: u32 = list.iter().map(|(_, weight)| weight).sum();
        if total == 0 {
            return None;
        }
        let mut roll = rng.random_range(0..total);
        for &(feature, weight) in list {
            if roll < weight {
                return Some(feature);
            }
            roll -= weight;
        }
        None
    }

    /// Vanilla `getConfiguredFeature`: the flower list next to flowers when
    /// this grower has one, otherwise the plain list.
    fn single_tree_list(self, has_flowers: bool) -> Vec<WeightedFeature> {
        let flower_trees = self.flower_trees();
        if has_flowers && !flower_trees.is_empty() {
            flower_trees
        } else {
            self.trees()
        }
    }

    /// Vanilla `hasFlowers`: any flower within two blocks horizontally and
    /// one vertically.
    fn has_flowers(world: &dyn LevelReader, pos: BlockPos) -> bool {
        (-2..=2).any(|dx| {
            (-1..=1).any(|dy| {
                (-2..=2).any(|dz| {
                    world
                        .get_block_state(pos.offset(dx, dy, dz))
                        .get_block()
                        .has_tag(&BlockTag::FLOWERS)
                })
            })
        })
    }

    /// Vanilla `canGrow`: growers with only mega trees need a 2x2.
    #[must_use]
    pub fn can_grow(self, world: &dyn LevelReader, pos: BlockPos, state: BlockStateId) -> bool {
        let has_single_tree = !self
            .single_tree_list(Self::has_flowers(world, pos))
            .is_empty();
        has_single_tree
            || self.mega_trees().is_empty()
            || Self::find_two_by_two(world, state, pos).is_some()
    }

    /// Vanilla `findTwoByTwoSaplingPos`: the first 2x2 of `state`'s block
    /// containing `pos`, trying the corners towards negative x and z.
    fn find_two_by_two(
        world: &dyn LevelReader,
        state: BlockStateId,
        pos: BlockPos,
    ) -> Option<TwoByTwo> {
        let block = state.get_block();
        for dx in [0, -1] {
            for dz in [0, -1] {
                let saplings = [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(x, z)| {
                    let at = pos.offset(dx + x, 0, dz + z);
                    (world.get_block_state(at), at)
                });
                if saplings.iter().all(|(found, _)| found.get_block() == block) {
                    return Some(TwoByTwo { dx, dz, saplings });
                }
            }
        }
        None
    }

    /// Vanilla `removeSapling`: leaves the fluid the sapling held.
    fn remove_sapling(world: &Arc<World>, pos: BlockPos) {
        let empty = fluid_state_to_block(world.get_block_state(pos).get_fluid_state());
        world.set_block(pos, empty, REMOVE_SAPLING_FLAGS);
    }

    /// Vanilla `resetSaplings`.
    fn reset_saplings(world: &Arc<World>, saplings: &[(BlockStateId, BlockPos)]) {
        for &(state, pos) in saplings {
            world.set_block(pos, state, UpdateFlags::UPDATE_NONE);
        }
    }

    /// Vanilla `growTree`: grows a mega tree when the grower has one and the
    /// sapling is part of a 2x2, otherwise a single tree. The saplings are
    /// put back when the tree does not fit.
    pub fn grow_tree(
        self,
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        rng: &mut dyn Rng,
    ) -> bool {
        if let Some(feature) = Self::pick(&self.mega_trees(), rng)
            && let Some(two_by_two) = Self::find_two_by_two(world.as_ref(), state, pos)
        {
            for &(_, sapling) in &two_by_two.saplings {
                Self::remove_sapling(world, sapling);
            }
            let origin = pos.offset(two_by_two.dx, 0, two_by_two.dz);
            if place_bonemeal_feature(world, feature, rng, origin) {
                return true;
            }
            Self::reset_saplings(world, &two_by_two.saplings);
            return false;
        }

        let list = self.single_tree_list(Self::has_flowers(world.as_ref(), pos));
        let Some(feature) = Self::pick(&list, rng) else {
            return false;
        };
        Self::remove_sapling(world, pos);
        if place_bonemeal_feature(world, feature, rng, pos) {
            return true;
        }
        Self::reset_saplings(world, &[(state, pos)]);
        false
    }
}

#[cfg(test)]
mod tests {
    use rand::{SeedableRng, rngs::StdRng};
    use steel_registry::blocks::BlockRef;
    use steel_registry::blocks::properties::BlockStateProperties;
    use steel_registry::{init_vanilla_registry, vanilla_blocks};
    use steel_utils::ChunkPos;

    use super::*;
    use crate::behavior::blocks::vegetation::bonemealable::Bonemealable;
    use crate::behavior::{BLOCK_BEHAVIORS, init_behaviors};
    use crate::test_support::{TestWorld, fresh_test_world, insert_ready_full_chunk};

    const ORIGIN: BlockPos = BlockPos::new(8, 64, 8);

    /// A world with dirt under a 25x25 square around [`ORIGIN`].
    fn grass_field(name: &'static str) -> TestWorld {
        init_vanilla_registry();
        init_behaviors();
        let fixture = fresh_test_world(name);
        let world = &fixture.world;
        let center = ChunkPos::from_block_pos(ORIGIN);
        for chunk_x in center.0.x - 1..=center.0.x + 1 {
            for chunk_z in center.0.y - 1..=center.0.y + 1 {
                insert_ready_full_chunk(world, ChunkPos::new(chunk_x, chunk_z));
            }
        }
        for x in ORIGIN.x() - 12..=ORIGIN.x() + 12 {
            for z in ORIGIN.z() - 12..=ORIGIN.z() + 12 {
                world.set_block(
                    BlockPos::new(x, ORIGIN.y() - 1, z),
                    vanilla_blocks::DIRT.default_state(),
                    UpdateFlags::UPDATE_NONE,
                );
            }
        }
        fixture
    }

    fn ripe(block: BlockRef) -> BlockStateId {
        block
            .default_state()
            .set_value(&BlockStateProperties::STAGE, 1)
    }

    fn plant(world: &Arc<World>, positions: &[BlockPos], state: BlockStateId) {
        for &pos in positions {
            world.set_block(pos, state, UpdateFlags::UPDATE_NONE);
        }
    }

    fn bonemealable(state: BlockStateId) -> &'static dyn Bonemealable {
        let Some(bonemealable) = BLOCK_BEHAVIORS
            .get_behavior(state.get_block())
            .as_bonemealable()
        else {
            panic!("{:?} takes bonemeal", state.get_block().key);
        };
        bonemealable
    }

    fn column_has(world: &Arc<World>, pos: BlockPos, block: BlockRef) -> bool {
        (0..24).any(|dy| world.get_block_state(pos.above_n(dy)).get_block() == block)
    }

    /// Grows `state` at `pos` until a tree takes, trying several seeds.
    fn grow(grower: TreeGrower, world: &Arc<World>, pos: BlockPos, state: BlockStateId) -> bool {
        (0..32).any(|seed| grower.grow_tree(world, pos, state, &mut StdRng::seed_from_u64(seed)))
    }

    #[test]
    fn minimum_heights_follow_the_shortest_tree() {
        init_vanilla_registry();
        assert_eq!(TreeGrower::Oak.minimum_height(), Some(4));
        assert_eq!(TreeGrower::DarkOak.minimum_height(), None);
        assert_eq!(TreeGrower::PaleOak.minimum_height(), None);
    }

    #[test]
    fn bonemeal_ripens_a_young_sapling_first() {
        let fixture = grass_field("sapling_stage");
        let world = &fixture.world;
        let state = vanilla_blocks::OAK_SAPLING.default_state();
        plant(world, &[ORIGIN], state);

        bonemealable(state).perform_bonemeal(state, world, &mut StdRng::seed_from_u64(0), ORIGIN);

        assert_eq!(
            world
                .get_block_state(ORIGIN)
                .get_value(&BlockStateProperties::STAGE),
            1
        );
    }

    #[test]
    fn a_ripe_oak_sapling_grows_an_oak() {
        let fixture = grass_field("sapling_oak");
        let world = &fixture.world;
        let state = ripe(&vanilla_blocks::OAK_SAPLING);
        plant(world, &[ORIGIN], state);

        assert!(grow(TreeGrower::Oak, world, ORIGIN, state));
        assert!(column_has(world, ORIGIN, &vanilla_blocks::OAK_LOG));
    }

    #[test]
    fn a_lone_dark_oak_sapling_needs_three_more() {
        let fixture = grass_field("sapling_dark_oak");
        let world = &fixture.world;
        let state = ripe(&vanilla_blocks::DARK_OAK_SAPLING);
        plant(world, &[ORIGIN], state);
        assert!(!bonemealable(state).is_valid_bonemeal_target(state, world.as_ref(), ORIGIN));
        assert!(!grow(TreeGrower::DarkOak, world, ORIGIN, state));
        assert_eq!(world.get_block_state(ORIGIN), state, "the sapling stays");

        let square = [
            ORIGIN,
            ORIGIN.offset(-1, 0, 0),
            ORIGIN.offset(0, 0, -1),
            ORIGIN.offset(-1, 0, -1),
        ];
        plant(world, &square, state);
        assert!(bonemealable(state).is_valid_bonemeal_target(state, world.as_ref(), ORIGIN));
        assert!(grow(TreeGrower::DarkOak, world, ORIGIN, state));
        for pos in square {
            assert!(
                column_has(world, pos, &vanilla_blocks::DARK_OAK_LOG),
                "the 2x2 trunk covers {pos:?}"
            );
        }
    }

    #[test]
    fn four_spruce_saplings_grow_a_mega_tree() {
        let fixture = grass_field("sapling_mega_spruce");
        let world = &fixture.world;
        let state = ripe(&vanilla_blocks::SPRUCE_SAPLING);
        let square = [
            ORIGIN,
            ORIGIN.offset(1, 0, 0),
            ORIGIN.offset(0, 0, 1),
            ORIGIN.offset(1, 0, 1),
        ];
        plant(world, &square, state);

        assert!(grow(TreeGrower::Spruce, world, ORIGIN, state));
        for pos in square {
            assert!(
                column_has(world, pos, &vanilla_blocks::SPRUCE_LOG),
                "the 2x2 trunk covers {pos:?}"
            );
        }
    }

    #[test]
    fn flowers_switch_to_the_bee_nest_trees() {
        let fixture = grass_field("sapling_flowers");
        let world = &fixture.world;
        assert!(!TreeGrower::has_flowers(world.as_ref(), ORIGIN));
        world.set_block(
            ORIGIN.offset(2, 1, -2),
            vanilla_blocks::DANDELION.default_state(),
            UpdateFlags::UPDATE_NONE,
        );
        assert!(TreeGrower::has_flowers(world.as_ref(), ORIGIN));

        let with_flowers = TreeGrower::Oak.single_tree_list(true);
        assert_eq!(with_flowers[0].0.key, features::OAK_BEES_005.key);
        let spruce = TreeGrower::Spruce.single_tree_list(true);
        assert_eq!(
            spruce[0].0.key,
            features::SPRUCE.key,
            "spruce has no bee trees"
        );
    }

    #[test]
    fn bonemealed_azalea_grows_an_azalea_tree() {
        let fixture = grass_field("azalea_tree");
        let world = &fixture.world;
        let state = vanilla_blocks::AZALEA.default_state();
        plant(world, &[ORIGIN], state);
        let azalea = bonemealable(state);
        assert!(azalea.is_valid_bonemeal_target(state, world.as_ref(), ORIGIN));

        let grown = (0..32).any(|seed| {
            azalea.perform_bonemeal(state, world, &mut StdRng::seed_from_u64(seed), ORIGIN);
            world.get_block_state(ORIGIN).get_block() != &vanilla_blocks::AZALEA
        });
        assert!(grown);
        assert!(column_has(world, ORIGIN, &vanilla_blocks::OAK_LOG));
    }

    #[test]
    fn water_above_stops_azalea_bonemeal() {
        let fixture = grass_field("azalea_water");
        let world = &fixture.world;
        let state = vanilla_blocks::AZALEA.default_state();
        plant(world, &[ORIGIN], state);
        world.set_block(
            ORIGIN.above(),
            vanilla_blocks::WATER.default_state(),
            UpdateFlags::UPDATE_NONE,
        );
        assert!(!bonemealable(state).is_valid_bonemeal_target(state, world.as_ref(), ORIGIN));
    }
}
