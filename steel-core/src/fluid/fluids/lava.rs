//! Lava fluid callbacks.
//!
//! Implements `FluidBehavior` and `FlowingFluid` for sharing base spread logic.

use std::sync::Arc;

use glam::DVec3;
use rand::RngExt as _;
use steel_registry::REGISTRY;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::Direction;
use steel_registry::items::ItemRef;
use steel_registry::level_events;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::sound_events;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_game_rules::LAVA_SOURCE_CONVERSION;
use steel_registry::vanilla_items;
use steel_utils::BlockPos;
use steel_utils::BlockStateId;
use steel_utils::ChunkPos;
use steel_utils::types::UpdateFlags;

use crate::behavior::blocks::FireBlock;
use crate::entity::{Entity, InsideBlockEffectCollector, InsideBlockEffectType};
use crate::fluid::{FlowingFluid, FluidBehavior, get_flow as flowing_fluid_flow};
use crate::fluid::{
    FluidRef, FluidState, FluidStateExt, get_fluid_state, get_height, is_lava_fluid,
    is_water_fluid, lava_id, liquid_block_state,
};
use crate::world::World;
const NORMAL_LAVA_ENTITY_FLOW_SCALE: f64 = 0.002_333_333_333_333_333_5;
const FAST_LAVA_ENTITY_FLOW_SCALE: f64 = 0.007;

/// Lava fluid implementation.
///
/// Implements [`FluidBehavior`] with lava-specific parameters and
/// behaviors (world-dependent spread, uphill delay, lava/water chemistry,
/// fizz sounds).
pub struct LavaFluid;

impl LavaFluid {
    /// Returns true if this world uses fast lava (nether-like).
    fn is_fast_lava(world: &Arc<World>) -> bool {
        world.dimension_type.fast_lava
    }

    /// Returns the lava current scale for entity fluid pushing.
    pub(crate) fn entity_flow_scale(world: &Arc<World>) -> f64 {
        if Self::is_fast_lava(world) {
            FAST_LAVA_ENTITY_FLOW_SCALE
        } else {
            NORMAL_LAVA_ENTITY_FLOW_SCALE
        }
    }

    /// Vanilla `LavaFluid.isFlammable`: an unloaded position never catches.
    fn is_flammable(world: &World, pos: BlockPos) -> bool {
        if !world.is_outside_build_height(pos.y()) && !is_loaded(world, pos) {
            return false;
        }
        world
            .get_block_state(pos)
            .get_block()
            .config
            .ignited_by_lava
    }

    /// Vanilla `LavaFluid.hasFlammableNeighbours`.
    fn has_flammable_neighbours(world: &World, pos: BlockPos) -> bool {
        Direction::ALL
            .into_iter()
            .any(|direction| Self::is_flammable(world, pos.relative(direction)))
    }

    /// Vanilla `randomTick`, upward branch: climbs one to two blocks and
    /// lights the first air that touches something flammable.
    fn ignite_above(world: &Arc<World>, pos: BlockPos, passes: i32) {
        let mut rng = rand::rng();
        let mut test_pos = pos;
        for _ in 0..passes {
            test_pos = test_pos.offset(rng.random_range(0..3) - 1, 1, rng.random_range(0..3) - 1);
            if !is_loaded(world, test_pos) {
                return;
            }
            let state = world.get_block_state(test_pos);
            if state.is_air() {
                if Self::has_flammable_neighbours(world, test_pos) {
                    world.set_block(
                        test_pos,
                        FireBlock::get_state(world.as_ref(), test_pos),
                        UpdateFlags::UPDATE_ALL,
                    );
                    return;
                }
            } else if state
                .get_block()
                .has_tag(&BlockTag::BLOCKS_LAVA_FIRE_SPREAD)
            {
                return;
            }
        }
    }

    /// Vanilla `randomTick`, level branch: three tries at setting fire on top
    /// of a flammable block beside the lava.
    fn ignite_beside(world: &Arc<World>, pos: BlockPos) {
        let mut rng = rand::rng();
        for _ in 0..3 {
            let test_pos = pos.offset(rng.random_range(0..3) - 1, 0, rng.random_range(0..3) - 1);
            if !is_loaded(world, test_pos) {
                return;
            }
            if world.get_block_state(test_pos.above()).is_air()
                && Self::is_flammable(world, test_pos)
            {
                world.set_block(
                    test_pos.above(),
                    FireBlock::get_state(world.as_ref(), test_pos),
                    UpdateFlags::UPDATE_ALL,
                );
            }
        }
    }
}

/// Vanilla `Level.isLoaded`.
fn is_loaded(world: &World, pos: BlockPos) -> bool {
    world.has_full_chunk(ChunkPos::from_block_pos(pos))
}

impl FluidBehavior for LavaFluid {
    /// Vanilla `LavaFluid.randomTick`: lava sets nearby flammable blocks on
    /// fire when the fire tick game rule allows it.
    fn random_tick(&self, world: &Arc<World>, pos: BlockPos) {
        if !world.can_spread_fire_around(pos) {
            return;
        }
        let passes = rand::rng().random_range(0..3);
        if passes > 0 {
            Self::ignite_above(world, pos, passes);
        } else {
            Self::ignite_beside(world, pos);
        }
    }

