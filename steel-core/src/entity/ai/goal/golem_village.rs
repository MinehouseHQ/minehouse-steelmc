//! The iron golem's village goals (vanilla `MoveBackToVillageGoal`,
//! `GolemRandomStrollInVillageGoal`, `OfferFlowerGoal` and
//! `DefendVillageTargetGoal`).

use std::f64::consts::FRAC_PI_2;

use glam::DVec3;
use rand::seq::IndexedRandom as _;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::{REGISTRY, TaggedRegistryExt as _, vanilla_entities, vanilla_items};
use steel_utils::types::GameType;
use steel_utils::{BlockPos, Downcast as _, SectionPos, WorldAabb};

use super::random_pos::{default_random_pos_towards, land_random_pos, land_random_pos_towards};
use super::random_stroll::RandomStrollGoal;
use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};
use super::target_goal::TargetGoalBase;
use crate::entity::ai::brain::behaviors::find_section_closest_to_village;
use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::entities::mobs::golems::EQUIPMENT_SLOT_ANTENNA;
use crate::entity::entities::{IronGolemEntity, VillagerEntity};
use crate::entity::{Entity, LivingEntity, PathfinderMob, SharedEntity};
use crate::player::Player;
use crate::poi::OccupationStatus;
use crate::world::World;

/// Vanilla `MoveBackToVillageGoal`'s `RandomStrollGoal` interval.
const MOVE_BACK_INTERVAL: i32 = 10;
/// Vanilla `GolemRandomStrollInVillageGoal`'s `RandomStrollGoal` interval.
const VILLAGE_STROLL_INTERVAL: i32 = 240;
const RANDOM_POS_XZ_DISTANCE: i32 = 10;
const RANDOM_POS_Y_DISTANCE: i32 = 7;
/// Vanilla `GolemRandomStrollInVillageGoal.POI_SECTION_SCAN_RADIUS`.
const POI_SECTION_SCAN_RADIUS: i32 = 2;
/// Vanilla `GolemRandomStrollInVillageGoal.VILLAGER_SCAN_RADIUS`.
const VILLAGER_SCAN_RADIUS: f64 = 32.0;
/// Vanilla `OfferFlowerGoal.OFFER_TICKS`.
const OFFER_TICKS: i32 = 400;
/// Vanilla `OfferFlowerGoal`'s one-in-8000 chance a tick.
const OFFER_CHANCE: u32 = 8000;
/// Vanilla `OfferFlowerGoal.OFFER_TARGET_CONTEXT` range.
const OFFER_RANGE: f64 = 6.0;
/// Vanilla `DefendVillageTargetGoal`'s targeting range.
const DEFEND_RANGE: f64 = 64.0;
/// Vanilla `DefendVillageTargetGoal`: the reputation at or under which a
/// villager's golem turns on a player.
const HOSTILE_REPUTATION: i32 = -100;

/// Vanilla `Vec3.atBottomCenterOf(sectionPos.center())`.
fn section_bottom_center(section: SectionPos) -> DVec3 {
    let center = section_center(section);
    DVec3::new(
        f64::from(center.x()) + 0.5,
        f64::from(center.y()),
        f64::from(center.z()) + 0.5,
    )
}

/// Vanilla `SectionPos.center`.
const fn section_center(section: SectionPos) -> BlockPos {
    BlockPos::new(
        section.x() * 16 + 8,
        section.y() * 16 + 8,
        section.z() * 16 + 8,
    )
}

/// Vanilla `MoveBackToVillageGoal`: outside a village, wanders toward the
/// nearby section closest to one.
pub(crate) struct MoveBackToVillageGoal {
    stroll: RandomStrollGoal,
}

impl MoveBackToVillageGoal {
    #[must_use]
    pub(crate) const fn new(speed_modifier: f64, check_no_action_time: bool) -> Self {
        Self {
            stroll: RandomStrollGoal::with_interval_and_no_action_time_check(
                speed_modifier,
                MOVE_BACK_INTERVAL,
                check_no_action_time,
            ),
        }
    }
}

/// Vanilla `MoveBackToVillageGoal.getPosition`.
fn position_back_to_village(mob: &dyn PathfinderMob) -> Option<DVec3> {
    let world = mob.level()?;
    let section = SectionPos::from_block_pos(mob.block_position());
    let optimal = find_section_closest_to_village(&world, section, POI_SECTION_SCAN_RADIUS);
    if optimal == section {
        return None;
    }
    default_random_pos_towards(
        mob,
        RANDOM_POS_XZ_DISTANCE,
        RANDOM_POS_Y_DISTANCE,
        section_bottom_center(optimal),
        FRAC_PI_2,
    )
}

