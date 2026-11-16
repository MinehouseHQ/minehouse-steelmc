//! Vanilla `EndCrystalItem`: place an End crystal on obsidian or bedrock.

use std::sync::Arc;

use glam::DVec3;
use steel_macros::item_behavior;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::{vanilla_blocks, vanilla_entities, vanilla_game_events};
use steel_utils::WorldAabb;

use crate::behavior::{InteractionResult, ItemBehavior, UseOnContext};
use crate::entity::entities::EndCrystalEntity;
use crate::entity::{SharedEntity, next_entity_id};
use crate::player::Player;
use crate::world::game_event::GameEventContext;

/// Vanilla `EndCrystalItem`.
#[item_behavior]
pub struct EndCrystalItem;

impl ItemBehavior for EndCrystalItem {
    /// Vanilla `EndCrystalItem.useOn`.
    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        let world = context.world;
        let pos = context.hit_result.block_pos;
        let block = world.get_block_state(pos).get_block();
        if block != &vanilla_blocks::OBSIDIAN && block != &vanilla_blocks::BEDROCK {
            return InteractionResult::Fail;
        }
        let above = pos.above();
        if !world.get_block_state(above).is_air() {
            return InteractionResult::Fail;
        }
        let corner = DVec3::new(
            f64::from(above.x()),
            f64::from(above.y()),
            f64::from(above.z()),
        );
        let space = WorldAabb::new(
            corner.x,
            corner.y,
            corner.z,
            corner.x + 1.0,
            corner.y + 2.0,
            corner.z + 1.0,
        );
        if !world.get_entities_in_aabb(&space).is_empty() {
            return InteractionResult::Fail;
        }

        let crystal = EndCrystalEntity::new(
            &vanilla_entities::END_CRYSTAL,
            next_entity_id(),
            DVec3::new(corner.x + 0.5, corner.y, corner.z + 0.5),
            Arc::downgrade(world),
        );
        crystal.set_show_bottom(false);
        let crystal: SharedEntity = Arc::new(crystal);
        if world.try_add_entity(crystal).is_err() {
            return InteractionResult::Fail;
        }
        let player: &Player = context.player;
        world.game_event(
            &vanilla_game_events::ENTITY_PLACE,
            above,
            &GameEventContext::new(Some(player), None),
        );
        if let Some(fight) = world.dragon_fight() {
            fight.try_respawn(world);
        }

        context.inv.with_item(|item| item.shrink(1));
        InteractionResult::Success
    }
}
