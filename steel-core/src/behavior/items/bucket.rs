//! Bucket item behavior implementation.
use crate::behavior::context::InteractionResult;
use crate::behavior::item_utils::create_filled_result;
use crate::behavior::{
    BLOCK_BEHAVIORS, BlockStateBehaviorExt, FLUID_BEHAVIORS, ItemBehavior, UseItemContext,
    pickup_waterlogged_block,
};
use crate::entity::{
    Entity, EntitySpawnPlacement, EntitySpawnReason, EntitySpawnRequest, add_spawned_entity,
    create_spawned_entity,
};
use crate::fluid::{fluid_state_to_block, is_lava_fluid, is_water_fluid};
use crate::player::Player;
use crate::world::{RaytraceAction, World};
use simdnbt::owned::NbtCompound;
use std::sync::Arc;
use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::Direction;
use steel_registry::data_components::vanilla_components::{BUCKET_ENTITY_DATA, CustomData};
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::fluid::{FluidRef, FluidState};
use steel_registry::item_stack::ItemStack;
use steel_registry::level_events;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::sound_events;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_fluids;
use steel_registry::vanilla_game_events;
use steel_registry::vanilla_items;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId};

use crate::world::game_event::GameEventContext;

/// A bucket holding `content`, or the empty bucket when `content` is the empty fluid.
#[item_behavior]
pub struct BucketItem {
    #[json_arg(vanilla_fluids, ref, json = "content")]
    content: FluidRef,
}

impl BucketItem {
    /// Creates a bucket behavior for `content`.
    #[must_use]
    pub const fn new(content: FluidRef) -> Self {
        Self { content }
    }
}

impl ItemBehavior for BucketItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        if self.content.is_empty {
            return use_empty_bucket(context);
        }
        use_filled_bucket(self.content, None, context)
    }
}

/// Vanilla `MobBucketItem`: a bucket holding a mob, and water unless `content` is empty.
#[item_behavior]
pub struct MobBucketItem {
    #[json_arg(vanilla_entities, json = "type")]
    entity_type: EntityTypeRef,
    #[json_arg(vanilla_fluids, ref, json = "content")]
    content: FluidRef,
    #[json_arg(sound_events, json = "empty_sound")]
    empty_sound: SoundEventRef,
}

impl MobBucketItem {
    /// Creates a mob bucket behavior.
    #[must_use]
    pub const fn new(
        entity_type: EntityTypeRef,
        content: FluidRef,
        empty_sound: SoundEventRef,
    ) -> Self {
        Self {
            entity_type,
            content,
            empty_sound,
        }
    }

    /// Vanilla `MobBucketItem.emptyContents`: an empty-content bucket only plays its sound.
    pub(crate) fn empty_contents(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        player: Option<&Player>,
        check_sneak: bool,
    ) -> bool {
        if self.content.is_empty {
            play_mob_empty_sound(world, pos, self.empty_sound, player);
            return true;
        }
        empty_contents_with_sound(
            world,
            self.content,
            pos,
            player,
            check_sneak,
            Some(self.empty_sound),
        )
    }

    /// Vanilla `MobBucketItem.checkExtraContent`: releases the mob at `pos`.
    pub(crate) fn check_extra_content(
        &self,
        world: &Arc<World>,
        player: Option<&Player>,
        stack: &ItemStack,
        pos: BlockPos,
    ) {
        self.spawn(world, stack, pos);
        world.game_event(
            &vanilla_game_events::ENTITY_PLACE,
            pos,
            &GameEventContext::new(player.map(|player| player as &dyn Entity), None),
        );
    }

    /// Vanilla `MobBucketItem.spawn`.
    fn spawn(&self, world: &Arc<World>, stack: &ItemStack, pos: BlockPos) {
        let request = EntitySpawnRequest {
            entity_type: self.entity_type,
            placement: EntitySpawnPlacement::Block {
                pos,
                try_move_down: true,
                moved_up: false,
            },
            reason: EntitySpawnReason::Bucket,
            finalize_spawn: true,
            play_ambient_sound: false,
            item_stack: Some(stack),
            user_is_operator: false,
        };
        let Ok(entity) = create_spawned_entity(world, request) else {
            return;
        };
        if let Some(bucketable) = entity.as_bucketable() {
            let tag = stack
                .get(BUCKET_ENTITY_DATA)
                .map_or_else(NbtCompound::new, CustomData::copy_tag);
            bucketable.load_from_bucket_tag(&tag);
            bucketable.set_from_bucket_flag(true);
        }
        if add_spawned_entity(world, Arc::clone(&entity)).is_ok()
            && let Some(mob) = entity.as_mob()
        {
            mob.play_ambient_sound();
        }
    }
}

