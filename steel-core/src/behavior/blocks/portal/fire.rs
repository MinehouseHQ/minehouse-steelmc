//! Fire block behavior implementation.
//!
//! Vanilla splits fire into `BaseFireBlock` (portal logic, placement checks) and `FireBlock`
//! (spreading, aging). This combines the portal-relevant parts from `BaseFireBlock`.

use std::sync::Arc;

use rand::RngExt as _;
use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::{BlockStateProperties, BoolProperty};
use steel_registry::level_events;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_damage_types;
use steel_registry::vanilla_dimension_types;
use steel_registry::{REGISTRY, RegistryExt as _};
use steel_utils::Identifier;
use steel_utils::axis::Axis;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, Direction};

use crate::behavior::block::BlockBehavior;
use crate::behavior::context::BlockPlaceContext;
use crate::entity::damage::DamageSource;
use crate::entity::{Entity, InsideBlockEffectCollector, InsideBlockEffectType};
use crate::player::Player;
use crate::portal::portal_shape::{PortalShape, nether_portal_config};
use crate::world::{LevelReader, ScheduledTickAccess, World};

use super::fire_flammability::{burn_odds, ignite_odds};
use crate::behavior::blocks::redstone::TntBlock;

/// Vanilla `FireBlock.MAX_AGE`.
const MAX_AGE: u8 = 15;
/// Vanilla `FireBlock`'s connection to each side but below.
const FACE_PROPERTIES: [(Direction, BoolProperty); 5] = [
    (Direction::North, BlockStateProperties::NORTH),
    (Direction::East, BlockStateProperties::EAST),
    (Direction::South, BlockStateProperties::SOUTH),
    (Direction::West, BlockStateProperties::WEST),
    (Direction::Up, BlockStateProperties::UP),
];

/// Behavior for fire blocks.
#[block_behavior]
pub struct FireBlock {
    block: BlockRef,
}

impl FireBlock {
    /// Creates a new fire block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Returns true if the world supports nether portal creation.
    ///
    /// Vanilla expresses this in terms of dimensions; Steel checks the loaded
    /// world's vanilla dimension type.
    pub(crate) fn in_portal_world(world: &World) -> bool {
        world.dimension_type == &vanilla_dimension_types::OVERWORLD
            || world.dimension_type == &vanilla_dimension_types::THE_NETHER
    }

    /// Checks if fire can be placed at `pos`, matching vanilla's `BaseFireBlock.canBePlacedAt`.
    /// Position must be air AND (fire can survive there OR it's a valid portal location).
    pub(crate) fn can_be_placed_at(
        world: &Arc<World>,
        pos: BlockPos,
        forward_dir: Direction,
    ) -> bool {
        if !world.get_block_state(pos).is_air() {
            return false;
        }
        Self::selected_fire_can_survive_at(world.as_ref(), pos)
            || Self::is_portal(world, pos, forward_dir)
    }

    /// Steel equivalent of vanilla's `BaseFireBlock.getState` for selecting
    /// between soul fire and regular fire.
    pub(crate) fn get_state(world: &dyn LevelReader, pos: BlockPos) -> BlockStateId {
        if SoulFireBlock::can_survive_at(world, pos) {
            vanilla_blocks::SOUL_FIRE.default_state()
        } else {
            Self::state_for_placement(world, pos)
        }
    }

    /// Vanilla `FireBlock.getStateForPlacement(level, pos)`: fire hanging off
    /// flammable sides when nothing below holds it up.
    fn state_for_placement(world: &dyn LevelReader, pos: BlockPos) -> BlockStateId {
        let below = pos.below();
        let below_state = world.get_block_state(below);
        let mut state = vanilla_blocks::FIRE.default_state();
        if !can_burn(below_state) && !world.is_face_sturdy(below_state, below, Direction::Up) {
            for (direction, property) in FACE_PROPERTIES {
                state = state.set_value(
                    &property,
                    can_burn(world.get_block_state(pos.relative(direction))),
                );
            }
        }
        state
    }

    /// Vanilla `FireBlock.getStateWithAge`.
    fn state_with_age(world: &dyn LevelReader, pos: BlockPos, age: u8) -> BlockStateId {
        let state = Self::get_state(world, pos);
        if state.get_block() == &vanilla_blocks::FIRE {
            state.set_value(&BlockStateProperties::AGE_15, age)
        } else {
            state
        }
    }

