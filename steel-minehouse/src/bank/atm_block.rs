//! «Банкомат», the block (the Fabric `AtmBlock`).
//!
//! The client sees a barrier: a full hitbox and collision, nothing drawn, can't be mined in
//! survival. The model is an item display at the center of the block with the item model
//! `minehouse-bank:atm` of the server pack, turned to face the player who placed the block.
//! Right click opens the ATM window.
//!
//! On the Fabric server the display was a virtual entity of Polymer. Here it is a real one,
//! saved with the chunk and tagged [`TAG`]: it is made when the block is placed, removed with
//! the block, and made again by a right click if it is missing (an ATM of a world that came
//! from the Fabric server has none).

use std::sync::Arc;

use glam::DVec3;
use steel_core::{
    behavior::{
        BlockBehavior, BlockLootContext, BlockPlaceContext, InteractionResult, InventoryAccess,
    },
    entity::{
        Entity as _, RemovalReason, SharedEntity, entities::ItemDisplayEntity, next_entity_id,
    },
    player::Player,
    world::World,
};
use steel_registry::{
    blocks::{
        Block, BlockRegistry, behavior::BlockConfig, block_state_ext::BlockStateExt as _,
        properties::BlockStateProperties,
    },
    item_stack::ItemStack,
    items::item::BlockHitResult,
    sound_types, vanilla_blocks, vanilla_entities, vanilla_items,
};
use steel_utils::{
    BlockPos, BlockStateId, Direction, Downcast as _, Identifier, WorldAabb, types::InteractionHand,
};

use super::{atm, gui};
use crate::content::{ATM, blocks::show_as};

/// The tag of the item display that draws an ATM.
pub const TAG: &str = "minehouse_atm";

/// «Банкомат». Like a barrier for survival players: it can't be mined.
pub static ATM_BLOCK: Block = Block::new(
    Identifier::new_static("minehouse-bank", "atm"),
    BlockConfig::new()
        .destroy_time(-1.0)
        .explosion_resistance(3_600_000.0)
        .can_occlude(false)
        .sound_type(sound_types::STONE),
    &[&BlockStateProperties::HORIZONTAL_FACING],
);

/// Has clients see an ATM as a barrier that breaks like smooth stone.
pub(crate) fn show_to_clients(blocks: &mut BlockRegistry) {
    show_as(
        blocks,
        &ATM_BLOCK,
        |blocks, _| blocks.get_default_state_id(&vanilla_blocks::BARRIER),
        |blocks, _| blocks.get_default_state_id(&vanilla_blocks::SMOOTH_STONE),
    );
}

/// The yaw that turns the front of the model (north) to `facing`. An item display shows a
/// model turned 180° compared to a placed block, hence the extra half turn.
#[must_use]
pub const fn model_yaw(facing: Direction) -> f32 {
    180.0
        + match facing {
            Direction::East => 90.0,
            Direction::South => 180.0,
            Direction::West => 270.0,
            _ => 0.0,
        }
}

fn center(pos: BlockPos) -> DVec3 {
    DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + 0.5,
        f64::from(pos.z()) + 0.5,
    )
}

/// The item displays of the ATM at `pos`.
fn displays(world: &World, pos: BlockPos) -> Vec<SharedEntity> {
    let center = center(pos);
    world
        .get_entities_in_aabb(&WorldAabb::from_min_max(center, center).inflate(0.5))
        .into_iter()
        .filter(|entity| {
            !entity.is_removed()
                && entity.downcast_ref::<ItemDisplayEntity>().is_some()
                && entity.tags().iter().any(|tag| tag == TAG)
                && entity.position().distance_squared(center) < 0.01
        })
        .collect()
}

/// Makes the item display of the ATM at `pos`, unless it is there.
fn show_model(world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
    if !displays(world, pos).is_empty() {
        return;
    }
    let display = ItemDisplayEntity::new(
        &vanilla_entities::ITEM_DISPLAY,
        next_entity_id(),
        center(pos),
        Arc::downgrade(world),
    );
    let mut model = ItemStack::new(&vanilla_items::PAPER);
    gui::model(&mut model, "minehouse-bank:atm");
    display.set_item_stack(model);
    let facing = state.get_value(&BlockStateProperties::HORIZONTAL_FACING);
    display.base().set_rotation((model_yaw(facing), 0.0));
    display.base().add_tag(TAG.to_owned());
    let entity: SharedEntity = Arc::new(display);
    if let Err(error) = world.try_add_entity(entity) {
        log::warn!("Bank: could not show the ATM at {pos:?}: {error:?}");
    }
}

/// The ATM block.
pub(crate) struct Atm;

impl Atm {
    fn open(state: BlockStateId, world: &Arc<World>, pos: BlockPos, player: &Player) {
        show_model(world, pos, state);
        atm::open(player, pos);
    }
}

impl BlockBehavior for Atm {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        // The front looks at the player.
        Some(ATM_BLOCK.default_state().set_value(
            &BlockStateProperties::HORIZONTAL_FACING,
            context.horizontal_direction().opposite(),
        ))
    }

    fn get_drops(
        &self,
        _state: BlockStateId,
        _context: &BlockLootContext<'_>,
    ) -> Option<Vec<ItemStack>> {
        Some(vec![ATM.stack(1)])
    }

    fn on_place(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _old_state: BlockStateId,
        _moved_by_piston: bool,
    ) {
        show_model(world, pos, state);
    }

    fn affect_neighbors_after_removal(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _moved_by_piston: bool,
    ) {
        for display in displays(world, pos) {
            display.set_removed(RemovalReason::Discarded);
        }
    }

    fn use_item_on(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        hand: InteractionHand,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        if hand != InteractionHand::MainHand {
            return InteractionResult::Pass;
        }
        Self::open(state, world, pos, player);
        InteractionResult::Success
    }

    fn use_without_item(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        Self::open(state, world, pos, player);
        InteractionResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_turns_its_front_to_where_the_block_faces() {
        assert!((model_yaw(Direction::North) - 180.0).abs() < f32::EPSILON);
        assert!((model_yaw(Direction::East) - 270.0).abs() < f32::EPSILON);
        assert!((model_yaw(Direction::South) - 360.0).abs() < f32::EPSILON);
        assert!((model_yaw(Direction::West) - 450.0).abs() < f32::EPSILON);
    }
}
