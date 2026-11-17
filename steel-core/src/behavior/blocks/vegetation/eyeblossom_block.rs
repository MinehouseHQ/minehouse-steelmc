use std::sync::Arc;

use glam::DVec3;
use rand::{Rng, RngExt};
use steel_macros::block_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::particle_type::{ParticleData, TrailParticleOption};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::{
    sound_events, vanilla_blocks, vanilla_entities, vanilla_game_events, vanilla_mob_effects,
    vanilla_particle_types,
};
use steel_utils::RgbColor;
use steel_utils::types::{Difficulty, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId, Direction};

use crate::behavior::block::BlockBehavior;
use crate::behavior::blocks::vegetation::vegetation_block::survival_update_shape;
use crate::behavior::context::BlockPlaceContext;
use crate::entity::entities::attracts_bees;
use crate::entity::{Entity, InsideBlockEffectCollector, MobEffectInstance};
use crate::world::game_event::GameEventContext;
use crate::world::{LevelReader, ScheduledTickAccess, World};

use super::{BlockRef, default_surviving_state, survives_on_tag};

/// Vanilla's horizontal reach of the wave that flips nearby eyeblossoms.
const EYEBLOSSOM_XZ_RANGE: i32 = 3;
/// Vanilla's vertical reach of the wave that flips nearby eyeblossoms.
const EYEBLOSSOM_Y_RANGE: i32 = 2;
/// Vanilla's poison duration for bees inside an open eyeblossom.
const BEE_POISON_TICKS: i32 = 25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Vanilla open/closed eyeblossom type from `classes.json`.
pub enum EyeblossomType {
    /// Emits open-eyeblossom effects and transforms closed at daytime.
    Open,
    /// Emits closed-eyeblossom effects and transforms open at nighttime.
    Closed,
}

impl EyeblossomType {
    /// Vanilla `Type.fromBoolean`.
    #[must_use]
    pub const fn from_open(open: bool) -> Self {
        if open { Self::Open } else { Self::Closed }
    }

    /// Vanilla `Type.open`.
    #[must_use]
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }

    /// Vanilla `Type.transform`: the other type.
    #[must_use]
    pub const fn transform(self) -> Self {
        Self::from_open(!self.is_open())
    }

    /// Vanilla `Type.state`: the planted eyeblossom of this type.
    #[must_use]
    pub fn state(self) -> BlockStateId {
        if self.is_open() {
            vanilla_blocks::OPEN_EYEBLOSSOM.default_state()
        } else {
            vanilla_blocks::CLOSED_EYEBLOSSOM.default_state()
        }
    }

    /// Vanilla `Type.longSwitchSound`, played when the flower flips on its
    /// own.
    #[must_use]
    pub fn long_switch_sound(self) -> SoundEventRef {
        if self.is_open() {
            &sound_events::BLOCK_EYEBLOSSOM_OPEN_LONG
        } else {
            &sound_events::BLOCK_EYEBLOSSOM_CLOSE_LONG
        }
    }

    /// Vanilla `Type.shortSwitchSound`, played when a neighbor's wave flips
    /// the flower.
    fn short_switch_sound(self) -> SoundEventRef {
        if self.is_open() {
            &sound_events::BLOCK_EYEBLOSSOM_OPEN
        } else {
            &sound_events::BLOCK_EYEBLOSSOM_CLOSE
        }
    }

    /// Vanilla `Type.particleColor`.
    const fn particle_color(self) -> i32 {
        if self.is_open() {
            16_545_810
        } else {
            6_250_335
        }
    }

    /// Vanilla `Type.spawnTransformParticle`: one trail rising from the
    /// block center.
    pub fn spawn_transform_particle(self, world: &Arc<World>, pos: BlockPos, rng: &mut dyn Rng) {
        let start = DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()) + 0.5,
            f64::from(pos.z()) + 0.5,
        );
        let lifetime = 0.5 + rng.random::<f64>();
        let velocity = DVec3::new(
            rng.random::<f64>() - 0.5,
            rng.random::<f64>() + 1.0,
            rng.random::<f64>() - 0.5,
        );
        let target = start + velocity * lifetime;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "vanilla casts the trail lifetime in ticks to int"
        )]
        let duration = (20.0 * lifetime) as i32;
        let particle = ParticleData::new(
            &vanilla_particle_types::TRAIL,
            TrailParticleOption::new(target, RgbColor::new(self.particle_color()), duration),
        );
        world.send_particles(particle, start, 1, DVec3::ZERO, 0.0);
    }
}

