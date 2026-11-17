//! Vanilla `CushionItem`: place a cushion on top of the clicked block.

use std::sync::Arc;

use glam::DVec3;
use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::items::item::BlockHitResult;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::{sound_events, vanilla_entities, vanilla_game_events};
use steel_utils::{Direction, Downcast as _, WorldAabb};

use crate::behavior::{
    BlockPlaceContext, InteractionResult, ItemBehavior, PlacementSource, UseOnContext,
};
use crate::entity::entities::CushionEntity;
use crate::entity::{
    Entity as _, EntitySpawnPlacement, EntitySpawnReason, EntitySpawnRequest, add_spawned_entity,
    create_spawned_entity,
};
use crate::world::World;

/// Vanilla `CushionItem.COLLISION_SHAPE_RAYCAST_EPSILON`.
const COLLISION_SHAPE_RAYCAST_EPSILON: f64 = 0.001;

/// Vanilla `CushionItem`.
#[item_behavior]
pub struct CushionItem;

impl CushionItem {
    /// Vanilla `CushionItem.recalculateContextForSpecialCollisionShapes`: blocks
    /// tagged `#cushion_uses_collision_shape` (cauldrons, hoppers) are clicked
    /// against their collision shape, so the cushion sits on the rim.
    fn recalculate_hit_for_special_collision_shapes(context: &UseOnContext) -> BlockHitResult {
        let hit = context.hit_result.clone();
        let clicked_state = context.world.get_block_state(hit.block_pos);
        if !clicked_state
            .get_block()
            .has_tag(&BlockTag::CUSHION_USES_COLLISION_SHAPE)
        {
            return hit;
        }
        let player_position = context.player.position();
        let ray_from = DVec3::new(
            player_position.x,
            context.player.get_eye_y(),
            player_position.z,
        );
        let ray = hit.location - ray_from;
        let ray_to = hit.location + ray.normalize() * COLLISION_SHAPE_RAYCAST_EPSILON;
        World::clip_shape(
            hit.block_pos,
            ray_from,
            ray_to,
            clicked_state.get_collision_shape_at(hit.block_pos),
        )
        .map_or(hit, |collision_hit| BlockHitResult {
            location: collision_hit.location,
            direction: collision_hit.direction,
            block_pos: collision_hit.block_pos,
            miss: collision_hit.miss,
            inside: collision_hit.inside,
            world_border_hit: collision_hit.world_border_hit,
        })
    }
}

impl ItemBehavior for CushionItem {
    /// Vanilla `CushionItem.useOn`.
    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        let hit = Self::recalculate_hit_for_special_collision_shapes(context);
        if hit.direction != Direction::Up {
            return InteractionResult::Fail;
        }
        let place_context = BlockPlaceContext::new(
            context.world,
            PlacementSource::player_hand(context.player, &context.inv),
            &hit,
        );
        let block_pos = place_context.place_pos();
        let entity_pos = DVec3::new(
            f64::from(block_pos.x()) + 0.5,
            hit.location.y,
            f64::from(block_pos.z()) + 0.5,
        );
        let dimensions = vanilla_entities::CUSHION.dimensions;
        let spawn_aabb = WorldAabb::entity_box(
            entity_pos.x,
            entity_pos.y,
            entity_pos.z,
            f64::from(dimensions.half_width()),
            f64::from(dimensions.height),
        );
        let world = context.world;
        if !CushionEntity::can_be_placed_at(world, spawn_aabb)
            || world.has_entity_in_aabb_matching(&spawn_aabb, |entity| {
                entity.downcast_ref::<CushionEntity>().is_some()
            })
        {
            return InteractionResult::Fail;
        }

        let player = context.player;
        let stack = context.inv.with_item(|item| item.clone());
        let request = EntitySpawnRequest {
            entity_type: &vanilla_entities::CUSHION,
            placement: EntitySpawnPlacement::Block {
                pos: block_pos,
                try_move_down: true,
                moved_up: true,
            },
            reason: EntitySpawnReason::SpawnItemUse,
            finalize_spawn: true,
            play_ambient_sound: false,
            item_stack: Some(&stack),
            user_is_operator: player.is_operator(),
        };
        let Ok(cushion) = create_spawned_entity(world, request) else {
            return InteractionResult::Fail;
        };
        cushion.snap_to(
            entity_pos,
            Direction::from_yaw(place_context.rotation()).to_yaw(),
            0.0,
        );
        if add_spawned_entity(world, Arc::clone(&cushion)).is_err() {
            return InteractionResult::Fail;
        }
        if let Some(cushion) = cushion.downcast_ref::<CushionEntity>() {
            cushion.destroy_if_in_fire(world);
        }
        world.play_sound_at(
            &sound_events::ENTITY_CUSHION_PLACE,
            SoundSource::Blocks,
            cushion.position(),
            0.75,
            0.8,
            None,
        );
        cushion.game_event_with_source_entity(
            &vanilla_game_events::ENTITY_PLACE,
            Some(player.as_ref()),
        );
        let has_infinite_materials = player.has_infinite_materials();
        context
            .inv
            .with_item(|item| item.consume(1, has_infinite_materials));
        InteractionResult::Success
    }
}
