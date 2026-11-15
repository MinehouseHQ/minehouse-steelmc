//! Vanilla `CrafterBlock`: crafts from its grid on a redstone pulse and
//! pushes the result out of its front.

use std::slice;
use std::sync::{Arc, Weak};

use glam::DVec3;
use steel_macros::block_behavior;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::{BlockStateProperties, Direction, FrontAndTop};
use steel_registry::item_stack::ItemStack;
use steel_registry::{level_events, vanilla_block_entity_types};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, Downcast as _, Identifier, WorldAabb, translations};
use text_components::TextComponent;

use crate::behavior::ITEM_BEHAVIORS;
use crate::behavior::InventoryAccess;
use crate::behavior::block::{BlockBehavior, BlockEntityCreation};
use crate::behavior::context::{
    BlockHitResult, BlockPlaceContext, InteractionResult, PlacementSource,
};
use crate::behavior::dispense::spawn_item;
use crate::block_entity::entities::{
    CRAFTER_GRID_SIZE, CrafterBlockEntity, CrafterContainer, HopperBlockEntity, add_item,
};
use crate::block_entity::{BLOCK_ENTITIES, BlockEntityTicker, SharedBlockEntity};
use crate::entity::Entity as _;
use crate::inventory::lock::{ContainerId, ContainerLockGuard, ContainerRef};
use crate::inventory::menu::kinds::crafter;
use crate::inventory::recipe_manager::{
    assemble_recipe, find_recipe, positioned_grid_input, recipe_remainders,
};
use crate::player::Player;
use crate::world::{SignalGetter as _, World};

/// Vanilla `CrafterBlock.MAX_CRAFTING_TICKS`.
const MAX_CRAFTING_TICKS: i32 = 6;
/// Vanilla `CrafterBlock.CRAFTING_TICK_DELAY`.
const CRAFTING_TICK_DELAY: i32 = 4;
/// Vanilla `DefaultDispenseItemBehavior.spawnItem` speed for a crafted item.
const SPAWN_SPEED: f64 = 6.0;
/// How far in front of the crafter's center a crafted item appears.
const SPAWN_OFFSET: f64 = 0.7;
/// Side of the box around the crafter whose players get `crafter_recipe_crafted`.
const CRAFTER_TRIGGER_BOX: f64 = 17.0;

/// Vanilla `CrafterBlock`.
#[block_behavior]
pub struct CrafterBlock {
    block: BlockRef,
}

impl CrafterBlock {
    /// Creates the block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    fn crafter_at(world: &World, pos: BlockPos) -> Option<SharedBlockEntity> {
        world
            .get_block_entity(pos)
            .filter(|block_entity| block_entity.downcast_ref::<CrafterBlockEntity>().is_some())
    }

    /// Vanilla `dispenseFrom`: crafts once from the grid.
    fn dispense_from(state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        let Some(block_entity) = Self::crafter_at(world, pos) else {
            return;
        };
        let Some(crafter) = block_entity.downcast_ref::<CrafterBlockEntity>() else {
            return;
        };
        let positioned =
            positioned_grid_input(CRAFTER_GRID_SIZE, CRAFTER_GRID_SIZE, crafter.items(), world);
        let input = positioned.input;
        let Some(recipe) = find_recipe(&input) else {
            world.level_event(level_events::SOUND_CRAFTER_FAIL, pos, 0, None);
            return;
        };
        let mut result = assemble_recipe(recipe, &input);
        if result.is_empty() {
            world.level_event(level_events::SOUND_CRAFTER_FAIL, pos, 0, None);
            return;
        }

        crafter.set_crafting_ticks_remaining(MAX_CRAFTING_TICKS);
        world.set_block(
            pos,
            state.set_value(&BlockStateProperties::CRAFTING, true),
            UpdateFlags::UPDATE_CLIENTS,
        );
        // Vanilla `ItemStack.onCraftedBySystem`.
        ITEM_BEHAVIORS
            .get_behavior(result.item())
            .on_crafted_post_process(&mut result, world);
        let crafted_recipe = CraftedRecipe {
            recipe: recipe.key(),
            items: crafter.items(),
        };
        Self::dispense_item(world, pos, state, &block_entity, result, &crafted_recipe);

        for remainder in recipe_remainders(recipe, &input) {
            if !remainder.is_empty() {
                Self::dispense_item(world, pos, state, &block_entity, remainder, &crafted_recipe);
            }
        }

        crafter.consume_one_of_each();
    }

    /// Vanilla `dispenseItem`: inserts into the container in front, and drops
    /// whatever does not fit.
    fn dispense_item(
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        block_entity: &SharedBlockEntity,
        result: ItemStack,
        crafted: &CraftedRecipe,
    ) {
        let direction = front(state);
        let mut remaining = result.clone();
        if let Some(targets) = HopperBlockEntity::get_container_at(world, pos.relative(direction))
            && let Some(own) = block_entity.container_ref()
        {
            remaining = insert_into(&own, targets, result, direction);
        }

        if remaining.is_empty() {
            return;
        }
        let (x, y, z) = pos.get_center();
        let (step_x, step_y, step_z) = direction.offset();
        let spawn_at = DVec3::new(
            x + f64::from(step_x) * SPAWN_OFFSET,
            y + f64::from(step_y) * SPAWN_OFFSET,
            z + f64::from(step_z) * SPAWN_OFFSET,
        );
        spawn_item(world, remaining, SPAWN_SPEED, direction, spawn_at);
        crafted.trigger_nearby(world, DVec3::new(x, y, z));
        world.level_event(level_events::SOUND_CRAFTER_CRAFT, pos, 0, None);
        world.level_event(
            level_events::PARTICLES_SHOOT_WHITE_SMOKE,
            pos,
            direction.get_3d_data_value(),
            None,
        );
    }

    fn open_menu(world: &World, pos: BlockPos, player: &Player) -> bool {
        let Some(block_entity) = Self::crafter_at(world, pos) else {
            return false;
        };
        let Some(crafter_entity) = block_entity.downcast_ref::<CrafterBlockEntity>() else {
            return false;
        };
        let Some(container) = block_entity.container_ref() else {
            return false;
        };
        if !ContainerRef::prepare_open(slice::from_ref(&container), player) {
            return false;
        }
        let title = crafter_entity
            .custom_name()
            .unwrap_or_else(|| TextComponent::translated(translations::CONTAINER_CRAFTER.msg()));
        let data = Arc::clone(crafter_entity.data());
        let inventory = Arc::clone(&player.inventory);
        player.open_menu(title, move |context| {
            crafter(inventory, context.container_id, container, data)
        });
        true
    }
}