impl Goal for MoveBackToVillageGoal {
    fn controls(&self) -> GoalControls {
        self.stroll.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        if world.is_village(mob.block_position()) {
            return false;
        }
        self.stroll
            .can_use_with_position(mob, position_back_to_village)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.stroll.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.stroll.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.stroll.stop(mob);
    }
}

/// Vanilla `GolemRandomStrollInVillageGoal`: wanders toward villagers that
/// want a golem and the village's occupied points of interest.
pub(crate) struct GolemRandomStrollInVillageGoal {
    stroll: RandomStrollGoal,
}

impl GolemRandomStrollInVillageGoal {
    #[must_use]
    pub(crate) const fn new(speed_modifier: f64) -> Self {
        Self {
            stroll: RandomStrollGoal::with_interval_and_no_action_time_check(
                speed_modifier,
                VILLAGE_STROLL_INTERVAL,
                false,
            ),
        }
    }
}

/// Vanilla `GolemRandomStrollInVillageGoal.getPosition`.
fn village_stroll_position(mob: &dyn PathfinderMob) -> Option<DVec3> {
    if rand::random::<f32>() < 0.3 {
        return position_towards_anywhere(mob);
    }
    let target = if rand::random::<f32>() < 0.7 {
        position_towards_villager_who_wants_golem(mob).or_else(|| position_towards_poi(mob))
    } else {
        position_towards_poi(mob).or_else(|| position_towards_villager_who_wants_golem(mob))
    };
    target.or_else(|| position_towards_anywhere(mob))
}

fn position_towards_anywhere(mob: &dyn PathfinderMob) -> Option<DVec3> {
    land_random_pos(mob, RANDOM_POS_XZ_DISTANCE, RANDOM_POS_Y_DISTANCE)
}

fn position_towards_villager_who_wants_golem(mob: &dyn PathfinderMob) -> Option<DVec3> {
    let world = mob.level()?;
    let area = mob.bounding_box().inflate_xyz(
        VILLAGER_SCAN_RADIUS,
        VILLAGER_SCAN_RADIUS,
        VILLAGER_SCAN_RADIUS,
    );
    let villagers = world.get_entities_in_aabb_matching(&area, |entity| {
        entity.entity_type() == &vanilla_entities::VILLAGER
            && entity
                .downcast_ref::<VillagerEntity>()
                .is_some_and(|villager| villager.wants_to_spawn_golem(&world))
    });
    let villager = villagers.choose(&mut rand::rng())?;
    land_random_pos_towards(
        mob,
        RANDOM_POS_XZ_DISTANCE,
        RANDOM_POS_Y_DISTANCE,
        villager.position(),
    )
}

fn position_towards_poi(mob: &dyn PathfinderMob) -> Option<DVec3> {
    let world = mob.level()?;
    let section = random_village_section(&world, mob)?;
    let pos = random_poi_within_section(&world, section)?;
    land_random_pos_towards(
        mob,
        RANDOM_POS_XZ_DISTANCE,
        RANDOM_POS_Y_DISTANCE,
        DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()),
            f64::from(pos.z()) + 0.5,
        ),
    )
}

/// Vanilla `getRandomVillageSection`: a section within two of the golem's that
/// is part of a village.
fn random_village_section(world: &World, mob: &dyn PathfinderMob) -> Option<SectionPos> {
    let center = SectionPos::from_block_pos(mob.block_position());
    let radius = POI_SECTION_SCAN_RADIUS;
    let mut sections = Vec::new();
    for x in -radius..=radius {
        for y in -radius..=radius {
            for z in -radius..=radius {
                let section = SectionPos::new(center.x() + x, center.y() + y, center.z() + z);
                if world.sections_to_village(section) == 0 {
                    sections.push(section);
                }
            }
        }
    }
    sections.choose(&mut rand::rng()).copied()
}

/// Vanilla `getRandomPoiWithinSection`: an occupied point of interest within
/// eight blocks of the section's center.
fn random_poi_within_section(world: &World, section: SectionPos) -> Option<BlockPos> {
    let pois = world.poi_storage.lock().get_in_range(
        &|_| true,
        section_center(section),
        8,
        OccupationStatus::Occupied,
    );
    pois.choose(&mut rand::rng()).map(|(pos, _)| *pos)
}

impl Goal for GolemRandomStrollInVillageGoal {
    fn controls(&self) -> GoalControls {
        self.stroll.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.stroll
            .can_use_with_position(mob, village_stroll_position)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.stroll.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.stroll.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.stroll.stop(mob);
    }
}

/// Vanilla `OfferFlowerGoal`: in daylight, the golem now and then holds a
/// poppy out to a villager; a copper golem standing close takes it.
pub(crate) struct OfferFlowerGoal {
    entity: Option<SharedEntity>,
    tick: i32,
}

impl OfferFlowerGoal {
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            entity: None,
            tick: 0,
        }
    }
}

