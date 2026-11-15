//! The magic herb: the wild plant found in the world, the crop grown from its seeds, and
//! the block of magic leaves. A port of the Fabric `WildHerbBlock`, `HerbCropBlock` and
//! `LeafBlock`. The cauldron and what the leaves are for are not ported yet, nor is the
//! world generation of the wild plant.

use std::sync::Arc;

use rand::{Rng, RngExt as _};
use steel_core::{
    behavior::{
        BlockBehavior, BlockLootContext, BlockPlaceContext,
        blocks::vegetation::{CropLike, bonemealable::Bonemealable},
    },
    world::{LevelReader, ScheduledTickAccess, World},
};
use steel_registry::{
    blocks::{
        Block, BlockLightProperties, BlockRef, BlockRegistry, StateBooleanData, StateByteData,
        behavior::{BlockConfig, PushReaction},
        block_state_ext::BlockStateExt as _,
        properties::IntProperty,
        shapes::VoxelShape,
    },
    item_stack::ItemStack,
    sound_types,
    vanilla_block_tags::BlockTag,
    vanilla_blocks,
};
use steel_utils::{
    BlockLocalAabb, BlockPos, BlockStateId, Direction, Identifier, types::UpdateFlags,
};

use super::{
    LEAF, SEEDS,
    blocks::{show_block, show_full_block},
};

/// Vanilla `MapColor.PLANT`, `COLOR_CYAN` and `COLOR_PURPLE`.
const MAP_COLOR_PLANT: u8 = 7;
const MAP_COLOR_CYAN: u8 = 23;
const MAP_COLOR_PURPLE: u8 = 24;

/// How far the crop has grown: 0 a sprout, 3 in bloom.
pub static AGE: IntProperty = IntProperty::new("age", 0, 3);
const MAX_AGE: u8 = 3;

/// «Блок магических листьев».
pub static LEAF_BLOCK: Block = Block::new(
    Identifier::new_static("minehouse-herbs", "magic_leaf_block"),
    BlockConfig::new()
        .destroy_time(1.0)
        .explosion_resistance(1.0)
        .sound_type(sound_types::WART_BLOCK),
    &[],
)
.with_map_color(StateByteData::new(MAP_COLOR_CYAN, &[]));

const fn no_shape(_offset: u16) -> VoxelShape {
    VoxelShape::EMPTY
}

/// A plant lets all light through.
const fn plant_light(_offset: u16) -> BlockLightProperties {
    BlockLightProperties {
        light_emission: 0,
        light_dampening: 0,
        use_shape_for_light_occlusion: false,
    }
}

/// What every plant of ours is: walked through, broken at once, popped by a piston.
const fn plant() -> BlockConfig {
    BlockConfig::new()
        .has_collision(false)
        .can_occlude(false)
        .destroy_time(0.0)
        .push_reaction(PushReaction::Popped)
}

/// The crop is a column 14 wide that gains 3 in height with every age, from 4.
const fn crop_shape(offset: u16) -> VoxelShape {
    const SHAPES: [&[BlockLocalAabb]; 4] = [
        &[BlockLocalAabb::new(
            0.0625, 0.0, 0.0625, 0.9375, 0.25, 0.9375,
        )],
        &[BlockLocalAabb::new(
            0.0625, 0.0, 0.0625, 0.9375, 0.4375, 0.9375,
        )],
        &[BlockLocalAabb::new(
            0.0625, 0.0, 0.0625, 0.9375, 0.625, 0.9375,
        )],
        &[BlockLocalAabb::new(
            0.0625, 0.0, 0.0625, 0.9375, 0.8125, 0.9375,
        )],
    ];
    VoxelShape::from_boxes(SHAPES[if offset < 4 { offset as usize } else { 3 }])
}