/// The face a crafter pushes its result out of.
fn front(state: BlockStateId) -> Direction {
    state
        .try_get_value(&BlockStateProperties::ORIENTATION)
        .unwrap_or(FrontAndTop::NorthUp)
        .front()
}

/// Vanilla `dispenseItem` into a container: one item at a time into another
/// crafter or when the stack is larger than the container allows, otherwise
/// as much as fits per attempt. Returns what is left.
fn insert_into(
    own: &ContainerRef,
    targets: Vec<ContainerRef>,
    result: ItemStack,
    direction: Direction,
) -> ItemStack {
    let source_id = own.container_id();
    let target_ids: Vec<ContainerId> = targets.iter().map(ContainerRef::container_id).collect();
    let Some(&first) = target_ids.first() else {
        return result;
    };
    let mut locked = vec![own.clone()];
    locked.extend(targets);
    let mut guard = ContainerLockGuard::lock_all(&locked);
    let one_at_a_time = guard.get_typed::<CrafterContainer>(first).is_some()
        || guard.get(first).is_some_and(|container| {
            result.count() > container.get_max_stack_size_for_item(&result)
        });
    let into_face = Some(direction.opposite());
    let mut remaining = result;
    if one_at_a_time {
        while !remaining.is_empty() {
            let rest = add_item(
                &mut guard,
                Some(source_id),
                &target_ids,
                remaining.copy_with_count(1),
                into_face,
            );
            if !rest.is_empty() {
                break;
            }
            remaining.shrink(1);
        }
    } else {
        while !remaining.is_empty() {
            let before = remaining.count();
            remaining = add_item(
                &mut guard,
                Some(source_id),
                &target_ids,
                remaining,
                into_face,
            );
            if remaining.count() == before {
                break;
            }
        }
    }
    remaining
}

