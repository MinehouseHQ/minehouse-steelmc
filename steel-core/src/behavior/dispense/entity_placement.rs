//! Dispenser behaviors that spawn entities or equip the entities in front
//! (Vanilla `SpawnEggItemBehavior`, the armor stand and chest entries of
//! `DispenseItemBehavior.bootStrap`).

use std::sync::Arc;

use steel_registry::data_components::components::EntityData;
use steel_registry::data_components::vanilla_components::ENTITY_DATA;
use steel_registry::item_stack::ItemStack;
use steel_registry::{vanilla_entities, vanilla_game_events};
use steel_utils::{Direction, WorldAabb};

use super::BlockSource;
use crate::behavior::ITEM_BEHAVIORS;
use crate::entity::{
    EntitySpawnPlacement, EntitySpawnReason, EntitySpawnRequest, add_spawned_entity,
    create_spawned_entity, spawn_entity,
};
use crate::inventory::slot_range::CURSOR_OR_HORSE_CHEST_SLOT;
use crate::world::game_event::GameEventContext;

/// Vanilla `Direction.toYRot`: `(data2d & 3) * 90`, so up and down face 270.
pub(super) const fn direction_y_rot(direction: Direction) -> f32 {
    match direction {
        Direction::Up | Direction::Down => 270.0,
        horizontal => horizontal.to_yaw(),
    }
}

/// The unit box of the block in front of the dispenser (Vanilla `new AABB(pos)`).
fn target_box(source: &BlockSource<'_>) -> WorldAabb {
    let target = source.target();
    WorldAabb::new(
        f64::from(target.x()),
        f64::from(target.y()),
        f64::from(target.z()),
        f64::from(target.x() + 1),
        f64::from(target.y() + 1),
        f64::from(target.z() + 1),
    )
}

/// Vanilla `SpawnEggItemBehavior.execute`: spawns the egg's entity in front.
pub(super) fn spawn_egg(source: &BlockSource<'_>, mut stack: ItemStack) -> ItemStack {
    let direction = source.facing();
    let Some(entity_type) = stack.get(ENTITY_DATA).map(EntityData::entity_type) else {
        return stack;
    };
    let request = EntitySpawnRequest {
        entity_type,
        placement: EntitySpawnPlacement::Block {
            pos: source.target(),
            try_move_down: direction != Direction::Up,
            moved_up: false,
        },
        reason: EntitySpawnReason::Dispenser,
        finalize_spawn: true,
        play_ambient_sound: true,
        item_stack: Some(&stack),
        user_is_operator: false,
    };
    if spawn_entity(source.world, request).is_err() {
        return stack;
    }

    stack.shrink(1);
    source.world.game_event(
        &vanilla_game_events::ENTITY_PLACE,
        source.pos,
        &GameEventContext::new(None, None),
    );
    stack
}

/// Vanilla `Items.ARMOR_STAND` dispenser behavior: places an armor stand in
/// front, facing away from the dispenser.
pub(super) fn armor_stand(source: &BlockSource<'_>, mut stack: ItemStack) -> ItemStack {
    let request = EntitySpawnRequest {
        entity_type: &vanilla_entities::ARMOR_STAND,
        placement: EntitySpawnPlacement::Block {
            pos: source.target(),
            try_move_down: false,
            moved_up: false,
        },
        reason: EntitySpawnReason::Dispenser,
        finalize_spawn: true,
        play_ambient_sound: false,
        item_stack: Some(&stack),
        user_is_operator: false,
    };
    let Ok(stand) = create_spawned_entity(source.world, request) else {
        return stack;
    };
    stand.set_rotation((direction_y_rot(source.facing()), stand.rotation().1));
    if add_spawned_entity(source.world, Arc::clone(&stand)).is_err() {
        return stack;
    }
    stack.shrink(1);
    stack
}

/// Vanilla `Items.CHEST` dispenser behavior: puts the chest on a tamed donkey,
/// mule or llama in front. Returns whether a chest was fitted.
pub(super) fn chest_on_chested_horse(source: &BlockSource<'_>, stack: &mut ItemStack) -> bool {
    let candidates = source
        .world
        .get_entities_in_aabb_matching(&target_box(source), |entity| {
            entity.is_alive()
                && entity
                    .as_abstract_chested_horse()
                    .is_some_and(|horse| !horse.has_chest())
        });
    for entity in candidates {
        let Some(horse) = entity.as_abstract_chested_horse() else {
            continue;
        };
        if !horse.is_tamed() {
            continue;
        }
        if let Some(slot) = entity.get_slot(CURSOR_OR_HORSE_CHEST_SLOT)
            && slot.set(stack.clone())
        {
            stack.shrink(1);
            return true;
        }
    }
    false
}

/// Whether `item` is one of the spawn eggs `DispenserBlock.getDefaultDispenseMethod`
/// hands to `SpawnEggItemBehavior`.
pub(super) fn is_spawn_egg(stack: &ItemStack) -> bool {
    stack.has(ENTITY_DATA)
        && ITEM_BEHAVIORS
            .get_behavior(stack.item)
            .as_spawn_egg()
            .is_some()
}

#[cfg(test)]
mod tests;