/// Vanilla `OfferFlowerGoal.getGolemBoundingBox`.
fn offer_area(mob: &dyn PathfinderMob) -> WorldAabb {
    mob.bounding_box().inflate_xyz(6.0, 2.0, 6.0)
}

/// Vanilla `ServerLevel.getNearestEntity` over the gift candidates in the
/// golem's offer area.
fn nearest_gift_candidate(world: &World, mob: &dyn PathfinderMob) -> Option<SharedEntity> {
    let conditions = TargetingConditions::for_non_combat().range(OFFER_RANGE);
    let origin = mob.position();
    let candidates = world.get_entities_in_aabb_matching(&offer_area(mob), |entity| {
        REGISTRY.entity_types.is_in_tag(
            entity.entity_type(),
            &EntityTypeTag::CANDIDATE_FOR_IRON_GOLEM_GIFT,
        ) && entity
            .as_living_entity()
            .is_some_and(|living| conditions.test(world, Some(mob), living))
    });
    let mut nearest: Option<(SharedEntity, f64)> = None;
    for candidate in candidates {
        let distance = candidate.position().distance_squared(origin);
        if nearest.as_ref().is_none_or(|(_, best)| distance < *best) {
            nearest = Some((candidate, distance));
        }
    }
    nearest.map(|(entity, _)| entity)
}

impl Goal for OfferFlowerGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        if !world.is_bright_outside() || rand::random_range(0..OFFER_CHANCE) != 0 {
            return false;
        }
        self.entity = nearest_gift_candidate(&world, mob);
        self.entity.is_some()
    }

    fn can_continue_to_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        self.tick > 0
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.tick = reduced_tick_delay(OFFER_TICKS);
        if let Some(golem) = mob.downcast_ref::<IronGolemEntity>() {
            golem.offer_flower(true);
        }
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(golem) = mob.downcast_ref::<IronGolemEntity>() {
            golem.offer_flower(false);
        }
        if self.tick == 0
            && let Some(entity) = self.entity.as_ref()
        {
            give_poppy(mob, entity.as_ref());
        }
        self.entity = None;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        if let Some(entity) = self.entity.as_ref() {
            let position = entity.position();
            mob.mob_base().controls().lock().look_control.set_look_at(
                DVec3::new(position.x, entity.get_eye_y(), position.z),
                30.0,
                30.0,
            );
        }
        self.tick -= 1;
    }
}

/// The end of vanilla `OfferFlowerGoal.stop`: a gift-taker still in reach with
/// a bare antenna gets the poppy, and always drops it.
fn give_poppy(golem: &dyn PathfinderMob, entity: &dyn Entity) {
    let Some(mob) = entity.as_mob() else {
        return;
    };
    if REGISTRY.entity_types.is_in_tag(
        entity.entity_type(),
        &EntityTypeTag::ACCEPTS_IRON_GOLEM_GIFT,
    ) && mob.equipment_in_slot(EQUIPMENT_SLOT_ANTENNA).is_empty()
        && offer_area(golem).intersects(entity.bounding_box())
    {
        mob.set_item_slot(
            EQUIPMENT_SLOT_ANTENNA,
            ItemStack::new(&vanilla_items::POPPY),
        );
        mob.set_guaranteed_drop(EQUIPMENT_SLOT_ANTENNA);
    }
}

/// Vanilla `DefendVillageTargetGoal`: turns on a player that a nearby villager
/// holds in contempt.
pub(crate) struct DefendVillageTargetGoal {
    target_goal: TargetGoalBase,
    potential_target: Option<SharedEntity>,
}

impl DefendVillageTargetGoal {
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            target_goal: TargetGoalBase::new(false, true),
            potential_target: None,
        }
    }
}

/// The player, if any, that one of `villagers` thinks least of: vanilla keeps
/// the last such pair it walks over.
fn despised_player(villagers: &[SharedEntity], players: &[SharedEntity]) -> Option<SharedEntity> {
    let mut despised = None;
    for villager in villagers {
        let Some(villager) = villager.downcast_ref::<VillagerEntity>() else {
            continue;
        };
        for player in players {
            if villager.player_reputation(player.uuid()) <= HOSTILE_REPUTATION {
                despised = Some(SharedEntity::clone(player));
            }
        }
    }
    despised
}

