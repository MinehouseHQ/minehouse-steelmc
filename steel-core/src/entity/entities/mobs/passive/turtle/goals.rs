//! Goals of vanilla `Turtle`.

use glam::DVec3;
use std::f32::consts::PI;
use steel_registry::vanilla_blocks;
use steel_registry::{sound_events, vanilla_game_events};
use steel_utils::{BlockPos, Downcast as _};

use super::{TurtleEntity, is_sand};
use crate::entity::ai::goal::{
    BreedGoal, Goal, GoalControls, MoveToBlockGoal, RandomStrollGoal, default_random_pos_towards,
    reduced_tick_delay,
};
use crate::entity::{
    AgeableMob as _, Animal as _, Entity as _, Mob as _, PathfinderMob, SharedEntity,
};
use crate::world::custom_spawner::has_chunks_at;
use crate::world::game_event::GameEventContext;
use crate::world::{LevelReader as _, World};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_utils::types::UpdateFlags;

/// Vanilla `TurtleGoHomeGoal.GIVE_UP_TICKS`.
const GO_HOME_GIVE_UP_TICKS: i32 = 600;
/// Vanilla `TurtleGoToWaterGoal.GIVE_UP_TICKS`.
const GO_TO_WATER_GIVE_UP_TICKS: i32 = 1200;
/// Vanilla `TurtleLayEggGoal`: ticks of digging before the egg is laid.
const LAY_EGG_DIG_TICKS: i32 = 200;
/// Vanilla `TurtleTravelGoal`: chunks within 34 blocks of the next step
/// must be loaded.
const TRAVEL_LOADED_RADIUS: i32 = 34;

fn turtle(mob: &dyn PathfinderMob) -> Option<&TurtleEntity> {
    let turtle = mob.downcast_ref::<TurtleEntity>();
    if turtle.is_none() {
        tracing::warn!("turtle goal assigned to a non-turtle entity");
    }
    turtle
}

fn is_water(level: &World, pos: BlockPos) -> bool {
    level.get_block_state(pos).get_block() == &vanilla_blocks::WATER
}

/// Vanilla `DefaultRandomPos.getPosTowards` with the two fallbacks the
/// home and travel goals share.
fn step_towards(turtle: &TurtleEntity, target: BlockPos) -> Option<DVec3> {
    let towards = DVec3::new(
        f64::from(target.x()) + 0.5,
        f64::from(target.y()),
        f64::from(target.z()) + 0.5,
    );
    default_random_pos_towards(turtle, 16, 3, towards, f64::from(PI / 10.0))
        .or_else(|| default_random_pos_towards(turtle, 8, 7, towards, f64::from(PI / 2.0)))
}

/// Vanilla `TurtleBreedGoal`: no mating while already carrying an egg.
pub(super) struct TurtleBreedGoal {
    breed: BreedGoal,
}

impl TurtleBreedGoal {
    pub(super) const fn new(speed_modifier: f64) -> Self {
        Self {
            breed: BreedGoal::new(speed_modifier),
        }
    }
}

impl Goal for TurtleBreedGoal {
    fn controls(&self) -> GoalControls {
        self.breed.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.breed.can_use(mob) && turtle(mob).is_some_and(|turtle| !turtle.has_egg())
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.breed.can_continue_to_use(mob)
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.breed.stop(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.breed.tick(mob, entity);
    }
}

/// Vanilla `TurtleGoHomeGoal`: egg carriers, and now and then a turtle far
/// from its beach, swim back home.
pub(super) struct TurtleGoHomeGoal {
    speed_modifier: f64,
    stuck: bool,
    close_to_home_try_ticks: i32,
}

impl TurtleGoHomeGoal {
    pub(super) const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            stuck: false,
            close_to_home_try_ticks: 0,
        }
    }
}

