use crate::behavior::block_pattern::BlockPattern;
use crate::behavior::blocks::SkullBlock;
use crate::behavior::blocks::decoration::skull::abstract_skull_block::AbstractSkullBlock;
use crate::behavior::{
    BlockBehavior, BlockEntityCreation, BlockLootContext, BlockPlaceContext, PlacementSource,
};
use crate::block_entity::SharedBlockEntity;
use crate::entity::ai::path::PathComputationType;
use crate::entity::entities::WitherBossEntity;
use crate::entity::{Entity as _, LivingEntity as _, next_entity_id};
use crate::world::LevelReader as _;
use crate::world::World;
use glam::DVec3;
use std::sync::{Arc, LazyLock, Weak};
use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::{vanilla_blocks, vanilla_entities, vanilla_items};
use steel_utils::axis::Axis;
use steel_utils::types::Difficulty;
use steel_utils::{BlockPos, BlockStateId};

fn is_summon_base(state: BlockStateId) -> bool {
    state
        .get_block()
        .has_tag(&BlockTag::WITHER_SUMMON_BASE_BLOCKS)
}

fn is_wither_skull(state: BlockStateId) -> bool {
    let block = state.get_block();
    block == &vanilla_blocks::WITHER_SKELETON_SKULL
        || block == &vanilla_blocks::WITHER_SKELETON_WALL_SKULL
}

fn is_air(state: BlockStateId) -> bool {
    state.is_air()
}

/// Vanilla `witherPatternFull`: a T of soul sand or soul soil crowned by
/// three wither skeleton skulls.
static WITHER_PATTERN_FULL: LazyLock<BlockPattern> = LazyLock::new(|| {
    BlockPattern::new(
        &[&["^^^", "###", "~#~"]],
        &[('#', is_summon_base), ('^', is_wither_skull), ('~', is_air)],
    )
});

/// Vanilla `witherPatternBase`: the T without its skulls.
static WITHER_PATTERN_BASE: LazyLock<BlockPattern> = LazyLock::new(|| {
    BlockPattern::new(
        &[&["   ", "###", "~#~"]],
        &[('#', is_summon_base), ('~', is_air)],
    )
});

/// Behavior for wither skull blocks.
#[block_behavior]
pub struct WitherSkullBlock {
    base: SkullBlock,
}

impl WitherSkullBlock {
    /// Creates a new wither skull block behavior for the given block.
    pub const fn new(block: BlockRef) -> Self {
        Self {
            base: SkullBlock::new(block),
        }
    }

    /// Vanilla `WitherSkullBlock.checkSpawn`: builds a wither when the skull
    /// at `pos` completes the pattern.
    // TODO(advancement-triggers): Trigger `CriteriaTriggers.SUMMONED_ENTITY` for
    // players within 50 blocks of the wither once Steel has advancement triggers.
    pub fn check_spawn(world: &Arc<World>, pos: BlockPos) {
        if !is_wither_skull(world.get_block_state(pos))
            || pos.y() < world.min_y()
            || world.difficulty() == Difficulty::Peaceful
        {
            return;
        }
        let Some(found) = WITHER_PATTERN_FULL.find(world, pos) else {
            return;
        };
        found.clear_blocks(world);
        let spawn_pos = found.block(1, 2, 0);
        let wither = WitherBossEntity::new(
            &vanilla_entities::WITHER,
            next_entity_id(),
            DVec3::new(
                f64::from(spawn_pos.x()) + 0.5,
                f64::from(spawn_pos.y()) + 0.55,
                f64::from(spawn_pos.z()) + 0.5,
            ),
            Arc::downgrade(world),
        );
        let yaw = if found.forwards().get_axis() == Axis::X {
            0.0
        } else {
            90.0
        };
        wither.set_rotation((yaw, 0.0));
        wither.base().set_old_rotation_to_current();
        wither.set_y_body_rot(yaw);
        wither.make_invulnerable();
        if let Err(error) = world.try_add_entity(Arc::new(wither)) {
            log::debug!("world rejected a built wither: {error}");
        }
        found.update_blocks(world);
    }

    /// Vanilla `WitherSkullBlock.canSpawnMob`: whether placing `item` at `pos`
    /// would complete a wither.
    #[must_use]
    pub fn can_spawn_mob(world: &World, pos: BlockPos, item: &ItemStack) -> bool {
        item.is(&vanilla_items::WITHER_SKELETON_SKULL)
            && pos.y() >= world.min_y() + 2
            && world.difficulty() != Difficulty::Peaceful
            && WITHER_PATTERN_BASE.find(world, pos).is_some()
    }
}

impl BlockBehavior for WitherSkullBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        self.base.get_state_for_placement(context)
    }

    fn set_placed_by(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source: &PlacementSource<'_>,
    ) {
        Self::check_spawn(world, pos);
    }

    fn handle_neighbor_changed(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        source_block: BlockRef,
        moved_by_piston: bool,
    ) {
        self.base
            .handle_neighbor_changed(state, world, pos, source_block, moved_by_piston);
    }

    fn is_pathfindable(&self, state: BlockStateId, computation_type: PathComputationType) -> bool {
        self.base.is_pathfindable(state, computation_type)
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        self.base.new_block_entity(level, pos, state)
    }

    fn get_clone_item_stack(
        &self,
        block: BlockRef,
        state: BlockStateId,
        block_entity: Option<SharedBlockEntity>,
        include_data: bool,
    ) -> Option<ItemStack> {
        self.base
            .get_clone_item_stack(block, state, block_entity, include_data)
    }

    fn get_drops(
        &self,
        state: BlockStateId,
        context: &BlockLootContext<'_>,
    ) -> Option<Vec<ItemStack>> {
        self.base.get_drops(state, context)
    }
}

impl AbstractSkullBlock for WitherSkullBlock {
    fn state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        self.base.state_for_placement(context)
    }
}
