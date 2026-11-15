//! Vanilla `WardenEntitySensor`.

use std::sync::Arc;

use steel_registry::vanilla_entities;

use super::WardenEntity;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory_types::{NEAREST_ATTACKABLE, NEAREST_LIVING_ENTITIES};
use crate::entity::ai::brain::sensors::NearestLivingEntitySensor;
use crate::entity::ai::brain::{Memories, Sensor};
use crate::world::World;

/// Remembers living entities like [`NearestLivingEntitySensor`], and the
/// closest one the warden can target as its nearest attackable, players first.
pub(super) struct WardenEntitySensor;

impl Sensor<WardenEntity> for WardenEntitySensor {
    fn register_memories(&self, memories: &mut Memories) {
        Sensor::<WardenEntity>::register_memories(&NearestLivingEntitySensor, memories);
        memories.register(&NEAREST_ATTACKABLE);
    }

    fn do_tick(&mut self, world: &Arc<World>, body: &WardenEntity) {
        NearestLivingEntitySensor.do_tick(world, body);
        let living = body.brain.memories().lock().get(&NEAREST_LIVING_ENTITIES);
        let attackable = living.and_then(|entities| {
            closest(body, &entities, true).or_else(|| closest(body, &entities, false))
        });
        body.brain
            .memories()
            .lock()
            .set(&NEAREST_ATTACKABLE, attackable);
    }
}

/// Vanilla `getClosest`: the first targetable entity that is, or is not, a
/// player.
fn closest(body: &WardenEntity, entities: &[SharedEntity], players: bool) -> Option<SharedEntity> {
    entities
        .iter()
        .find(|entity| {
            body.can_target_entity(entity.as_ref())
                && (entity.entity_type() == &vanilla_entities::PLAYER) == players
        })
        .map(SharedEntity::clone)
}