impl Goal for TurtleGoHomeGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(turtle) = turtle(mob) else {
            return false;
        };
        if turtle.is_baby() {
            return false;
        }
        if turtle.has_egg() {
            return true;
        }
        rand::random_range(0..reduced_tick_delay(700)) == 0
            && !turtle
                .home_pos()
                .closer_to_center_than(turtle.position(), 64.0)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        if let Some(turtle) = turtle(mob) {
            turtle.set_going_home(true);
        }
        self.stuck = false;
        self.close_to_home_try_ticks = 0;
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(turtle) = turtle(mob) {
            turtle.set_going_home(false);
        }
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        turtle(mob).is_some_and(|turtle| {
            !turtle
                .home_pos()
                .closer_to_center_than(turtle.position(), 7.0)
        }) && !self.stuck
            && self.close_to_home_try_ticks <= reduced_tick_delay(GO_HOME_GIVE_UP_TICKS)
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(turtle) = turtle(mob) else {
            return;
        };
        let home = turtle.home_pos();
        let close_to_home = home.closer_to_center_than(turtle.position(), 16.0);
        if close_to_home {
            self.close_to_home_try_ticks += 1;
        }
        if !turtle.mob_base().navigation().lock().is_done() {
            return;
        }
        let mut next = step_towards(turtle, home);
        if let Some(pos) = next
            && !close_to_home
            && let Some(world) = turtle.level()
            && !is_water(&world, BlockPos::containing(pos.x, pos.y, pos.z))
        {
            let towards = DVec3::new(
                f64::from(home.x()) + 0.5,
                f64::from(home.y()),
                f64::from(home.z()) + 0.5,
            );
            next = default_random_pos_towards(turtle, 16, 5, towards, f64::from(PI / 2.0));
        }
        match next {
            Some(pos) => {
                turtle.move_to_pos(pos, self.speed_modifier);
            }
            None => self.stuck = true,
        }
    }
}

/// Vanilla `TurtleGoToWaterGoal`: a turtle on land heads back to the sea.
pub(super) struct TurtleGoToWaterGoal {
    move_to: MoveToBlockGoal,
}

impl TurtleGoToWaterGoal {
    /// Vanilla passes `turtle.isBaby() ? 2.0 : speed` while the turtle is
    /// still being built, before its age is set, so the speed is always
    /// `speed_modifier`.
    pub(super) fn new(speed_modifier: f64) -> Self {
        Self {
            move_to: MoveToBlockGoal::new(speed_modifier, 24, is_water)
                .with_vertical_search_start(-1)
                .with_recalculate_path_interval(160),
        }
    }
}

impl Goal for TurtleGoToWaterGoal {
    fn controls(&self) -> GoalControls {
        self.move_to.controls()
    }

    fn requires_update_every_tick(&self) -> bool {
        self.move_to.requires_update_every_tick()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(turtle) = turtle(mob) else {
            return false;
        };
        let in_water = turtle.is_in_water();
        if turtle.is_baby() && !in_water {
            return self.move_to.can_use(mob);
        }
        !turtle.going_home() && !in_water && !turtle.has_egg() && self.move_to.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        !mob.is_in_water()
            && self.move_to.try_ticks() <= GO_TO_WATER_GIVE_UP_TICKS
            && self
                .move_to
                .is_valid_target(&world, self.move_to.block_pos())
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.move_to.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.move_to.stop(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.move_to.tick(mob, entity);
    }
}

/// Vanilla `TurtleLayEggGoal`: back home with an egg, the turtle digs into
/// the sand and lays one to four eggs.
pub(super) struct TurtleLayEggGoal {
    move_to: MoveToBlockGoal,
}

impl TurtleLayEggGoal {
    pub(super) fn new(speed_modifier: f64) -> Self {
        Self {
            move_to: MoveToBlockGoal::new(speed_modifier, 16, |level, pos| {
                level.get_block_state(pos.above()).is_air() && is_sand(level, pos)
            }),
        }
    }

    fn near_home(turtle: &TurtleEntity) -> bool {
        turtle
            .home_pos()
            .closer_to_center_than(turtle.position(), 9.0)
    }

    /// Lays the egg on the sand block the goal walked to.
    fn lay(&self, turtle: &TurtleEntity) {
        let Some(world) = turtle.level() else {
            return;
        };
        let turtle_pos = turtle.block_position();
        world.play_block_sound(
            &sound_events::ENTITY_TURTLE_LAY_EGG,
            turtle_pos,
            0.3,
            0.9 + rand::random::<f32>() * 0.2,
            None,
        );
        let egg_pos = self.move_to.block_pos().above();
        let egg_state = vanilla_blocks::TURTLE_EGG
            .default_state()
            .set_value(&BlockStateProperties::EGGS, rand::random_range(1..=4));
        world.set_block(egg_pos, egg_state, UpdateFlags::UPDATE_ALL);
        world.game_event(
            &vanilla_game_events::BLOCK_PLACE,
            egg_pos,
            &GameEventContext::new(Some(turtle), Some(egg_state)),
        );
        turtle.set_has_egg(false);
        turtle.set_laying_egg(false);
        turtle.set_in_love_time(super::IN_LOVE_AFTER_LAYING);
    }
}

impl Goal for TurtleLayEggGoal {
    fn controls(&self) -> GoalControls {
        self.move_to.controls()
    }