    /// Vanilla `FireBlock.isValidFireLocation`: a flammable block on any side.
    fn is_valid_fire_location(world: &dyn LevelReader, pos: BlockPos) -> bool {
        Direction::ALL
            .iter()
            .any(|&direction| can_burn(world.get_block_state(pos.relative(direction))))
    }

    /// Vanilla `FireBlock.getIgniteOdds(level, pos)`: how readily fire jumps
    /// into the empty `pos` from its most flammable neighbor.
    fn ignite_odds_at(world: &dyn LevelReader, pos: BlockPos) -> i32 {
        if !world.get_block_state(pos).is_air() {
            return 0;
        }
        Direction::ALL
            .iter()
            .map(|&direction| state_ignite_odds(world.get_block_state(pos.relative(direction))))
            .fold(0, i32::max)
    }

    /// Vanilla `FireBlock.isNearRain`.
    fn is_near_rain(world: &World, pos: BlockPos) -> bool {
        world.is_raining_at(pos)
            || world.is_raining_at(pos.west())
            || world.is_raining_at(pos.east())
            || world.is_raining_at(pos.north())
            || world.is_raining_at(pos.south())
    }

    /// Vanilla `FireBlock.checkBurnOut`: the neighbor at `pos` may burn away,
    /// sometimes leaving fire behind, and TNT is primed.
    fn check_burn_out(world: &Arc<World>, pos: BlockPos, chance: i32, age: u8) {
        let mut rng = rand::rng();
        let old_state = world.get_block_state(pos);
        if rng.random_range(0..chance) >= state_burn_odds(old_state) {
            return;
        }
        if rng.random_range(0..i32::from(age) + 10) < 5 && !world.is_raining_at(pos) {
            let new_age = (age + rng.random_range(0..5_u8) / 4).min(MAX_AGE);
            world.set_block(
                pos,
                Self::state_with_age(world.as_ref(), pos, new_age),
                UpdateFlags::UPDATE_ALL,
            );
        } else {
            world.remove_block(pos, false);
        }
        if old_state.get_block() == &vanilla_blocks::TNT {
            TntBlock::prime(world, pos);
        }
    }

    /// Vanilla `FireBlock.getFireTickDelay`.
    fn fire_tick_delay() -> i32 {
        30 + rand::rng().random_range(0..10)
    }

    /// Whether the block below burns forever in this dimension.
    fn is_infiniburn(world: &World, below: BlockStateId) -> bool {
        world
            .dimension_type
            .infiniburn
            .trim_start_matches('#')
            .parse::<Identifier>()
            .is_ok_and(|tag| below.get_block().has_tag(&tag))
    }

    /// Vanilla `EnvironmentAttributes.INCREASED_FIRE_BURNOUT` at `pos`.
    fn increased_fire_burnout(world: &World, pos: BlockPos) -> bool {
        world
            .noise_biome_id(pos.x() >> 2, pos.y() >> 2, pos.z() >> 2)
            .and_then(|id| REGISTRY.biomes.by_id(usize::from(id)))
            .is_some_and(|biome| biome.increased_fire_burnout)
    }

    /// Vanilla `fireState.canSurvive(level, pos)` for a state returned by [`Self::get_state`].
    pub(crate) fn fire_state_can_survive(
        world: &dyn LevelReader,
        fire: BlockStateId,
        pos: BlockPos,
    ) -> bool {
        if fire.get_block() == &vanilla_blocks::SOUL_FIRE {
            SoulFireBlock::can_survive_at(world, pos)
        } else {
            Self::can_survive_at(world, pos)
        }
    }

    fn selected_fire_can_survive_at(world: &dyn LevelReader, pos: BlockPos) -> bool {
        SoulFireBlock::can_survive_at(world, pos) || Self::can_survive_at(world, pos)
    }

    /// Matches vanilla's `FireBlock.canSurvive`: block below has a sturdy top face,
    /// or an adjacent block is flammable.
    fn can_survive_at(world: &dyn LevelReader, pos: BlockPos) -> bool {
        let below_pos = pos.below();
        world.is_face_sturdy(world.get_block_state(below_pos), below_pos, Direction::Up)
            || Self::is_valid_fire_location(world, pos)
    }

