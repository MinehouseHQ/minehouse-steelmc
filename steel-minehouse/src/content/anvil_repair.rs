//! Mending an anvil (the Fabric `AnvilRepair`). A click on a chipped or damaged anvil with a
//! block of iron in hand undoes one step of damage and takes the block: damaged becomes
//! chipped, chipped becomes whole, facing the same way. A whole anvil opens as always, and
//! so does a worn one for any other item.

use std::sync::Arc;

use steel_core::{behavior::InteractionResult, player::Player, world::World};
use steel_protocol::packets::game::SoundSource;
use steel_registry::{
    blocks::{block_state_ext::BlockStateExt as _, properties::BlockStateProperties},
    items::item::BlockHitResult,
    sound_events as sounds, vanilla_blocks, vanilla_items,
};
use steel_utils::{BlockStateId, types::InteractionHand, types::UpdateFlags};

/// One step less damaged, facing the same way. `None` if `state` is not a worn anvil.
#[must_use]
pub fn repaired(state: BlockStateId) -> Option<BlockStateId> {
    let block = state.get_block();
    let better = if block == &vanilla_blocks::DAMAGED_ANVIL {
        &vanilla_blocks::CHIPPED_ANVIL
    } else if block == &vanilla_blocks::CHIPPED_ANVIL {
        &vanilla_blocks::ANVIL
    } else {
        return None;
    };
    Some(
        better
            .default_state()
            .copy_value(&BlockStateProperties::FACING, &state),
    )
}

/// `player` clicked the block at `hit` with `hand`: mends the anvil there if the hand holds
/// a block of iron. `None` leaves the click to vanilla.
pub(super) fn clicked(
    player: &Arc<Player>,
    world: &Arc<World>,
    hand: InteractionHand,
    hit: &BlockHitResult,
) -> Option<InteractionResult> {
    if !player.abilities.lock().may_build {
        return None;
    }
    let pos = hit.block_pos;
    let better = repaired(world.get_block_state(pos))?;
    {
        let mut inventory = player.inventory.lock();
        let held = inventory.get_item_in_hand_mut(hand);
        if !held.is(&vanilla_items::IRON_BLOCK) {
            return None;
        }
        if !player.has_infinite_materials() {
            held.shrink(1);
        }
    }
    world.set_block(pos, better, UpdateFlags::UPDATE_ALL);
    world.play_sound(
        &sounds::BLOCK_ANVIL_USE,
        SoundSource::Blocks,
        pos,
        1.0,
        1.0,
        None,
    );
    Some(InteractionResult::Success)
}

#[cfg(test)]
mod tests {
    use steel_registry::{blocks::Block, init_vanilla_registry};
    use steel_utils::Direction;

    use super::*;

    fn anvil(block: &'static Block, facing: Direction) -> BlockStateId {
        init_vanilla_registry();
        block
            .default_state()
            .set_value(&BlockStateProperties::HORIZONTAL_FACING, facing)
    }

    #[test]
    fn each_click_undoes_one_step() {
        let damaged = anvil(&vanilla_blocks::DAMAGED_ANVIL, Direction::North);
        let chipped = repaired(damaged);
        assert_eq!(
            chipped,
            Some(anvil(&vanilla_blocks::CHIPPED_ANVIL, Direction::North))
        );
        let whole = chipped.and_then(repaired);
        assert_eq!(whole, Some(anvil(&vanilla_blocks::ANVIL, Direction::North)));
    }

    #[test]
    fn the_anvil_keeps_its_facing() {
        for facing in [Direction::East, Direction::South, Direction::West] {
            assert_eq!(
                repaired(anvil(&vanilla_blocks::DAMAGED_ANVIL, facing)),
                Some(anvil(&vanilla_blocks::CHIPPED_ANVIL, facing))
            );
        }
    }

    #[test]
    fn a_whole_anvil_and_other_blocks_are_left_alone() {
        init_vanilla_registry();
        assert_eq!(repaired(vanilla_blocks::ANVIL.default_state()), None);
        assert_eq!(repaired(vanilla_blocks::STONE.default_state()), None);
    }
}