    fn requires_update_every_tick(&self) -> bool {
        self.move_to.requires_update_every_tick()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        turtle(mob).is_some_and(|turtle| turtle.has_egg() && Self::near_home(turtle))
            && self.move_to.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.move_to.can_continue_to_use(mob)
            && turtle(mob).is_some_and(|turtle| turtle.has_egg() && Self::near_home(turtle))
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.move_to.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.move_to.stop(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.move_to.tick(mob, entity);
        let Some(turtle) = turtle(mob) else {
            return;
        };
        if turtle.is_in_water() || !self.move_to.is_reached_target() {
            return;
        }
        if turtle.lay_egg_counter() < 1 {
            turtle.set_laying_egg(true);
        } else if turtle.lay_egg_counter() > LAY_EGG_DIG_TICKS {
            self.lay(turtle);
        }
        if turtle.is_laying_egg() {
            turtle.increment_lay_egg_counter();
        }
    }
}

/// Vanilla `TurtleRandomStrollGoal`: strolls only on land, and not while
/// heading home.
pub(super) struct TurtleRandomStrollGoal {
    stroll: RandomStrollGoal,
}

impl TurtleRandomStrollGoal {
    pub(super) const fn new(speed_modifier: f64, interval: i32) -> Self {
        Self {
            stroll: RandomStrollGoal::with_interval(speed_modifier, interval),
        }
    }
}

impl Goal for TurtleRandomStrollGoal {
    fn controls(&self) -> GoalControls {
        self.stroll.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        turtle(mob).is_some_and(|turtle| {
            !turtle.is_in_water() && !turtle.going_home() && !turtle.has_egg()
        }) && self.stroll.can_use(mob)
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

/// Vanilla `TurtleTravelGoal`: in the water a turtle sets off for a spot up
/// to 512 blocks away.
pub(super) struct TurtleTravelGoal {
    speed_modifier: f64,
    stuck: bool,
}

impl TurtleTravelGoal {
    pub(super) const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            stuck: false,
        }
    }
}

impl Goal for TurtleTravelGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        turtle(mob)
            .is_some_and(|turtle| !turtle.going_home() && !turtle.has_egg() && turtle.is_in_water())
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(turtle) = turtle(mob) else {
            return;
        };
        let xt = rand::random_range(0..1025) - 512;
        let mut yt = rand::random_range(0..9) - 4;
        let position = turtle.position();
        if let Some(world) = turtle.level()
            && f64::from(yt) + position.y > f64::from(world.sea_level() - 1)
        {
            yt = 0;
        }
        let zt = rand::random_range(0..1025) - 512;
        turtle.set_travel_pos(Some(BlockPos::containing(
            f64::from(xt) + position.x,
            f64::from(yt) + position.y,
            f64::from(zt) + position.z,
        )));
        self.stuck = false;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(turtle) = turtle(mob) else {
            return;
        };
        let Some(travel_pos) = turtle.travel_pos() else {
            self.stuck = true;
            return;
        };
        if !turtle.mob_base().navigation().lock().is_done() {
            return;
        }
        let next = step_towards(turtle, travel_pos).filter(|pos| {
            let x = pos.x.floor() as i32;
            let z = pos.z.floor() as i32;
            turtle.level().is_some_and(|world| {
                has_chunks_at(
                    &world,
                    x - TRAVEL_LOADED_RADIUS,
                    z - TRAVEL_LOADED_RADIUS,
                    x + TRAVEL_LOADED_RADIUS,
                    z + TRAVEL_LOADED_RADIUS,
                )
            })
        });
        match next {
            Some(pos) => {
                turtle.move_to_pos(pos, self.speed_modifier);
            }
            None => self.stuck = true,
        }
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        !mob.mob_base().navigation().lock().is_done()
            && !self.stuck
            && turtle(mob).is_some_and(|turtle| {
                !turtle.going_home() && !turtle.is_in_love() && !turtle.has_egg()
            })
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(turtle) = turtle(mob) {
            turtle.set_travel_pos(None);
        }
    }
}