    /// Matches vanilla's `BaseFireBlock.isPortal`: checks if placing fire here could form a portal.
    /// Requires a portal-capable world, adjacent obsidian, and a valid empty portal shape.
    fn is_portal(world: &Arc<World>, pos: BlockPos, forward_dir: Direction) -> bool {
        if !Self::in_portal_world(world) {
            return false;
        }

        let has_obsidian = Direction::ALL.iter().any(|&dir| {
            world.get_block_state(pos.relative(dir)).get_block() == &vanilla_blocks::OBSIDIAN
        });
        if !has_obsidian {
            return false;
        }

        let preferred_axis = if forward_dir.get_axis().is_horizontal() {
            forward_dir.rotate_y_counter_clockwise().get_axis()
        } else if rand::random::<bool>() {
            Axis::X
        } else {
            Axis::Z
        };

        let config = nether_portal_config();
        PortalShape::find_empty_portal_shape_with_axis(world, pos, preferred_axis, &config)
            .is_some()
    }

    fn queue_entity_contact_effects(
        effect_collector: &mut InsideBlockEffectCollector,
        fire_damage: f32,
    ) {
        effect_collector.apply(InsideBlockEffectType::ClearFreeze);
        effect_collector.apply(InsideBlockEffectType::FireIgnite);
        effect_collector.run_after(
            InsideBlockEffectType::FireIgnite,
            Box::new(move |entity| {
                if !entity.fire_immune()
                    && let Some(entity_world) = entity.level()
                {
                    entity.hurt(
                        &entity_world,
                        &DamageSource::environment(&vanilla_damage_types::IN_FIRE),
                        fire_damage,
                    );
                }
            }),
        );
    }
}

/// Vanilla `FireBlock.getIgniteOdds(state)`: waterlogged blocks never catch.
fn state_ignite_odds(state: BlockStateId) -> i32 {
    if state.try_get_value(&BlockStateProperties::WATERLOGGED) == Some(true) {
        0
    } else {
        ignite_odds(state.get_block())
    }
}

/// Vanilla `FireBlock.getBurnOdds(state)`.
fn state_burn_odds(state: BlockStateId) -> i32 {
    if state.try_get_value(&BlockStateProperties::WATERLOGGED) == Some(true) {
        0
    } else {
        burn_odds(state.get_block())
    }
}

/// Vanilla `FireBlock.canBurn`.
fn can_burn(state: BlockStateId) -> bool {
    state_ignite_odds(state) > 0
}