impl Goal for DefendVillageTargetGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::TARGET
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        let area = mob.bounding_box().inflate_xyz(10.0, 8.0, 10.0);
        let conditions = TargetingConditions::for_combat().range(DEFEND_RANGE);
        let world_ref = world.as_ref();
        let villagers = world.get_entities_in_aabb_matching(&area, |entity| {
            entity
                .downcast_ref::<VillagerEntity>()
                .is_some_and(|villager| {
                    conditions.test(world_ref, Some(mob), villager as &dyn LivingEntity)
                })
        });
        let players = world.get_entities_in_aabb_matching(&area, |entity| {
            entity
                .downcast_ref::<Player>()
                .is_some_and(|player| conditions.test(world_ref, Some(mob), player))
        });
        if let Some(target) = despised_player(&villagers, &players) {
            self.potential_target = Some(target);
        }
        self.potential_target.as_ref().is_some_and(|target| {
            target.downcast_ref::<Player>().is_none_or(|player| {
                !player.is_spectator() && player.game_mode() != GameType::Creative
            })
        })
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.target_goal.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let _ = mob.set_target(self.potential_target.as_ref());
        self.target_goal.start();
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.target_goal.stop(mob);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_items};
    use steel_utils::ChunkPos;
    use steel_utils::types::GameType;

    use super::{DefendVillageTargetGoal, give_poppy, position_back_to_village};
    use crate::behavior::init_behaviors;
    use crate::entity::ai::goal::selector::Goal as _;
    use crate::entity::ai::gossip::ReputationEventType;
    use crate::entity::entities::mobs::golems::EQUIPMENT_SLOT_ANTENNA;
    use crate::entity::entities::{CopperGolemEntity, IronGolemEntity, VillagerEntity};
    use crate::entity::{
        Entity, LivingEntity as _, Mob as _, SharedEntity, init_entities, next_entity_id,
    };
    use crate::player::ResetReason;
    use crate::test_support::{TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk};
    use crate::world::World;

    fn add<T: Entity + 'static>(world: &Arc<World>, entity: T) -> Arc<T> {
        let entity = Arc::new(entity);
        let shared: SharedEntity = Arc::<T>::clone(&entity);
        world
            .try_add_entity(shared)
            .expect("test entity should attach to the loaded chunk");
        entity
    }

    fn world(name: &'static str) -> Arc<World> {
        init_vanilla_registry();
        init_behaviors();
        init_entities();
        let fixture = fresh_test_world(name);
        insert_ready_full_chunk(&fixture.world, ChunkPos::new(0, 0));
        Arc::clone(&fixture.world)
    }

    fn golem(world: &Arc<World>) -> Arc<IronGolemEntity> {
        add(
            world,
            IronGolemEntity::new(
                &vanilla_entities::IRON_GOLEM,
                next_entity_id(),
                DVec3::new(8.5, 65.0, 8.5),
                Arc::downgrade(world),
            ),
        )
    }

    #[test]
    fn a_golem_turns_on_a_player_a_villager_despises() {
        let world = world("golem_defends_village");
        let golem = golem(&world);
        let villager = add(
            &world,
            VillagerEntity::new(
                &vanilla_entities::VILLAGER,
                next_entity_id(),
                DVec3::new(10.5, 65.0, 8.5),
                Arc::downgrade(&world),
            ),
        );
        let player =
            TestPlayerBuilder::new(Arc::clone(&world), "Villain".to_owned(), next_entity_id())
                .build();
        player.base().set_position_local(DVec3::new(6.5, 65.0, 8.5));
        player.set_client_loaded(true);
        assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));
        let mut goal = DefendVillageTargetGoal::new();

        assert!(!goal.can_use(golem.as_ref()), "a stranger is left alone");

        villager.on_reputation_event_from(ReputationEventType::VillagerKilled, player.uuid());
        assert!(goal.can_use(golem.as_ref()));
        goal.start(golem.as_ref());
        assert!(
            golem
                .target()
                .is_some_and(|target| target.uuid() == player.uuid())
        );

        player.restore_game_modes(GameType::Creative, None);
        assert!(!goal.can_use(golem.as_ref()), "creative players are spared");
    }

    #[test]
    fn a_copper_golem_in_reach_takes_the_offered_poppy() {
        let world = world("golem_offers_poppy");
        let golem = golem(&world);
        let near = add(
            &world,
            CopperGolemEntity::new(
                &vanilla_entities::COPPER_GOLEM,
                next_entity_id(),
                DVec3::new(10.5, 65.0, 8.5),
                Arc::downgrade(&world),
            ),
        );
        let far = add(
            &world,
            CopperGolemEntity::new(
                &vanilla_entities::COPPER_GOLEM,
                next_entity_id(),
                DVec3::new(15.5, 65.0, 15.5),
                Arc::downgrade(&world),
            ),
        );

        give_poppy(golem.as_ref(), near.as_ref());
        give_poppy(golem.as_ref(), far.as_ref());

        assert!(
            near.equipment_in_slot(EQUIPMENT_SLOT_ANTENNA)
                .is(&vanilla_items::POPPY)
        );
        assert!(far.equipment_in_slot(EQUIPMENT_SLOT_ANTENNA).is_empty());
    }

    #[test]
    fn with_no_village_around_there_is_nowhere_to_head_back_to() {
        let world = world("golem_no_village");
        let golem = golem(&world);
        assert!(position_back_to_village(golem.as_ref()).is_none());
    }
}