impl ItemBehavior for MobBucketItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        use_filled_bucket(self.content, Some(self), context)
    }

    fn as_mob_bucket(&self) -> Option<&MobBucketItem> {
        Some(self)
    }
}

fn filled_bucket_success_stack(context: &UseItemContext) -> ItemStack {
    if context.player.has_infinite_materials() {
        context
            .inv
            .with_item(|item| item.copy_with_count(item.count()))
    } else {
        ItemStack::new(&vanilla_items::BUCKET)
    }
}

fn use_empty_bucket(context: &mut UseItemContext) -> InteractionResult {
    let (start, end) = context.player.get_ray_endpoints();

    // Raytrace: stop on source fluids
    let (hit_block, hit_direction) = context.world.raytrace(start, end, |pos, world| {
        let state = world.get_block_state(pos);
        let block = state.get_block();

        if block == &vanilla_blocks::AIR {
            return RaytraceAction::Pass;
        }

        let fluid_state = state.get_fluid_state();
        if fluid_state.is_source() {
            return RaytraceAction::ImmediateHit;
        }
        // Flowing fluid is transparent to empty buckets.
        if !fluid_state.is_empty() {
            return RaytraceAction::Pass;
        }

        RaytraceAction::CheckShape
    });

    // A miss passes, so other handlers can try.
    let (Some(hit_pos), Some(direction)) = (hit_block, hit_direction) else {
        return InteractionResult::Pass;
    };

    if !may_use_bucket_at(context, hit_pos, direction) {
        return InteractionResult::Fail;
    }

    let hit_state = context.world.get_block_state(hit_pos);
    let block_behavior = BLOCK_BEHAVIORS.get_behavior(hit_state.get_block());

    // TODO: Remove fallback once all waterloggable blocks implement pickup_block.
    let picked = block_behavior
        .pickup_block(context.world, hit_pos, hit_state, Some(context.player))
        .or_else(|| {
            pickup_waterlogged_block(
                block_behavior,
                context.world,
                hit_pos,
                hit_state,
                Some(context.player),
            )
        });
    // Fail, not pass, so the client knows no item changed.
    let Some(result) = picked else {
        return InteractionResult::Fail;
    };

    context.inv.with_item(|item| {
        context
            .player
            .award_stat(&vanilla_stat_types::ITEM_USED, item.item());
    });
    if let Some(sound) = result.sound {
        context.player.play_sound(sound, 1.0, 1.0);
    }

    context.player.trigger_filled_bucket(&result.filled_bucket);
    create_filled_result(context, result.filled_bucket, true);
    context.world.game_event(
        &vanilla_game_events::FLUID_PICKUP,
        hit_pos,
        &GameEventContext::new(Some(context.player.as_ref()), None),
    );

    InteractionResult::Success
}

fn use_filled_bucket(
    fluid: FluidRef,
    mob_bucket: Option<&MobBucketItem>,
    context: &mut UseItemContext,
) -> InteractionResult {
    let (start, end) = context.player.get_ray_endpoints();
    let (ray_block, ray_dir) = context.world.raytrace(start, end, |pos, world| {
        let state = world.get_block_state(pos);
        let block = state.get_block();
        // Filled buckets use ClipContext.Fluid.NONE: ignore fluid shapes, but
        // still test the block shape of waterlogged/container blocks.
        if block == &vanilla_blocks::AIR {
            return RaytraceAction::Pass;
        }
        RaytraceAction::CheckShape
    });

    let (Some(clicked_pos), Some(direction)) = (ray_block, ray_dir) else {
        return InteractionResult::Pass;
    };

    if !context.world.is_in_valid_bounds(clicked_pos) {
        return InteractionResult::Fail;
    }

    if !may_use_bucket_at(context, clicked_pos, direction) {
        return InteractionResult::Fail;
    }

    let clicked_state = context.world.get_block_state(clicked_pos);
    // Only water itself enters liquid containers, not water-tagged fluids.
    let is_water_bucket = fluid == &vanilla_fluids::WATER;
    let player = Some(context.player.as_ref());

    // If the primary attempt fails, the secondary one retries at the offset position,
    // whatever the bucket.
    let primary_pos =
        filled_bucket_primary_pos(clicked_state, clicked_pos, direction, is_water_bucket);

    let pour = |pos: BlockPos, check_sneak: bool| match mob_bucket {
        Some(mob_bucket) => mob_bucket.empty_contents(context.world, pos, player, check_sneak),
        None => empty_contents(context.world, fluid, pos, player, check_sneak),
    };
    if !pour(primary_pos, true) && !pour(direction.relative(clicked_pos), false) {
        return InteractionResult::Fail;
    }

    let held = context.inv.with_item(|item| item.clone());
    if let Some(mob_bucket) = mob_bucket {
        mob_bucket.check_extra_content(context.world, player, &held, primary_pos);
    }
    // TODO(advancements): vanilla fires `CriteriaTriggers.PLACED_BLOCK` here for buckets
    // holding a fluid.
    context
        .player
        .award_stat(&vanilla_stat_types::ITEM_USED, held.item());

    let result_stack = filled_bucket_success_stack(context);
    create_filled_result(context, result_stack, true);
    InteractionResult::Success
}