impl BlockBehavior for FireBlock {
    /// Vanilla `FireBlock.updateShape`: re-hang the fire on its flammable
    /// sides, or put it out once nothing holds it.
    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        _direction: Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        if self.can_survive(state, world, pos) {
            Self::state_with_age(world, pos, state.get_value(&BlockStateProperties::AGE_15))
        } else {
            vanilla_blocks::AIR.default_state()
        }
    }

    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(Self::get_state(context.world.as_ref(), context.place_pos()))
    }

    /// Vanilla `FireBlock.tick`: ages, dies in rain or without fuel, burns
    /// its neighbors away and spreads into nearby air.
    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        world.schedule_block_tick_default(pos, self.block, Self::fire_tick_delay());
        if !world.can_spread_fire_around(pos) {
            return;
        }
        if !self.can_survive(state, world.as_ref(), pos) {
            world.remove_block(pos, false);
        }

        let mut rng = rand::rng();
        let below_state = world.get_block_state(pos.below());
        let infiniburn = Self::is_infiniburn(world, below_state);
        let age = state.get_value(&BlockStateProperties::AGE_15);
        if !infiniburn
            && world.is_raining()
            && Self::is_near_rain(world, pos)
            && rng.random::<f32>() < 0.2 + f32::from(age) * 0.03
        {
            world.remove_block(pos, false);
            return;
        }

        let new_age = (age + rng.random_range(0..3_u8) / 2).min(MAX_AGE);
        if age != new_age {
            world.set_block(
                pos,
                state.set_value(&BlockStateProperties::AGE_15, new_age),
                UpdateFlags::UPDATE_NONE,
            );
        }

        if !infiniburn {
            if !Self::is_valid_fire_location(world.as_ref(), pos) {
                let below = pos.below();
                if !world.is_face_sturdy(world.get_block_state(below), below, Direction::Up)
                    || age > 3
                {
                    world.remove_block(pos, false);
                }
                return;
            }
            if age == MAX_AGE
                && rng.random_range(0..4) == 0
                && !can_burn(world.get_block_state(pos.below()))
            {
                world.remove_block(pos, false);
                return;
            }
        }

        let increased_burnout = Self::increased_fire_burnout(world, pos);
        let extra = if increased_burnout { -50 } else { 0 };
        Self::check_burn_out(world, pos.east(), 300 + extra, age);
        Self::check_burn_out(world, pos.west(), 300 + extra, age);
        Self::check_burn_out(world, pos.below(), 250 + extra, age);
        Self::check_burn_out(world, pos.above(), 250 + extra, age);
        Self::check_burn_out(world, pos.north(), 300 + extra, age);
        Self::check_burn_out(world, pos.south(), 300 + extra, age);

        let difficulty = world.difficulty() as i32;
        for xx in -1..=1 {
            for zz in -1..=1 {
                for yy in -1..=4 {
                    if xx == 0 && yy == 0 && zz == 0 {
                        continue;
                    }
                    let rate = if yy > 1 { 100 + (yy - 1) * 100 } else { 100 };
                    let test_pos = pos.offset(xx, yy, zz);
                    let ignite_odds = Self::ignite_odds_at(world.as_ref(), test_pos);
                    if ignite_odds <= 0 {
                        continue;
                    }
                    let mut odds = (ignite_odds + 40 + difficulty * 7) / (i32::from(age) + 30);
                    if increased_burnout {
                        odds /= 2;
                    }
                    if odds > 0
                        && rng.random_range(0..rate) <= odds
                        && (!world.is_raining() || !Self::is_near_rain(world, test_pos))
                    {
                        let spread_age = (age + rng.random_range(0..5_u8) / 4).min(MAX_AGE);
                        world.set_block(
                            test_pos,
                            Self::state_with_age(world.as_ref(), test_pos, spread_age),
                            UpdateFlags::UPDATE_ALL,
                        );
                    }
                }
            }
        }
    }

    fn can_survive(&self, _state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> bool {
        Self::can_survive_at(world, pos)
    }

    fn entity_inside(
        &self,
        _state: BlockStateId,
        _world: &Arc<World>,
        _pos: BlockPos,
        _entity: &dyn Entity,
        effect_collector: &mut InsideBlockEffectCollector,
        _is_precise: bool,
    ) {
        Self::queue_entity_contact_effects(effect_collector, 1.0);
    }

    fn on_place(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        old_state: BlockStateId,
        _moved_by_piston: bool,
    ) {
        world.schedule_block_tick_default(pos, self.block, Self::fire_tick_delay());
        // Only attempt portal creation when fire is newly placed, not when replacing itself
        if old_state.get_block() == state.get_block() {
            return;
        }

        if Self::in_portal_world(world)
            && let Some(shape) =
                PortalShape::find_empty_portal_shape(world, pos, &nether_portal_config())
        {
            shape.place_portal_blocks(world);
            return;
        }

        if !self.can_survive(state, world, pos) {
            world.set_block(
                pos,
                vanilla_blocks::AIR.default_state(),
                UpdateFlags::UPDATE_ALL,
            );
        }
    }
    fn player_will_destroy(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _player: &Player,
    ) -> BlockStateId {
        world.level_event(level_events::SOUND_EXTINGUISH_FIRE, pos, 0, None);
        state
    }
}

/// Behavior for soul fire survival.
///
/// Vanilla keeps this as `SoulFireBlock`, separate from normal `FireBlock`.

#[block_behavior]
pub struct SoulFireBlock {
    block: BlockRef,
}

impl SoulFireBlock {
    /// Creates a new soul fire block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    fn can_survive_at(world: &dyn LevelReader, pos: BlockPos) -> bool {
        let block_below = world.get_block_state(pos.below()).get_block();
        block_below.has_tag(&BlockTag::SOUL_FIRE_BASE_BLOCKS)
    }
}

