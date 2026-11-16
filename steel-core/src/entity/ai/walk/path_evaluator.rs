use std::sync::OnceLock;

use steel_registry::blocks::BlockRef;

use super::*;

/// Block tags that walking path types check, cached per block.
///
/// Every pathfinding node looks up several block tags, and each lookup hashes two
/// identifiers. Tags are fixed once the registry is frozen, so the answers are computed
/// once per block on first use.
pub(super) mod path_tags {
    use super::{BlockRef, BlockTag, OnceLock};

    pub(in crate::entity::ai::walk) const TRAPDOORS: u16 = 1 << 0;
    pub(in crate::entity::ai::walk) const SPELEOTHEMS: u16 = 1 << 1;
    pub(in crate::entity::ai::walk) const DOORS: u16 = 1 << 2;
    pub(in crate::entity::ai::walk) const MOB_INTERACTABLE_DOORS: u16 = 1 << 3;
    pub(in crate::entity::ai::walk) const RAILS: u16 = 1 << 4;
    pub(in crate::entity::ai::walk) const LEAVES: u16 = 1 << 5;
    pub(in crate::entity::ai::walk) const FENCES: u16 = 1 << 6;
    pub(in crate::entity::ai::walk) const WALLS: u16 = 1 << 7;
    pub(in crate::entity::ai::walk) const FENCE_GATES: u16 = 1 << 8;
    pub(in crate::entity::ai::walk) const FIRE: u16 = 1 << 9;
    pub(in crate::entity::ai::walk) const CAMPFIRES: u16 = 1 << 10;

    pub(in crate::entity::ai::walk) fn compute(block: BlockRef) -> u16 {
        [
            (TRAPDOORS, &BlockTag::TRAPDOORS),
            (SPELEOTHEMS, &BlockTag::SPELEOTHEMS),
            (DOORS, &BlockTag::DOORS),
            (MOB_INTERACTABLE_DOORS, &BlockTag::MOB_INTERACTABLE_DOORS),
            (RAILS, &BlockTag::RAILS),
            (LEAVES, &BlockTag::LEAVES),
            (FENCES, &BlockTag::FENCES),
            (WALLS, &BlockTag::WALLS),
            (FENCE_GATES, &BlockTag::FENCE_GATES),
            (FIRE, &BlockTag::FIRE),
            (CAMPFIRES, &BlockTag::CAMPFIRES),
        ]
        .into_iter()
        .filter(|(_, tag)| block.has_tag(tag))
        .fold(0, |flags, (flag, _)| flags | flag)
    }

    /// The path tag flags of `block`.
    pub(in crate::entity::ai::walk) fn of(block: BlockRef) -> u16 {
        static FLAGS: OnceLock<Box<[u16]>> = OnceLock::new();
        let flags = FLAGS.get_or_init(|| {
            steel_registry::REGISTRY
                .blocks
                .iter()
                .map(|(_, block)| compute(block))
                .collect()
        });
        block
            .id
            .get()
            .and_then(|&id| flags.get(id).copied())
            .unwrap_or_else(|| compute(block))
    }
}

pub(super) const fn does_block_have_partial_collision(path_type: PathType) -> bool {
    matches!(
        path_type,
        PathType::Fence | PathType::DoorWoodClosed | PathType::DoorIronClosed
    )
}

pub struct WalkPathEvaluator;

impl WalkPathEvaluator {
    #[must_use]
    pub fn path_type(context: &mut PathfindingContext<'_>, x: i32, y: i32, z: i32) -> PathType {
        Self::path_type_static(context, BlockPos::new(x, y, z))
    }

    #[must_use]
    pub fn path_type_static(context: &mut PathfindingContext<'_>, pos: BlockPos) -> PathType {
        let x = pos.x();
        let y = pos.y();
        let z = pos.z();
        let block_path_type = context.get_path_type_from_state(x, y, z);
        if block_path_type != PathType::Open || y < context.level().min_y() + 1 {
            return block_path_type;
        }

        match context.get_path_type_from_state(x, y - 1, z) {
            PathType::Open | PathType::Water | PathType::Lava | PathType::Walkable => {
                PathType::Open
            }
            PathType::Fire => PathType::Fire,
            PathType::Damaging => PathType::Damaging,
            PathType::StickyHoney => PathType::StickyHoney,
            PathType::PowderSnow => PathType::OnTopOfPowderSnow,
            PathType::DamageCautious => PathType::DamageCautious,
            PathType::Trapdoor => PathType::OnTopOfTrapdoor,
            _ => Self::check_neighbour_blocks(context, x, y, z, PathType::Walkable),
        }
    }