/// Pours `fluid` from a bucket at `pos` (Vanilla `BucketItem.emptyContents`).
///
/// `player` is `None` for a dispenser. `check_sneak` is false for the secondary
/// attempt, which ignores sneaking. Returns whether the bucket was emptied.
pub(crate) fn empty_contents(
    world: &Arc<World>,
    fluid: FluidRef,
    pos: BlockPos,
    player: Option<&Player>,
    check_sneak: bool,
) -> bool {
    empty_contents_with_sound(world, fluid, pos, player, check_sneak, None)
}

/// [`empty_contents`] with the `MobBucketItem.playEmptySound` override when
/// `mob_empty_sound` is set.
fn empty_contents_with_sound(
    world: &Arc<World>,
    fluid: FluidRef,
    pos: BlockPos,
    player: Option<&Player>,
    check_sneak: bool,
    mob_empty_sound: Option<SoundEventRef>,
) -> bool {
    if !world.is_in_valid_bounds(pos) {
        return false;
    }

    let state = world.get_block_state(pos);
    let fluid_state = state.get_fluid_state();
    let is_sneaking = player.is_some_and(Player::is_crouching);
    let is_water_bucket = fluid == &vanilla_fluids::WATER;

    // When sneaking, only air accepts the fluid here; other blocks redirect to the
    // neighbor through the secondary attempt.
    if check_sneak && is_sneaking && !state.get_block().config.is_air {
        return false;
    }

    let behavior = BLOCK_BEHAVIORS.get_behavior(state.get_block());
    let is_liquid_container = state.is_liquid_container();
    let can_place_liquid =
        is_liquid_container && behavior.can_place_liquid_with_player(state, fluid, player);
    let can_replace = state.can_be_replaced_by_fluid(fluid);

    if !can_replace && !can_place_liquid {
        return false;
    }

    // Water-tagged fluids fizz out where water evaporates (e.g. the Nether).
    // TODO: Read the per-position WATER_EVAPORATES attribute once Steel has environment attributes.
    if is_water_fluid(fluid) && world.dimension_type.water_evaporates {
        world.level_event(level_events::PARTICLES_WATER_EVAPORATING, pos, 0, None);
        return true;
    }

    if is_water_bucket && is_liquid_container {
        let source_water = FluidState::source(&vanilla_fluids::WATER);
        behavior.place_liquid(world, pos, state, source_water);
        play_empty_sound(world, pos, fluid, player, mob_empty_sound);
        return true;
    }

    // Containers that accept a fluid other than water get overwritten here too.
    // An existing source of the same fluid just consumes the bucket.
    let is_same_fluid = FLUID_BEHAVIORS
        .get_behavior(fluid)
        .is_same(fluid_state.fluid_id);

    if is_same_fluid && fluid_state.is_source() {
        play_empty_sound(world, pos, fluid, player, mob_empty_sound);
        return true;
    }

    // Destroy non-liquid replaceable blocks first so they drop their items.
    if can_replace && !state.get_block().config.liquid && !state.get_block().config.is_air {
        world.destroy_block(pos, true);
    }

    let fluid_state_to_place = fluid_state_to_block(FluidState::source(fluid));
    if world.set_block(pos, fluid_state_to_place, UpdateFlags::UPDATE_ALL_IMMEDIATE) {
        let tick_delay = FLUID_BEHAVIORS.get_behavior(fluid).tick_delay(world);
        world.schedule_fluid_tick_default(pos, fluid, tick_delay);
        play_empty_sound(world, pos, fluid, player, mob_empty_sound);
        return true;
    }
    false
}