impl BlockBehavior for SoulFireBlock {
    /// Removes soul fire when its supporting block no longer allows it to survive
    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        direction: Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        if direction == Direction::Down && !self.can_survive(state, world, pos) {
            return vanilla_blocks::AIR.default_state();
        }
        state
    }

    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        let state = self.block.default_state();
        self.can_survive(state, context.world, context.place_pos())
            .then_some(state)
    }

    fn can_survive(&self, _state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> bool {
        Self::can_survive_at(world, pos)
    }

    fn entity_inside(
        &self,
        _state: BlockStateId,
        _world: &Arc<World>,
        _pos: BlockPos,
        _entity: &dyn Entity,
        effect_collector: &mut InsideBlockEffectCollector,
        _is_precise: bool,
    ) {
        FireBlock::queue_entity_contact_effects(effect_collector, 2.0);
    }
    fn player_will_destroy(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _player: &Player,
    ) -> BlockStateId {
        world.level_event(level_events::SOUND_EXTINGUISH_FIRE, pos, 0, None);
        state
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{
        blocks::block_state_ext::BlockStateExt, init_vanilla_registry, vanilla_blocks,
    };
    use steel_utils::{BlockPos, BlockStateId, Direction};

    use crate::behavior::block::BlockBehavior;
    use crate::test_support::TestLevel;

    use super::{FireBlock, SoulFireBlock};

    const POS: BlockPos = BlockPos::new(0, 64, 0);

    fn level_with_support(support_state: BlockStateId) -> TestLevel {
        TestLevel::default()
            .with_min_y(0)
            .with_block(POS.below(), support_state)
    }

    #[test]
    fn get_state_selects_soul_fire_on_soul_fire_base_block() {
        init_vanilla_registry();

        let level = level_with_support(vanilla_blocks::SOUL_SAND.default_state());

        assert_eq!(
            FireBlock::get_state(&level, POS).get_block(),
            &vanilla_blocks::SOUL_FIRE
        );
        assert!(FireBlock::selected_fire_can_survive_at(&level, POS));
    }

    #[test]
    fn get_state_selects_regular_fire_otherwise() {
        init_vanilla_registry();

        let level = level_with_support(vanilla_blocks::STONE.default_state());

        assert_eq!(
            FireBlock::get_state(&level, POS).get_block(),
            &vanilla_blocks::FIRE
        );
    }
    mod world {
        use std::sync::Arc;

        use steel_registry::blocks::block_state_ext::BlockStateExt as _;
        use steel_registry::blocks::properties::BlockStateProperties;
        use steel_registry::{
            init_vanilla_registry, vanilla_blocks, vanilla_entities, vanilla_game_rules,
        };
        use steel_utils::types::UpdateFlags;
        use steel_utils::{BlockPos, BlockStateId, ChunkPos, WorldAabb};

        use crate::behavior::block::BlockBehavior as _;
        use crate::behavior::init_behaviors;
        use crate::test_support::{TestWorld, fresh_test_world, insert_ready_full_chunk};
        use crate::world::World;

        use super::super::FireBlock;

        const FIRE_POS: BlockPos = BlockPos::new(8, 64, 8);

        fn fire_world(name: &'static str) -> TestWorld {
            init_vanilla_registry();
            init_behaviors();
            let fixture = fresh_test_world(name);
            insert_ready_full_chunk(&fixture.world, ChunkPos::new(0, 0));
            assert!(
                fixture
                    .world
                    .set_game_rule(&vanilla_game_rules::FIRE_SPREAD_RADIUS_AROUND_PLAYER, -1)
            );
            fixture
        }

        fn set(world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
            assert!(world.set_block(pos, state, UpdateFlags::UPDATE_ALL));
        }

        fn fire(age: u8) -> BlockStateId {
            vanilla_blocks::FIRE
                .default_state()
                .set_value(&BlockStateProperties::AGE_15, age)
        }

        fn tick_fire(world: &Arc<World>) {
            let state = world.get_block_state(FIRE_POS);
            FireBlock::new(&vanilla_blocks::FIRE).tick(state, world, FIRE_POS);
        }

        #[test]
        fn fire_clings_to_flammable_sides() {
            let fixture = fire_world("fire_clings_to_sides");
            let world = &fixture.world;
            set(
                world,
                FIRE_POS.north(),
                vanilla_blocks::OAK_PLANKS.default_state(),
            );

            let state = FireBlock::get_state(world.as_ref(), FIRE_POS);
            assert!(state.get_value(&BlockStateProperties::NORTH));
            assert!(!state.get_value(&BlockStateProperties::SOUTH));
            assert!(FireBlock::can_survive_at(world.as_ref(), FIRE_POS));

            set(
                world,
                FIRE_POS.below(),
                vanilla_blocks::OAK_PLANKS.default_state(),
            );
            let on_wood = FireBlock::get_state(world.as_ref(), FIRE_POS);
            assert_eq!(on_wood, vanilla_blocks::FIRE.default_state());
        }

        #[test]
        fn fire_without_fuel_burns_out_once_old() {
            let fixture = fire_world("fire_burns_out");
            let world = &fixture.world;
            set(
                world,
                FIRE_POS.below(),
                vanilla_blocks::STONE.default_state(),
            );
            set(world, FIRE_POS, fire(4));

            tick_fire(world);

            assert!(world.get_block_state(FIRE_POS).is_air());
        }

        #[test]
        fn fire_on_netherrack_never_goes_out() {
            let fixture = fire_world("fire_infiniburn");
            let world = &fixture.world;
            set(
                world,
                FIRE_POS.below(),
                vanilla_blocks::NETHERRACK.default_state(),
            );
            set(world, FIRE_POS, fire(0));

            for _ in 0..200 {
                tick_fire(world);
            }

            let state = world.get_block_state(FIRE_POS);
            assert_eq!(state.get_block(), &vanilla_blocks::FIRE);
            assert_eq!(state.get_value(&BlockStateProperties::AGE_15), 15);
        }

        #[test]
        fn fire_burns_wool_away_and_spreads_to_wood() {
            let fixture = fire_world("fire_burns_and_spreads");
            let world = &fixture.world;
            set(
                world,
                FIRE_POS.below(),
                vanilla_blocks::NETHERRACK.default_state(),
            );
            set(
                world,
                FIRE_POS.east(),
                vanilla_blocks::WHITE_WOOL.default_state(),
            );
            // A plank two blocks up lets fire jump into the air beside it.
            let plank = FIRE_POS.above().above().west();
            set(world, plank, vanilla_blocks::OAK_PLANKS.default_state());
            set(world, FIRE_POS, fire(0));

            let mut wool_gone = false;
            let mut spread = false;
            for _ in 0..400 {
                tick_fire(world);
                wool_gone |= world.get_block_state(FIRE_POS.east()).get_block()
                    != &vanilla_blocks::WHITE_WOOL;
                spread |= [plank.above(), plank.east(), plank.below(), plank.north()]
                    .into_iter()
                    .any(|pos| world.get_block_state(pos).get_block() == &vanilla_blocks::FIRE);
                if wool_gone && spread {
                    break;
                }
            }
            assert!(wool_gone);
            assert!(spread);
        }

        #[test]
        fn burning_tnt_is_primed() {
            let fixture = fire_world("fire_primes_tnt");
            let world = &fixture.world;
            set(
                world,
                FIRE_POS.below(),
                vanilla_blocks::NETHERRACK.default_state(),
            );
            set(world, FIRE_POS.east(), vanilla_blocks::TNT.default_state());
            set(world, FIRE_POS, fire(0));

            for _ in 0..400 {
                tick_fire(world);
                if world.get_block_state(FIRE_POS.east()).get_block() != &vanilla_blocks::TNT {
                    break;
                }
            }
            assert_ne!(
                world.get_block_state(FIRE_POS.east()).get_block(),
                &vanilla_blocks::TNT
            );
            let primed = world.get_entities_in_aabb_matching(
                &WorldAabb::new(0.0, 60.0, 0.0, 16.0, 70.0, 16.0),
                |entity| entity.entity_type() == &vanilla_entities::TNT,
            );
            assert_eq!(primed.len(), 1);
        }
    }

    #[test]
    fn update_shape_removes_unsupported_fire() {
        init_vanilla_registry();
        let behavior = FireBlock::new(&vanilla_blocks::FIRE);
        let state = vanilla_blocks::FIRE.default_state();
        let level = TestLevel::default();
        let result = behavior.update_shape(
            state,
            &level,
            POS,
            Direction::Down,
            POS.below(),
            vanilla_blocks::AIR.default_state(),
        );
        assert_eq!(result.get_block(), &vanilla_blocks::AIR);
    }

    #[test]
    fn update_shape_removes_unsupported_soul_fire() {
        init_vanilla_registry();
        let behavior = SoulFireBlock::new(&vanilla_blocks::SOUL_FIRE);
        let state = vanilla_blocks::SOUL_FIRE.default_state();
        let level = TestLevel::default();
        let result = behavior.update_shape(
            state,
            &level,
            POS,
            Direction::Down,
            POS.below(),
            vanilla_blocks::AIR.default_state(),
        );
        assert_eq!(result.get_block(), &vanilla_blocks::AIR);
    }
}