    fn pickup_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ITEM_BUCKET_FILL_LAVA)
    }

    fn create_legacy_block(&self, fluid_state: FluidState) -> BlockStateId {
        liquid_block_state(&vanilla_blocks::LAVA, fluid_state)
    }

    fn bucket(&self) -> ItemRef {
        &vanilla_items::LAVA_BUCKET
    }

    fn fluid_type(&self) -> FluidRef {
        lava_id()
    }

    fn is_same(&self, fluid: FluidRef) -> bool {
        is_lava_fluid(fluid)
    }

    fn tick_delay(&self, world: &Arc<World>) -> i32 {
        if Self::is_fast_lava(world) { 10 } else { 30 }
    }

    fn drop_off(&self, world: &Arc<World>) -> u8 {
        if Self::is_fast_lava(world) { 1 } else { 2 }
    }

    fn slope_find_distance(&self, world: &Arc<World>) -> u8 {
        if Self::is_fast_lava(world) { 4 } else { 2 }
    }

    fn explosion_resistance(&self) -> f32 {
        100.0
    }

    fn can_convert_to_source(&self, world: &Arc<World>) -> bool {
        world.get_game_rule(&LAVA_SOURCE_CONVERSION)
    }

    fn get_flow(&self, world: &Arc<World>, pos: BlockPos, fluid_state: FluidState) -> DVec3 {
        flowing_fluid_flow(world, pos, fluid_state)
    }

    /// Lava can be replaced if its effective height >= 0.444 and the replacer is water.
    ///
    /// Uses `get_height()` (not raw `amount / 9.0`) so that falling lava
    /// (same fluid directly above → height = 1.0) is also correctly handled.
    fn can_be_replaced_with(
        &self,
        fluid_state: FluidState,
        world: &Arc<World>,
        pos: BlockPos,
        other_fluid: FluidRef,
        _direction: Direction,
    ) -> bool {
        get_height(world, pos, fluid_state) >= 0.444_444_45 && is_water_fluid(other_fluid)
    }

    /// Uphill lava spreads 4× slower with 3/4 probability.
    ///
    /// "Uphill" means the target position (`new_state`) has a greater effective
    /// height than the source (`old_state`). Uses `get_height()` so that a
    /// falling lava source (height = 1.0) is correctly treated as "tall".
    fn get_spread_delay(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        old_state: FluidState,
        new_state: FluidState,
    ) -> i32 {
        let base = self.tick_delay(world);
        if !old_state.is_empty()
            && !new_state.is_empty()
            && !old_state.falling
            && !new_state.falling
            && get_height(world, pos, new_state) > get_height(world, pos, old_state)
            && rand::random_range(0u32..4) != 0
        {
            base * 4
        } else {
            base
        }
    }

    /// Plays the fizz sound. Lava does NOT drop block items (unlike water).
    fn before_destroying_block(&self, world: &Arc<World>, pos: BlockPos, _state: BlockStateId) {
        world.level_event(level_events::LAVA_FIZZ, pos, 0, None);
    }

    /// Clears freezing, ignites, then applies lava damage.
    fn entity_inside(
        &self,
        _world: &Arc<World>,
        _pos: BlockPos,
        _entity: &dyn Entity,
        effect_collector: &mut InsideBlockEffectCollector,
    ) {
        effect_collector.apply(InsideBlockEffectType::ClearFreeze);
        effect_collector.apply(InsideBlockEffectType::LavaIgnite);
        effect_collector.run_after(
            InsideBlockEffectType::LavaIgnite,
            Box::new(Entity::lava_hurt),
        );
    }

    fn tick(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        block_state: BlockStateId,
        fluid_state: FluidState,
    ) {
        self.base_tick(world, pos, block_state, fluid_state);
    }

    fn spread(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        block_state: BlockStateId,
        fluid_state: FluidState,
    ) {
        self.base_spread(world, pos, block_state, fluid_state);
    }
}

// Marker impl to provide base FlowingFluid logic
impl FlowingFluid for LavaFluid {
    fn spread_to(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        fluid_state: FluidState,
        direction: Direction,
    ) {
        if direction == Direction::Down {
            let below_fluid = get_fluid_state(world, pos);
            if below_fluid.is_water() {
                // Fizz always plays when lava meets water going down,
                // regardless of whether stone is formed.
                world.level_event(level_events::LAVA_FIZZ, pos, 0, None);

                // Stone only forms when the target is a pure water LiquidBlock,
                // not a waterlogged block (stairs, slabs, etc.).
                let below_block = world.get_block_state(pos).get_block();
                if below_block == &vanilla_blocks::WATER {
                    world.set_block(
                        pos,
                        REGISTRY.blocks.get_default_state_id(&vanilla_blocks::STONE),
                        UpdateFlags::UPDATE_ALL_IMMEDIATE,
                    );
                }
                return;
            }
        }

        self.base_spread_to(world, pos, fluid_state);
    }
}
