//! Vanilla `PiglinSpecificSensor` and `PiglinBruteSpecificSensor`.

use std::sync::Arc;

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::{vanilla_blocks, vanilla_entities};
use steel_utils::{BlockPos, BlockStateId, Downcast as _};

use super::anger::{
    find_nearby_adult_piglins, is_player_holding_loved_item, is_wearing_safe_armor, is_zombified,
};
use crate::behavior::blocks::CampfireBlock;
use crate::entity::ai::brain::memory_types::{
    NEARBY_ADULT_PIGLINS, NEAREST_LIVING_ENTITIES, NEAREST_PLAYER_HOLDING_WANTED_ITEM,
    NEAREST_REPELLENT, NEAREST_TARGETABLE_PLAYER_NOT_WEARING_GOLD, NEAREST_VISIBLE_ADULT_PIGLINS,
    NEAREST_VISIBLE_BABY_HOGLIN, NEAREST_VISIBLE_HUNTABLE_HOGLIN, NEAREST_VISIBLE_LIVING_ENTITIES,
    NEAREST_VISIBLE_NEMESIS, NEAREST_VISIBLE_ZOMBIFIED, VISIBLE_ADULT_HOGLIN_COUNT,
    VISIBLE_ADULT_PIGLIN_COUNT,
};
use crate::entity::ai::brain::{Memories, Sensor};
use crate::entity::entities::HoglinEntity;
use crate::entity::{Entity, LivingEntity, Mob, SharedEntity};
use crate::world::World;

/// Vanilla `PiglinAi.REPELLENT_DETECTION_RANGE_HORIZONTAL` and `_VERTICAL`.
const REPELLENT_DETECTION_RANGE_HORIZONTAL: i32 = 8;
const REPELLENT_DETECTION_RANGE_VERTICAL: i32 = 4;

/// Wither skeletons and withers: what piglins fight on sight.
fn is_nemesis(entity: &dyn Entity) -> bool {
    let entity_type = entity.entity_type();
    entity_type == &vanilla_entities::WITHER_SKELETON || entity_type == &vanilla_entities::WITHER
}

/// Vanilla `PiglinSpecificSensor.isValidRepellent`: soul fire blocks, with a
/// soul campfire only while lit.
fn is_valid_repellent(state: BlockStateId) -> bool {
    let is_repellent = state.get_block().has_tag(&BlockTag::PIGLIN_REPELLENTS);
    if is_repellent && state.get_block() == &vanilla_blocks::SOUL_CAMPFIRE {
        CampfireBlock::is_lit_campfire(state)
    } else {
        is_repellent
    }
}

fn find_nearest_repellent(world: &World, body: &dyn LivingEntity) -> Option<BlockPos> {
    body.block_position()
        .within_manhattan(
            REPELLENT_DETECTION_RANGE_HORIZONTAL,
            REPELLENT_DETECTION_RANGE_VERTICAL,
            REPELLENT_DETECTION_RANGE_HORIZONTAL,
        )
        .find(|pos| is_valid_repellent(world.get_block_state(*pos)))
}

/// What the piglin sensor saw this scan.
#[derive(Default)]
struct PiglinSightings {
    nemesis: Option<SharedEntity>,
    huntable_hoglin: Option<SharedEntity>,
    baby_hoglin: Option<SharedEntity>,
    baby_piglin_seen: bool,
    zombified: Option<SharedEntity>,
    player_not_wearing_gold: Option<SharedEntity>,
    player_holding_wanted_item: Option<SharedEntity>,
    visible_adult_hoglin_count: i32,
    visible_adult_piglins: Vec<SharedEntity>,
}

impl PiglinSightings {
    fn see_hoglin(&mut self, entity: &SharedEntity, hoglin: &HoglinEntity) {
        if hoglin.is_baby() && self.baby_hoglin.is_none() {
            self.baby_hoglin = Some(Arc::clone(entity));
        } else if hoglin.is_adult() {
            self.visible_adult_hoglin_count += 1;
            if self.huntable_hoglin.is_none() && hoglin.can_be_hunted() {
                self.huntable_hoglin = Some(Arc::clone(entity));
            }
        }
    }

    fn see_player(&mut self, body: &dyn Mob, entity: &SharedEntity, player: &dyn LivingEntity) {
        if self.player_not_wearing_gold.is_none()
            && !is_wearing_safe_armor(player)
            && Mob::can_attack(body, player)
        {
            self.player_not_wearing_gold = Some(Arc::clone(entity));
        }
        if self.player_holding_wanted_item.is_none()
            && !entity.is_spectator()
            && is_player_holding_loved_item(player)
        {
            self.player_holding_wanted_item = Some(Arc::clone(entity));
        }
    }