impl BlockBehavior for CrafterBlock {
    /// Vanilla `getStateForPlacement`: the front faces the player, and a crafter
    /// facing up or down keeps its top toward the player's horizontal facing.
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        let front = context.get_nearest_looking_direction().opposite();
        let top = match front {
            Direction::Down => context.horizontal_direction().opposite(),
            Direction::Up => context.horizontal_direction(),
            Direction::North | Direction::South | Direction::West | Direction::East => {
                Direction::Up
            }
        };
        let orientation = FrontAndTop::from_front_and_top(front, top)?;
        Some(
            self.block
                .default_state()
                .set_value(&BlockStateProperties::ORIENTATION, orientation)
                .set_value(
                    &BlockStateProperties::TRIGGERED,
                    context.world.has_neighbor_signal(context.place_pos()),
                ),
        )
    }

    /// Vanilla `setPlacedBy`: a crafter placed powered crafts after the delay.
    fn set_placed_by(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source: &PlacementSource<'_>,
    ) {
        if state.get_value(&BlockStateProperties::TRIGGERED) {
            world.schedule_block_tick_default(pos, self.block, CRAFTING_TICK_DELAY);
        }
    }

    /// Vanilla `neighborChanged`: a rising signal crafts in 4 ticks; a falling
    /// one also ends the crafting animation.
    fn handle_neighbor_changed(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source_block: BlockRef,
        _moved_by_piston: bool,
    ) {
        let powered = world.has_neighbor_signal(pos);
        let triggered = state.get_value(&BlockStateProperties::TRIGGERED);
        let crafter = Self::crafter_at(world, pos);
        let set_triggered = |value: bool| {
            if let Some(entity) = crafter
                .as_ref()
                .and_then(|block_entity| block_entity.downcast_ref::<CrafterBlockEntity>())
            {
                entity.set_triggered(value);
            }
        };
        if powered && !triggered {
            world.schedule_block_tick_default(pos, self.block, CRAFTING_TICK_DELAY);
            world.set_block(
                pos,
                state.set_value(&BlockStateProperties::TRIGGERED, true),
                UpdateFlags::UPDATE_CLIENTS,
            );
            set_triggered(true);
        } else if !powered && triggered {
            world.set_block(
                pos,
                state
                    .set_value(&BlockStateProperties::TRIGGERED, false)
                    .set_value(&BlockStateProperties::CRAFTING, false),
                UpdateFlags::UPDATE_CLIENTS,
            );
            set_triggered(false);
        }
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        Self::dispense_from(state, world, pos);
    }

    fn open_menu_provider(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
    ) -> bool {
        Self::open_menu(world, pos, player)
    }

    fn use_without_item(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        Self::open_menu(world, pos, player);
        InteractionResult::Success
    }

    fn affect_neighbors_after_removal(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _moved_by_piston: bool,
    ) {
        world.update_neighbor_for_output_signal(pos, self.block);
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::CRAFTER,
            level,
            pos,
            state,
        ))
    }

    fn get_block_entity_ticker(
        &self,
        _world: &Arc<World>,
        _state: BlockStateId,
        block_entity_type: BlockEntityTypeRef,
    ) -> Option<BlockEntityTicker> {
        BlockEntityTicker::for_matching_entity_tick(
            block_entity_type,
            &vanilla_block_entity_types::CRAFTER,
        )
    }

    fn has_analog_output_signal(&self, _state: BlockStateId) -> bool {
        true
    }

    fn get_analog_output_signal(
        &self,
        _state: BlockStateId,
        world: &World,
        pos: BlockPos,
        _direction: Direction,
    ) -> i32 {
        world
            .get_block_entity(pos)
            .and_then(|block_entity| {
                block_entity
                    .downcast_ref::<CrafterBlockEntity>()
                    .map(CrafterBlockEntity::redstone_signal)
            })
            .unwrap_or(0)
    }
}

/// The recipe a crafter crafted and the grid it used, for
/// `crafter_recipe_crafted`.
struct CraftedRecipe {
    recipe: &'static Identifier,
    items: Vec<ItemStack>,
}

impl CraftedRecipe {
    /// Vanilla `CRAFTER_RECIPE_CRAFTED.trigger` for every non-spectator player in
    /// the 17-block box around the crafter.
    fn trigger_nearby(&self, world: &World, center: DVec3) {
        let area = WorldAabb::of_size(
            center,
            CRAFTER_TRIGGER_BOX,
            CRAFTER_TRIGGER_BOX,
            CRAFTER_TRIGGER_BOX,
        );
        let mut players = Vec::new();
        world.players.iter_players(|_, player| {
            if !player.is_spectator() && player.bounding_box().intersects(area) {
                players.push(Arc::clone(player));
            }
            true
        });
        for player in players {
            player.trigger_crafter_recipe_crafted(self.recipe, &self.items);
        }
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_entities, vanilla_items};
    use steel_utils::{ChunkPos, WorldAabb};

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::block_entity::init_block_entities;
    use crate::entity::init_entities;
    use crate::test_support::{TestWorld, fresh_test_world, insert_ready_full_chunk};

    const CRAFTER: BlockPos = BlockPos::new(8, 64, 8);