    #[must_use]
    pub fn check_neighbour_blocks(
        context: &mut PathfindingContext<'_>,
        x: i32,
        y: i32,
        z: i32,
        block_path_type: PathType,
    ) -> PathType {
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if dx == 0 && dz == 0 {
                        continue;
                    }

                    match context.get_path_type_from_state(x + dx, y + dy, z + dz) {
                        PathType::Damaging => return PathType::DamagingInNeighbor,
                        PathType::Fire | PathType::Lava => return PathType::FireInNeighbor,
                        PathType::Water => return PathType::WaterBorder,
                        PathType::DamageCautious => return PathType::DamageCautious,
                        _ => {}
                    }
                }
            }
        }

        block_path_type
    }

    #[must_use]
    pub fn path_type_from_state(level: &(impl LevelReader + ?Sized), pos: BlockPos) -> PathType {
        let block_state = level.get_block_state(pos);
        let block = block_state.get_block();
        if block_state.is_air() {
            return PathType::Open;
        }

        let tags = path_tags::of(block);
        if tags & path_tags::TRAPDOORS != 0
            || block == &vanilla_blocks::LILY_PAD
            || block == &vanilla_blocks::BIG_DRIPLEAF
        {
            return PathType::Trapdoor;
        }

        if block == &vanilla_blocks::POWDER_SNOW {
            return PathType::PowderSnow;
        }

        if block == &vanilla_blocks::CACTUS || block == &vanilla_blocks::SWEET_BERRY_BUSH {
            return PathType::Damaging;
        }

        if block == &vanilla_blocks::HONEY_BLOCK {
            return PathType::StickyHoney;
        }

        if block == &vanilla_blocks::COCOA {
            return PathType::Cocoa;
        }

        if block == &vanilla_blocks::WITHER_ROSE || tags & path_tags::SPELEOTHEMS != 0 {
            return PathType::DamageCautious;
        }

        let fluid_state = block_state.get_fluid_state();
        if fluid_state.is_lava() {
            return PathType::Lava;
        }

        if Self::is_burning_block_with_tags(block_state, tags) {
            return PathType::Fire;
        }

        if tags & path_tags::DOORS != 0 {
            return if block_state
                .try_get_value(&BlockStateProperties::OPEN)
                .unwrap_or(false)
            {
                PathType::DoorOpen
            } else if tags & path_tags::MOB_INTERACTABLE_DOORS != 0 {
                PathType::DoorWoodClosed
            } else {
                PathType::DoorIronClosed
            };
        }

        if tags & path_tags::RAILS != 0 {
            return PathType::Rail;
        }

        if tags & path_tags::LEAVES != 0 {
            return PathType::Leaves;
        }

        if tags & (path_tags::FENCES | path_tags::WALLS) != 0
            || tags & path_tags::FENCE_GATES != 0
                && !block_state
                    .try_get_value(&BlockStateProperties::OPEN)
                    .unwrap_or(false)
        {
            return PathType::Fence;
        }

        if !block_state.is_pathfindable(PathComputationType::Land) {
            return PathType::Blocked;
        }

        if fluid_state.is_water() {
            PathType::Water
        } else {
            PathType::Open
        }
    }

    #[must_use]
    pub fn is_burning_block(block_state: steel_utils::BlockStateId) -> bool {
        Self::is_burning_block_with_tags(block_state, path_tags::of(block_state.get_block()))
    }

    fn is_burning_block_with_tags(block_state: steel_utils::BlockStateId, tags: u16) -> bool {
        let block = block_state.get_block();
        tags & path_tags::FIRE != 0
            || block == &vanilla_blocks::LAVA
            || block == &vanilla_blocks::MAGMA_BLOCK
            || block == &vanilla_blocks::LAVA_CAULDRON
            || tags & path_tags::CAMPFIRES != 0
                && block_state
                    .try_get_value(&BlockStateProperties::LIT)
                    .unwrap_or(false)
    }
}