    /// One visible entity, in the order vanilla's `instanceof` chain checks.
    fn see(&mut self, body: &dyn Mob, entity: &SharedEntity) {
        let Some(living) = entity.as_living_entity() else {
            return;
        };
        if let Some(hoglin) = entity.downcast_ref::<HoglinEntity>() {
            self.see_hoglin(entity, hoglin);
        } else if entity.downcast_ref::<super::PiglinBruteEntity>().is_some() {
            self.visible_adult_piglins.push(Arc::clone(entity));
        } else if entity.downcast_ref::<super::PiglinEntity>().is_some() {
            if living.is_baby() && !self.baby_piglin_seen {
                self.baby_piglin_seen = true;
            } else if !living.is_baby() {
                self.visible_adult_piglins.push(Arc::clone(entity));
            }
        } else if entity.as_player().is_some() {
            self.see_player(body, entity, living);
        } else if self.nemesis.is_some() || !is_nemesis(entity.as_ref()) {
            if self.zombified.is_none() && is_zombified(entity.as_ref()) {
                self.zombified = Some(Arc::clone(entity));
            }
        } else {
            self.nemesis = Some(Arc::clone(entity));
        }
    }
}

/// Finds the nearest soul fire and sorts what the piglin sees into
/// nemeses, hoglins to hunt or ride, zombified mobs, players without gold
/// armor or holding gold, and fellow piglins.
pub(super) struct PiglinSpecificSensor;

impl<E: Mob> Sensor<E> for PiglinSpecificSensor {
    fn register_memories(&self, memories: &mut Memories) {
        memories.register(&NEAREST_VISIBLE_LIVING_ENTITIES);
        memories.register(&NEAREST_LIVING_ENTITIES);
        memories.register(&NEAREST_VISIBLE_NEMESIS);
        memories.register(&NEAREST_TARGETABLE_PLAYER_NOT_WEARING_GOLD);
        memories.register(&NEAREST_PLAYER_HOLDING_WANTED_ITEM);
        memories.register(&NEAREST_VISIBLE_HUNTABLE_HOGLIN);
        memories.register(&NEAREST_VISIBLE_BABY_HOGLIN);
        memories.register(&NEAREST_VISIBLE_ADULT_PIGLINS);
        memories.register(&NEARBY_ADULT_PIGLINS);
        memories.register(&VISIBLE_ADULT_PIGLIN_COUNT);
        memories.register(&VISIBLE_ADULT_HOGLIN_COUNT);
        memories.register(&NEAREST_REPELLENT);
    }

    fn do_tick(&mut self, world: &Arc<World>, body: &E) {
        let Some(memories) = body.brain_memories() else {
            return;
        };
        let repellent = find_nearest_repellent(world, body);
        memories.lock().set(&NEAREST_REPELLENT, repellent);

        let visible = memories.lock().get(&NEAREST_VISIBLE_LIVING_ENTITIES);
        let mut sightings = PiglinSightings::default();
        if let Some(visible) = visible {
            for entity in visible.find_all(|_| true) {
                sightings.see(body, entity);
            }
        }
        let adult_piglins = find_nearby_adult_piglins(body);
        let visible_adult_piglin_count =
            i32::try_from(sightings.visible_adult_piglins.len()).unwrap_or(i32::MAX);

        let mut memories = memories.lock();
        memories.set(&NEAREST_VISIBLE_NEMESIS, sightings.nemesis);
        memories.set(&NEAREST_VISIBLE_HUNTABLE_HOGLIN, sightings.huntable_hoglin);
        memories.set(&NEAREST_VISIBLE_BABY_HOGLIN, sightings.baby_hoglin);
        memories.set(&NEAREST_VISIBLE_ZOMBIFIED, sightings.zombified);
        memories.set(
            &NEAREST_TARGETABLE_PLAYER_NOT_WEARING_GOLD,
            sightings.player_not_wearing_gold,
        );
        memories.set(
            &NEAREST_PLAYER_HOLDING_WANTED_ITEM,
            sightings.player_holding_wanted_item,
        );
        memories.set(&NEARBY_ADULT_PIGLINS, Some(adult_piglins));
        memories.set(
            &NEAREST_VISIBLE_ADULT_PIGLINS,
            Some(sightings.visible_adult_piglins),
        );
        memories.set(
            &VISIBLE_ADULT_PIGLIN_COUNT,
            Some(visible_adult_piglin_count),
        );
        memories.set(
            &VISIBLE_ADULT_HOGLIN_COUNT,
            Some(sightings.visible_adult_hoglin_count),
        );
    }
}

/// Finds the nearest wither skeleton or wither and the nearby adult piglins.
pub(super) struct PiglinBruteSpecificSensor;

impl<E: Mob> Sensor<E> for PiglinBruteSpecificSensor {
    fn register_memories(&self, memories: &mut Memories) {
        memories.register(&NEAREST_VISIBLE_LIVING_ENTITIES);
        memories.register(&NEAREST_VISIBLE_NEMESIS);
        memories.register(&NEARBY_ADULT_PIGLINS);
    }

    fn do_tick(&mut self, _world: &Arc<World>, body: &E) {
        let Some(memories) = body.brain_memories() else {
            return;
        };
        let nemesis = memories
            .lock()
            .get(&NEAREST_VISIBLE_LIVING_ENTITIES)
            .and_then(|visible| visible.find_closest(|entity| is_nemesis(entity.as_ref())));
        let adult_piglins = find_nearby_adult_piglins(body);
        let mut memories = memories.lock();
        memories.set(&NEAREST_VISIBLE_NEMESIS, nemesis);
        memories.set(&NEARBY_ADULT_PIGLINS, Some(adult_piglins));
    }
}