/// Whether the player may use the held bucket against the `direction` face of `pos`.
fn may_use_bucket_at(context: &UseItemContext, pos: BlockPos, direction: Direction) -> bool {
    let item = context.inv.with_item(|item| item.clone());
    context
        .player
        .may_use_item_at(direction.relative(pos), direction, &item)
}

/// Vanilla `BucketItem.playEmptySound`, or the `MobBucketItem` override.
fn play_empty_sound(
    world: &Arc<World>,
    pos: BlockPos,
    fluid: FluidRef,
    player: Option<&Player>,
    mob_empty_sound: Option<SoundEventRef>,
) {
    match mob_empty_sound {
        Some(sound) => play_mob_empty_sound(world, pos, sound, player),
        None => play_empty_sound_and_event(world, pos, fluid, player),
    }
}

/// Vanilla `MobBucketItem.playEmptySound`: a neutral sound and no game event.
fn play_mob_empty_sound(
    world: &Arc<World>,
    pos: BlockPos,
    sound: SoundEventRef,
    player: Option<&Player>,
) {
    world.play_sound(
        sound,
        SoundSource::Neutral,
        pos,
        1.0,
        1.0,
        player.map(Player::id),
    );
}

fn play_empty_sound_and_event(
    world: &Arc<World>,
    pos: BlockPos,
    fluid: FluidRef,
    player: Option<&Player>,
) {
    let sound_event = if is_lava_fluid(fluid) {
        &sound_events::ITEM_BUCKET_EMPTY_LAVA
    } else {
        &sound_events::ITEM_BUCKET_EMPTY
    };
    // Excludes the placing player.
    world.play_block_sound(sound_event, pos, 1.0, 1.0, player.map(Player::id));
    world.game_event(
        &vanilla_game_events::FLUID_PLACE,
        pos,
        &GameEventContext::new(player.map(|player| player as &dyn Entity), None),
    );
}