/// The wild plant is a column 12 wide and 13 high.
const fn wild_shape(_offset: u16) -> VoxelShape {
    const SHAPE: &[BlockLocalAabb] =
        &[BlockLocalAabb::new(0.125, 0.0, 0.125, 0.875, 0.8125, 0.875)];
    VoxelShape::from_boxes(SHAPE)
}

/// The magic plant grown from seeds on farmland: four stages like beetroots, but slower.
/// In bloom it drops 1–2 magic leaves and 1–2 seeds, younger just its seed.
pub static CROP_BLOCK: Block = Block::new(
    Identifier::new_static("minehouse-herbs", "magic_herb"),
    plant()
        .set_is_randomly_ticking(true)
        .sound_type(sound_types::CROP),
    &[&AGE],
)
.with_shapes(
    no_shape, no_shape, crop_shape, no_shape, no_shape, crop_shape,
)
.with_light_properties(plant_light)
.with_map_color(StateByteData::new(MAP_COLOR_PLANT, &[]))
.with_suffocating(StateBooleanData::FALSE)
.with_redstone_conductor(StateBooleanData::FALSE);

/// The wild magic plant with violet cones. It has no item: broken, it is gone and leaves
/// 2–3 magic seeds, so the only way to more of it is to grow them.
pub static WILD_BLOCK: Block = Block::new(
    Identifier::new_static("minehouse-herbs", "wild_magic_herb"),
    plant().sound_type(sound_types::GRASS),
    &[],
)
.with_shapes(
    no_shape, no_shape, wild_shape, no_shape, no_shape, wild_shape,
)
.with_light_properties(plant_light)
.with_map_color(StateByteData::new(MAP_COLOR_PURPLE, &[]))
.with_suffocating(StateBooleanData::FALSE)
.with_redstone_conductor(StateBooleanData::FALSE);

/// The beetroots of the same age: what the crop is without the pack, and what it breaks
/// like with it.
fn beetroots(blocks: &BlockRegistry, state: BlockStateId) -> BlockStateId {
    let age = blocks.get_property_str(state, "age").unwrap_or("0");
    blocks
        .state_id_from_block_properties(&vanilla_blocks::BEETROOTS, &[("age", age)])
        .unwrap_or_else(|| blocks.get_default_state_id(&vanilla_blocks::BEETROOTS))
}

/// Has clients see the blocks of the herb. Without a state of its own the block of leaves
/// is a warped wart block, which is the same teal, the crop is beetroots of its age and
/// the wild plant is a fern; each breaks like that block too.
pub(super) fn show_to_clients(blocks: &mut BlockRegistry) {
    let wart = blocks.get_default_state_id(&vanilla_blocks::WARPED_WART_BLOCK);
    show_full_block(blocks, &LEAF_BLOCK, wart, wart);
    show_block(blocks, &CROP_BLOCK, beetroots, beetroots);
    let fern = blocks.get_default_state_id(&vanilla_blocks::FERN);
    show_full_block(blocks, &WILD_BLOCK, fern, fern);
}

/// 1 or 2, evenly.
fn one_or_two() -> i32 {
    rand::rng().random_range(1..=2)
}

/// What a crop of this age drops: its seed, and in bloom 1–2 seeds and 1–2 leaves.
fn crop_drops(age: u8) -> Vec<ItemStack> {
    if age >= MAX_AGE {
        vec![SEEDS.stack(one_or_two()), LEAF.stack(one_or_two())]
    } else {
        vec![SEEDS.stack(1)]
    }
}

/// How the crop behaves: a vanilla crop with our drops, half as fast as beetroots.
pub(super) struct HerbCrop;

impl CropLike for HerbCrop {
    fn block(&self) -> BlockRef {
        &CROP_BLOCK
    }

    fn age_property(&self) -> &IntProperty {
        &AGE
    }

    fn max_age(&self) -> u8 {
        MAX_AGE
    }

    fn clone_item_stack(&self) -> ItemStack {
        SEEDS.stack(1)
    }