/// Vanilla `EyeblossomBlock`: opens at night and closes at day, flipping
/// matching eyeblossoms nearby in a wave, and poisons bees inside when open.
#[block_behavior]
pub struct EyeblossomBlock {
    block: BlockRef,
    #[json_arg(r#enum = "EyeblossomType", json = "type")]
    eyeblossom_type: EyeblossomType,
}

impl EyeblossomBlock {
    /// Creates a new eyeblossom behavior.
    #[must_use]
    pub const fn new(block: BlockRef, eyeblossom_type: EyeblossomType) -> Self {
        Self {
            block,
            eyeblossom_type,
        }
    }

    /// Vanilla `tryChangingState`: flips the flower when the environment
    /// says otherwise, and schedules the same flowers within 3 blocks
    /// (2 vertically) to follow, later the farther they are.
    fn try_changing_state(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        rng: &mut dyn Rng,
    ) -> bool {
        let is_open = self.eyeblossom_type.is_open();
        if world.eyeblossom_open().unwrap_or(is_open) == is_open {
            return false;
        }
        let new_type = self.eyeblossom_type.transform();
        world.set_block(pos, new_type.state(), UpdateFlags::UPDATE_ALL);
        world.game_event(
            &vanilla_game_events::BLOCK_CHANGE,
            pos,
            &GameEventContext::new(None, Some(state)),
        );
        new_type.spawn_transform_particle(world, pos, rng);
        for dx in -EYEBLOSSOM_XZ_RANGE..=EYEBLOSSOM_XZ_RANGE {
            for dy in -EYEBLOSSOM_Y_RANGE..=EYEBLOSSOM_Y_RANGE {
                for dz in -EYEBLOSSOM_XZ_RANGE..=EYEBLOSSOM_XZ_RANGE {
                    let nearby = pos.offset(dx, dy, dz);
                    if world.get_block_state(nearby) != state {
                        continue;
                    }
                    let distance = f64::from(dx * dx + dy * dy + dz * dz).sqrt();
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "vanilla casts the scaled distances to int"
                    )]
                    let delay =
                        rng.random_range((distance * 5.0) as i32..=(distance * 10.0) as i32);
                    world.schedule_block_tick_default(nearby, state.get_block(), delay);
                }
            }
        }
        true
    }

    fn switch_with_sound(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        sound: fn(EyeblossomType) -> SoundEventRef,
    ) {
        if self.try_changing_state(state, world, pos, &mut rand::rng()) {
            let new_type = self.eyeblossom_type.transform();
            world.play_sound(sound(new_type), SoundSource::Blocks, pos, 1.0, 1.0, None);
        }
    }
}

impl BlockBehavior for EyeblossomBlock {
    fn can_survive(&self, _state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> bool {
        survives_on_tag(world, pos, &BlockTag::SUPPORTS_VEGETATION)
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        _direction: Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        survival_update_shape(self, state, world, pos)
    }

    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        default_surviving_state(self.block, self, context)
    }