fn filled_bucket_primary_pos(
    clicked_state: BlockStateId,
    clicked_pos: BlockPos,
    direction: Direction,
    is_water_bucket: bool,
) -> BlockPos {
    if is_water_bucket && clicked_state.is_liquid_container() {
        clicked_pos
    } else {
        direction.relative(clicked_pos)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        behavior::init_behaviors,
        player::game_mode::use_item,
        test_support::{TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk},
    };
    use glam::DVec3;
    use std::sync::{Arc, Weak};
    use steel_registry::entity_variant::TropicalFishPattern as RegistryPattern;
    use steel_registry::{
        RegistryHolderSet,
        data_component_predicate::DataComponentMatchers,
        data_components::{
            AdventureModePredicate, BlockPredicate, vanilla_components::CAN_PLACE_ON,
        },
        init_vanilla_registry,
        item_stack::ItemStack,
        vanilla_blocks, vanilla_items,
    };
    use steel_utils::{
        BlockPos, ChunkPos,
        types::{GameType, InteractionHand, UpdateFlags},
    };

    use super::*;
    use crate::entity::entities::{
        AxolotlEntity, CodEntity, TropicalFishEntity, TropicalFishPattern,
    };
    use crate::entity::{AbstractFish, AgeableMob, LivingEntity as _, Mob, SharedEntity};
    use crate::entity::{init_entities, update_bucket_entity_data};
    use crate::test_support::TestWorld;
    use steel_registry::DyeColor;
    use steel_registry::data_components::vanilla_components::{
        CUSTOM_NAME, TROPICAL_FISH_BASE_COLOR, TROPICAL_FISH_PATTERN, TROPICAL_FISH_PATTERN_COLOR,
    };
    use steel_registry::vanilla_entities;
    use steel_utils::Downcast as _;
    use text_components::TextComponent;

    #[test]
    fn filled_water_bucket_targets_non_waterlogged_liquid_container_in_place() {
        init_vanilla_registry();
        init_behaviors();

        let kelp = vanilla_blocks::KELP.default_state();

        assert_eq!(
            filled_bucket_primary_pos(kelp, BlockPos::ZERO, Direction::North, true),
            BlockPos::ZERO
        );
        assert_eq!(
            filled_bucket_primary_pos(kelp, BlockPos::ZERO, Direction::North, false),
            Direction::North.relative(BlockPos::ZERO)
        );
    }

    #[test]
    fn adventure_buckets_require_can_place_on_and_fail_when_denied() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("adventure_bucket_restriction");
        let world = &world_fixture.world;
        let pos = BlockPos::new(0, 65, 2);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        assert!(world.set_block(
            pos,
            vanilla_blocks::WATER.default_state(),
            UpdateFlags::UPDATE_ALL,
        ));

        let player = TestPlayerBuilder::new(Arc::clone(world), "TestPlayer", 1).build();
        player.base().set_position_local(DVec3::new(0.5, 64.0, 0.5));
        player.set_rotation((0.0, 0.0));
        player.restore_game_modes(GameType::Adventure, None);
        player
            .abilities
            .lock()
            .update_for_game_mode(GameType::Adventure);

        player
            .inventory
            .lock()
            .set_selected_item(ItemStack::new(&vanilla_items::BUCKET));
        assert_eq!(
            use_item(&player, world, InteractionHand::MainHand),
            InteractionResult::Fail
        );
        assert_eq!(
            world.get_block_state(pos),
            vanilla_blocks::WATER.default_state()
        );

        let predicate = BlockPredicate::new(
            Some(RegistryHolderSet::Direct(vec![&vanilla_blocks::WATER])),
            None,
            None,
            DataComponentMatchers::ANY,
        );
        let mut allowed_bucket = ItemStack::new(&vanilla_items::BUCKET);
        allowed_bucket.set(
            CAN_PLACE_ON,
            AdventureModePredicate::new(vec![predicate]).expect("one block predicate is valid"),
        );
        player.inventory.lock().set_selected_item(allowed_bucket);
        assert!(use_item(&player, world, InteractionHand::MainHand).consumes_action());
        assert_eq!(
            world.get_block_state(pos),
            vanilla_blocks::AIR.default_state()
        );

        assert!(world.set_block(
            pos,
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_ALL,
        ));
        player
            .inventory
            .lock()
            .set_selected_item(ItemStack::new(&vanilla_items::WATER_BUCKET));
        assert_eq!(
            use_item(&player, world, InteractionHand::MainHand),
            InteractionResult::Fail
        );
        assert_eq!(
            world.get_block_state(pos),
            vanilla_blocks::STONE.default_state()
        );
    }

    fn looking_down_at_floor(name: &'static str, id: i32) -> (TestWorld, Arc<Player>) {
        init_entities();
        let world_fixture = fresh_test_world(name);
        let world = Arc::clone(&world_fixture.world);
        let floor = BlockPos::new(0, 63, 0);
        insert_ready_full_chunk(&world, ChunkPos::from_block_pos(floor));
        assert!(world.set_block(
            floor,
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_ALL,
        ));
        let player = TestPlayerBuilder::new(Arc::clone(&world), name, id).build();
        player.base().set_position_local(DVec3::new(0.5, 64.0, 0.5));
        player.set_rotation((0.0, 90.0));
        (world_fixture, player)
    }

    fn spawned_of_type(world: &World, entity_type: EntityTypeRef) -> Vec<SharedEntity> {
        let area = steel_utils::WorldAabb::new(-4.0, 60.0, -4.0, 4.0, 70.0, 4.0);
        world.get_entities_in_aabb_matching(&area, |entity| entity.entity_type() == entity_type)
    }

    #[test]
    fn water_bucket_scoops_up_fish_with_its_pattern_and_name() {
        init_vanilla_registry();
        init_behaviors();
        let (world_fixture, player) = looking_down_at_floor("fish_pickup", 1);
        let _world = &world_fixture.world;
        player
            .inventory
            .lock()
            .set_selected_item(ItemStack::new(&vanilla_items::WATER_BUCKET));

        let fish = TropicalFishEntity::new(
            &vanilla_entities::TROPICAL_FISH,
            50,
            DVec3::ZERO,
            Weak::new(),
        );
        let mut colors = ItemStack::new(&vanilla_items::TROPICAL_FISH_BUCKET);
        colors.set(TROPICAL_FISH_PATTERN, RegistryPattern::Betty);
        colors.set(TROPICAL_FISH_BASE_COLOR, DyeColor::Red);
        colors.set(TROPICAL_FISH_PATTERN_COLOR, DyeColor::Lime);
        fish.apply_implicit_item_components(&colors);
        fish.set_custom_name(Some(TextComponent::plain("Nemo")));

        assert_eq!(
            Mob::mob_interact(&fish, player.as_ref(), InteractionHand::MainHand),
            InteractionResult::Success
        );
        assert!(fish.is_removed());

        let bucket = player.inventory.lock().get_selected_item().clone();
        assert!(bucket.is(&vanilla_items::TROPICAL_FISH_BUCKET));
        assert_eq!(
            bucket.get(TROPICAL_FISH_PATTERN),
            Some(&RegistryPattern::Betty)
        );
        assert_eq!(bucket.get(TROPICAL_FISH_BASE_COLOR), Some(&DyeColor::Red));
        assert_eq!(
            bucket.get(TROPICAL_FISH_PATTERN_COLOR),
            Some(&DyeColor::Lime)
        );
        assert!(bucket.get(CUSTOM_NAME).is_some());
        let data = bucket
            .get(BUCKET_ENTITY_DATA)
            .map_or_else(NbtCompound::new, CustomData::copy_tag);
        assert!(data.float("Health").is_some());
    }

    #[test]
    fn fish_ignore_other_items() {
        init_vanilla_registry();
        init_behaviors();
        let (_world_fixture, player) = looking_down_at_floor("fish_no_pickup", 2);
        player
            .inventory
            .lock()
            .set_selected_item(ItemStack::new(&vanilla_items::BUCKET));
        let fish = CodEntity::new(&vanilla_entities::COD, 51, DVec3::ZERO, Weak::new());

        assert_eq!(
            Mob::mob_interact(&fish, player.as_ref(), InteractionHand::MainHand),
            InteractionResult::Pass
        );
        assert!(!fish.is_removed());
    }

    #[test]
    fn mob_bucket_pours_water_and_releases_the_saved_fish() {
        init_vanilla_registry();
        init_behaviors();
        let (world_fixture, player) = looking_down_at_floor("fish_release", 3);
        let world = &world_fixture.world;

        let mut bucket = ItemStack::new(&vanilla_items::TROPICAL_FISH_BUCKET);
        bucket.set(TROPICAL_FISH_PATTERN, RegistryPattern::Glitter);
        bucket.set(TROPICAL_FISH_BASE_COLOR, DyeColor::Blue);
        update_bucket_entity_data(&mut bucket, |tag| tag.insert("Health", 1.5_f32));
        player.inventory.lock().set_selected_item(bucket);

        assert!(use_item(&player, world, InteractionHand::MainHand).consumes_action());
        assert_eq!(
            world.get_block_state(BlockPos::new(0, 64, 0)),
            vanilla_blocks::WATER.default_state()
        );
        assert!(
            player
                .inventory
                .lock()
                .get_selected_item()
                .is(&vanilla_items::BUCKET)
        );

        let released = spawned_of_type(world, &vanilla_entities::TROPICAL_FISH);
        assert_eq!(released.len(), 1);
        let fish = released[0]
            .downcast_ref::<TropicalFishEntity>()
            .expect("a tropical fish was released");
        assert!(AbstractFish::is_from_bucket(fish));
        assert_eq!(fish.variant().pattern, TropicalFishPattern::Glitter);
        assert_eq!(fish.variant().base_color, DyeColor::Blue);
        assert_eq!(fish.get_health().to_bits(), 1.5_f32.to_bits());
    }

    #[test]
    fn axolotl_bucket_keeps_age() {
        init_vanilla_registry();
        init_behaviors();
        let (world_fixture, player) = looking_down_at_floor("axolotl_bucket", 4);
        let world = &world_fixture.world;
        player
            .inventory
            .lock()
            .set_selected_item(ItemStack::new(&vanilla_items::WATER_BUCKET));
        let axolotl = AxolotlEntity::new(&vanilla_entities::AXOLOTL, 52, DVec3::ZERO, Weak::new());
        AgeableMob::set_age(&axolotl, -1200);
        assert_eq!(
            Mob::mob_interact(&axolotl, player.as_ref(), InteractionHand::MainHand),
            InteractionResult::Success
        );

        assert!(use_item(&player, world, InteractionHand::MainHand).consumes_action());
        let released = spawned_of_type(world, &vanilla_entities::AXOLOTL);
        assert_eq!(released.len(), 1);
        let baby = released[0]
            .downcast_ref::<AxolotlEntity>()
            .expect("an axolotl was released");
        assert_eq!(AgeableMob::get_age(baby), -1200);
        assert!(baby.is_from_bucket());
    }
}