    /// A magic plant takes its time: it grows on every other random tick of a crop.
    fn should_random_tick(&self) -> bool {
        rand::rng().random_range(0..2) == 0
    }

    fn drops(
        &self,
        state: BlockStateId,
        _context: &BlockLootContext<'_>,
    ) -> Option<Vec<ItemStack>> {
        Some(crop_drops(self.get_age(state)))
    }
}

impl Bonemealable for HerbCrop {
    /// A third of a wheat's 2–5, as for beetroots: nothing or one stage.
    fn get_bonemeal_age_increase(&self, _world: &Arc<World>, rng: &mut dyn Rng) -> u8 {
        rng.random_range(2..=5) / 3
    }

    fn is_valid_bonemeal_target(
        &self,
        state: BlockStateId,
        _world: &dyn LevelReader,
        _pos: BlockPos,
    ) -> bool {
        !self.is_max_age(state)
    }

    fn perform_bonemeal(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        rng: &mut dyn Rng,
        pos: BlockPos,
    ) {
        let age = self
            .get_age(state)
            .saturating_add(self.get_bonemeal_age_increase(world, rng))
            .min(MAX_AGE);
        world.set_block(pos, self.get_state_for_age(age), UpdateFlags::UPDATE_ALL);
    }
}

/// Whether a wild plant can stand on `below`: what vanilla vegetation grows on.
fn supports_wild(below: BlockStateId) -> bool {
    below.get_block().has_tag(&BlockTag::SUPPORTS_VEGETATION)
}

/// How the wild plant behaves: vanilla vegetation that drops seeds.
pub(super) struct WildHerb;

impl BlockBehavior for WildHerb {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        let below = context.world.get_block_state(context.place_pos().below());
        supports_wild(below).then(|| WILD_BLOCK.default_state())
    }

    fn can_survive(&self, _state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> bool {
        supports_wild(world.get_block_state(pos.below()))
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        _direction: Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        if self.can_survive(state, world, pos) {
            state
        } else {
            vanilla_blocks::AIR.default_state()
        }
    }

    fn get_drops(
        &self,
        _state: BlockStateId,
        _context: &BlockLootContext<'_>,
    ) -> Option<Vec<ItemStack>> {
        Some(vec![SEEDS.stack(rand::rng().random_range(2..=3))])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crop_grows_taller_and_is_walked_through() {
        let heights = [0.25, 0.4375, 0.625, 0.8125];
        for (age, height) in heights.into_iter().enumerate() {
            let offset = age as u16;
            let outline = CROP_BLOCK.get_outline_shape(offset);
            assert_eq!(outline.boxes().len(), 1);
            assert!((outline.boxes()[0].max_y() - height).abs() < 1e-9);
            assert_eq!(CROP_BLOCK.get_collision_shape(offset), VoxelShape::EMPTY);
            assert_eq!(CROP_BLOCK.get_light_properties(offset).light_dampening, 0);
        }
        assert_eq!(CROP_BLOCK.state_count(), 4);
        assert!(CROP_BLOCK.config.is_randomly_ticking);
        assert_eq!(WILD_BLOCK.get_collision_shape(0), VoxelShape::EMPTY);
        assert!(!WILD_BLOCK.config.is_randomly_ticking);
    }

    #[test]
    fn a_young_crop_drops_its_seed_and_a_blooming_one_leaves_too() {
        for age in 0..MAX_AGE {
            let drops = crop_drops(age);
            assert_eq!(drops.len(), 1);
            assert!(SEEDS.is(&drops[0]));
            assert_eq!(drops[0].count(), 1);
        }
        for _ in 0..50 {
            let drops = crop_drops(MAX_AGE);
            assert_eq!(drops.len(), 2);
            assert!(SEEDS.is(&drops[0]) && LEAF.is(&drops[1]));
            assert!(drops.iter().all(|stack| (1..=2).contains(&stack.count())));
        }
    }
}