    /// A crafter facing east on a stone floor, with an optional chest in front.
    fn arena(name: &'static str, chest: bool) -> TestWorld {
        init_vanilla_registry();
        init_behaviors();
        init_block_entities();
        init_entities();
        let fixture = fresh_test_world(name);
        let world = &fixture.world;
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(CRAFTER));
        for x in 0..16 {
            for z in 0..16 {
                world.set_block(
                    BlockPos::new(x, 63, z),
                    vanilla_blocks::STONE.default_state(),
                    UpdateFlags::UPDATE_ALL,
                );
            }
        }
        let state = vanilla_blocks::CRAFTER
            .default_state()
            .set_value(&BlockStateProperties::ORIENTATION, FrontAndTop::EastUp);
        assert!(world.set_block(CRAFTER, state, UpdateFlags::UPDATE_ALL));
        if chest {
            assert!(world.set_block(
                CRAFTER.relative(Direction::East),
                vanilla_blocks::CHEST.default_state(),
                UpdateFlags::UPDATE_ALL,
            ));
        }
        fixture
    }

    fn crafter_entity(world: &World) -> SharedBlockEntity {
        world.get_block_entity(CRAFTER).expect("a crafter")
    }

    fn fill(world: &World, slots: &[(usize, ItemStack)]) {
        let block_entity = crafter_entity(world);
        let crafter = block_entity
            .downcast_ref::<CrafterBlockEntity>()
            .expect("a crafter block entity");
        for (slot, stack) in slots {
            crafter.set_item(*slot, stack.clone());
        }
    }

    fn craft(world: &Arc<World>) {
        CrafterBlock::dispense_from(world.get_block_state(CRAFTER), world, CRAFTER);
    }

    fn chest_items(world: &World) -> Vec<ItemStack> {
        let container = world
            .get_block_entity(CRAFTER.relative(Direction::East))
            .and_then(ContainerRef::from_block_entity)
            .expect("a chest");
        let guard = ContainerLockGuard::lock_all(&[&container]);
        guard
            .get(container.container_id())
            .expect("the chest is locked")
            .items()
            .iter()
            .filter(|stack| !stack.is_empty())
            .cloned()
            .collect()
    }

    fn grid(world: &World) -> Vec<ItemStack> {
        crafter_entity(world)
            .downcast_ref::<CrafterBlockEntity>()
            .expect("a crafter block entity")
            .items()
    }

    fn crafting(world: &World) -> bool {
        world
            .get_block_state(CRAFTER)
            .get_value(&BlockStateProperties::CRAFTING)
    }

    #[test]
    fn a_pulse_crafts_into_the_container_in_front_and_uses_one_of_each_item() {
        let fixture = arena("crafter_into_chest", true);
        let world = &fixture.world;
        fill(
            world,
            &[(4, ItemStack::with_count(&vanilla_items::OAK_LOG, 2))],
        );

        craft(world);

        assert_eq!(
            chest_items(world),
            [ItemStack::with_count(&vanilla_items::OAK_PLANKS, 4)]
        );
        assert_eq!(
            grid(world)[4],
            ItemStack::with_count(&vanilla_items::OAK_LOG, 1)
        );
        assert!(crafting(world));

        let block_entity = crafter_entity(world);
        for _ in 0..MAX_CRAFTING_TICKS - 1 {
            block_entity.tick(world);
            assert!(crafting(world));
        }
        block_entity.tick(world);
        assert!(!crafting(world));
    }

    #[test]
    fn remainders_follow_the_result() {
        let fixture = arena("crafter_remainders", true);
        let world = &fixture.world;
        let milk = ItemStack::new(&vanilla_items::MILK_BUCKET);
        fill(
            world,
            &[
                (0, milk.clone()),
                (1, milk.clone()),
                (2, milk),
                (3, ItemStack::new(&vanilla_items::SUGAR)),
                (4, ItemStack::new(&vanilla_items::EGG)),
                (5, ItemStack::new(&vanilla_items::SUGAR)),
                (6, ItemStack::new(&vanilla_items::WHEAT)),
                (7, ItemStack::new(&vanilla_items::WHEAT)),
                (8, ItemStack::new(&vanilla_items::WHEAT)),
            ],
        );

        craft(world);

        assert_eq!(
            chest_items(world),
            [
                ItemStack::new(&vanilla_items::CAKE),
                ItemStack::with_count(&vanilla_items::BUCKET, 3),
            ]
        );
        assert!(grid(world).iter().all(ItemStack::is_empty));
    }

    #[test]
    fn without_a_container_the_result_is_dropped_in_front() {
        let fixture = arena("crafter_drop", false);
        let world = &fixture.world;
        fill(world, &[(0, ItemStack::new(&vanilla_items::OAK_LOG))]);

        craft(world);

        let dropped: Vec<_> = world
            .get_entities_in_aabb(&WorldAabb::new(9.0, 63.0, 7.0, 11.0, 66.0, 10.0))
            .into_iter()
            .filter(|entity| entity.entity_type() == &vanilla_entities::ITEM)
            .collect();
        assert_eq!(dropped.len(), 1);
        assert!(grid(world).iter().all(ItemStack::is_empty));
    }

    #[test]
    fn nothing_happens_without_a_recipe() {
        let fixture = arena("crafter_no_recipe", true);
        let world = &fixture.world;
        fill(world, &[(0, ItemStack::new(&vanilla_items::DIRT))]);

        craft(world);

        assert_eq!(chest_items(world), []);
        assert_eq!(grid(world)[0], ItemStack::new(&vanilla_items::DIRT));
        assert!(!crafting(world));
    }
}