    fn random_tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        self.switch_with_sound(state, world, pos, EyeblossomType::long_switch_sound);
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        self.switch_with_sound(state, world, pos, EyeblossomType::short_switch_sound);
    }

    fn entity_inside(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        _pos: BlockPos,
        entity: &dyn Entity,
        _effect_collector: &mut InsideBlockEffectCollector,
        _is_precise: bool,
    ) {
        if world.difficulty() == Difficulty::Peaceful
            || entity.entity_type() != &vanilla_entities::BEE
            || !attracts_bees(state)
        {
            return;
        }
        let Some(bee) = entity.as_living_entity() else {
            return;
        };
        if bee.has_mob_effect(vanilla_mob_effects::POISON) {
            return;
        }
        bee.add_mob_effect(MobEffectInstance::with_duration(
            vanilla_mob_effects::POISON,
            BEE_POISON_TICKS,
            0,
        ));
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_world_clocks};
    use steel_utils::{BlockPos, ChunkPos};

    use crate::behavior::{BLOCK_BEHAVIORS, init_behaviors};
    use crate::test_support::{TestLevel, fresh_test_world, insert_ready_full_chunk};

    use super::*;

    fn level_with_support(support: BlockRef) -> TestLevel {
        TestLevel::default().with_block(BlockPos::new(0, 63, 0), support.default_state())
    }

    #[test]
    fn eyeblossom_requires_vegetation_support() {
        init_vanilla_registry();
        let behavior =
            EyeblossomBlock::new(&vanilla_blocks::CLOSED_EYEBLOSSOM, EyeblossomType::Closed);
        let pos = BlockPos::new(0, 64, 0);
        let state = vanilla_blocks::CLOSED_EYEBLOSSOM.default_state();

        assert!(behavior.can_survive(state, &level_with_support(&vanilla_blocks::DIRT), pos));
        assert!(!behavior.can_survive(state, &level_with_support(&vanilla_blocks::AIR), pos));
    }

    const NOON: i64 = 6_000;
    const MIDNIGHT: i64 = 18_000;

    fn random_tick(world: &Arc<World>, pos: BlockPos) {
        let state = world.get_block_state(pos);
        BLOCK_BEHAVIORS
            .get_behavior(state.get_block())
            .random_tick(state, world, pos);
    }

    fn plant(world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        world.set_block(
            pos.below(),
            vanilla_blocks::DIRT.default_state(),
            UpdateFlags::UPDATE_NONE,
        );
        world.set_block(pos, state, UpdateFlags::UPDATE_NONE);
    }

    #[test]
    fn eyeblossoms_open_at_night_and_close_at_noon() {
        init_vanilla_registry();
        init_behaviors();
        let fixture = fresh_test_world("eyeblossom_day_night");
        let world = &fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        plant(
            world,
            pos,
            vanilla_blocks::CLOSED_EYEBLOSSOM.default_state(),
        );

        world.set_clock_total_ticks(&vanilla_world_clocks::OVERWORLD, NOON);
        assert_eq!(world.eyeblossom_open(), Some(false));
        random_tick(world, pos);
        assert_eq!(
            world.get_block_state(pos).get_block(),
            &vanilla_blocks::CLOSED_EYEBLOSSOM,
            "closed stays closed by day"
        );

        world.set_clock_total_ticks(&vanilla_world_clocks::OVERWORLD, MIDNIGHT);
        assert_eq!(world.eyeblossom_open(), Some(true));
        random_tick(world, pos);
        assert_eq!(
            world.get_block_state(pos).get_block(),
            &vanilla_blocks::OPEN_EYEBLOSSOM
        );

        world.set_clock_total_ticks(&vanilla_world_clocks::OVERWORLD, NOON);
        random_tick(world, pos);
        assert_eq!(
            world.get_block_state(pos).get_block(),
            &vanilla_blocks::CLOSED_EYEBLOSSOM
        );
    }

    #[test]
    fn a_flipping_eyeblossom_schedules_its_neighbors() {
        init_vanilla_registry();
        init_behaviors();
        let fixture = fresh_test_world("eyeblossom_wave");
        let world = &fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        let near = pos.offset(2, 0, 1);
        let far = pos.offset(4, 0, 0);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        let closed = vanilla_blocks::CLOSED_EYEBLOSSOM.default_state();
        for at in [pos, near, far] {
            plant(world, at, closed);
        }
        world.set_clock_total_ticks(&vanilla_world_clocks::OVERWORLD, MIDNIGHT);

        random_tick(world, pos);

        let block = &vanilla_blocks::CLOSED_EYEBLOSSOM;
        assert!(
            world.has_scheduled_block_tick(near, block),
            "a near flower follows"
        );
        assert!(
            !world.has_scheduled_block_tick(far, block),
            "4 blocks away is out of reach"
        );
    }

    #[test]
    fn potted_eyeblossoms_follow_the_night() {
        init_vanilla_registry();
        init_behaviors();
        let fixture = fresh_test_world("eyeblossom_potted");
        let world = &fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        world.set_block(
            pos,
            vanilla_blocks::POTTED_CLOSED_EYEBLOSSOM.default_state(),
            UpdateFlags::UPDATE_NONE,
        );

        world.set_clock_total_ticks(&vanilla_world_clocks::OVERWORLD, MIDNIGHT);
        random_tick(world, pos);
        assert_eq!(
            world.get_block_state(pos).get_block(),
            &vanilla_blocks::POTTED_OPEN_EYEBLOSSOM
        );
    }
}
