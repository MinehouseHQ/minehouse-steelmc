use std::sync::{
    Arc, Weak,
    atomic::{AtomicI32, Ordering},
};

use glam::DVec3;
use simdnbt::{
    ToNbtTag,
    borrow::{BaseNbtCompound, NbtCompound as NbtCompoundView},
    owned::{NbtCompound, NbtList, NbtTag},
};
use steel_registry::{
    REGISTRY,
    blocks::{
        behavior::PushReaction, block_state_ext::BlockStateExt, properties::BlockStateProperties,
    },
    data_components::{DataComponentMap, ItemContainerContents, vanilla_components::CONTAINER},
    item_stack::ItemStack,
    vanilla_block_entity_types,
};
use steel_utils::{
    BlockLocalAabb, BlockPos, BlockStateId, Direction, DowncastType, DowncastTypeKey, WorldAabb,
    geometry::{Aabb, Space},
    locks::SyncMutex,
    translations,
    types::UpdateFlags,
};

use steel_protocol::packets::game::SoundSource;
use steel_registry::{sound_events, vanilla_game_events};

use crate::{
    behavior::{ITEM_BEHAVIORS, blocks::ShulkerBoxBlock},
    block_entity::container_loot::ContainerLoot,
    block_entity::container_name::{
        ContainerName, apply_container_component, apply_container_loot_component,
        collect_container_loot_component, contents_from_items,
    },
    block_entity::container_openers::container_sound_pitch,
    block_entity::{BlockEntity, BlockEntityBase, ImplicitComponentGetter},
    entity::Entity as _,
    inventory::{
        container::Container,
        lock::{ContainerRef, SharedContainer},
    },
    physics::{CollisionWorld as _, MoverType, WorldCollisionProvider},
    player::Player,
    world::World,
    world::game_event::GameEventContext,
};

/// Number of slots in a shulker box (3 rows of 9).
pub const SHULKER_BOX_SLOTS: usize = 27;
const ANIMATION_STEPS: u8 = 10;
/// Block event that sets a shulker box's open count, and with it the lid.
const EVENT_SET_OPEN_COUNT: i32 = 1;

/// The current animation state of a shulker box.
#[derive(Debug, Clone, Copy)]
pub enum AnimationStatus {
    /// Fully closed.
    Closed,
    /// Opening.
    Opening,
    /// Fully open.
    Opened,
    /// Closing.
    Closing,
}

pub struct ShulkerBoxAnimation {
    animation_status: AnimationStatus,
    progress: u8,
    old_progress: u8,
}

impl ShulkerBoxAnimation {
    fn progress(&self) -> f32 {
        f32::from(self.progress) / f32::from(ANIMATION_STEPS)
    }

    fn old_progress(&self) -> f32 {
        f32::from(self.old_progress) / f32::from(ANIMATION_STEPS)
    }
}

/// Block entity backing every shulker box color, including the undyed one.
pub struct ShulkerBoxBlockEntity {
    base: Arc<BlockEntityBase>,
    container: Arc<SyncMutex<ShulkerBoxContainer>>,
    container_ref: ContainerRef,
    /// Vanilla `RandomizableContainerBlockEntity` loot table.
    loot: Arc<ContainerLoot>,
    animation: SyncMutex<ShulkerBoxAnimation>,
    open_count: AtomicI32,
    /// Vanilla `BaseContainerBlockEntity` custom name and lock.
    name: ContainerName,
}

struct ShulkerBoxContainer {
    items: Vec<ItemStack>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ShulkerBoxEntity`.
unsafe impl DowncastType for ShulkerBoxBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/shulker_box");
}

// SAFETY: This key is owned by Steel and uniquely identifies the independently
// lockable inventory data used by a shulker box block entity.
unsafe impl DowncastType for ShulkerBoxContainer {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:container/shulker_box");
}

fn do_neighbor_updates(world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
    world.update_neighbour_shapes(state, pos, UpdateFlags::UPDATE_ALL, 512);
    world.update_neighbors_at(pos, state.get_block());
}

impl ShulkerBoxBlockEntity {
    /// Creates a new shulker box block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        let base = Arc::new(BlockEntityBase::new(
            &vanilla_block_entity_types::SHULKER_BOX,
            level,
            pos,
            state,
        ));
        let container = Arc::new(SyncMutex::new(ShulkerBoxContainer {
            items: vec![ItemStack::empty(); SHULKER_BOX_SLOTS],
        }));
        let shared_container: SharedContainer =
            Arc::<SyncMutex<ShulkerBoxContainer>>::clone(&container);
        let loot = Arc::new(ContainerLoot::new());
        Self {
            container_ref: ContainerRef::owned_by_randomizable_block_entity(
                shared_container,
                Arc::clone(&base),
                Arc::clone(&loot),
            ),
            loot,
            base,
            container,
            animation: SyncMutex::new(ShulkerBoxAnimation {
                animation_status: AnimationStatus::Closed,
                progress: 0,
                old_progress: 0,
            }),
            open_count: AtomicI32::new(0),
            name: ContainerName::new(&translations::CONTAINER_SHULKER_BOX),
        }
    }

    fn update_animation(&self, world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        let mut animation = self.animation.lock();
        animation.old_progress = animation.progress;
        match animation.animation_status {
            AnimationStatus::Closed => animation.progress = 0,
            AnimationStatus::Opening => {
                animation.progress += 1;
                if animation.old_progress == 0 {
                    do_neighbor_updates(world, pos, state);
                }

                if animation.progress >= ANIMATION_STEPS {
                    animation.animation_status = AnimationStatus::Opened;
                    animation.progress = ANIMATION_STEPS;
                    do_neighbor_updates(world, pos, state);
                }

                drop(animation);
                self.move_collided_entities(world, pos, state);
            }
            AnimationStatus::Opened => animation.progress = ANIMATION_STEPS,
            AnimationStatus::Closing => {
                animation.progress = animation.progress.saturating_sub(1);
                if animation.old_progress == ANIMATION_STEPS {
                    do_neighbor_updates(world, pos, state);
                }

                if animation.progress == 0 {
                    animation.animation_status = AnimationStatus::Closed;
                    animation.progress = 0;
                    do_neighbor_updates(world, pos, state);
                }
            }
        }
    }

    fn move_collided_entities(&self, world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        let direction = state.get_value(&BlockStateProperties::FACING);

        let aabb: WorldAabb = {
            let animation = self.animation.lock();
            Self::get_progress_delta_aabb(
                1.0,
                direction,
                animation.old_progress(),
                animation.progress(),
                DVec3::from(pos.get_bottom_center()),
            )
        };

        let entities = world.get_entities_in_aabb(&aabb);
        for entity in entities {
            if entity.piston_push_reaction() == PushReaction::IgnoreEntity {
                continue;
            }
            let (offset_x, offset_y, offset_z) = direction.offset();
            entity.move_entity(
                MoverType::ShulkerBox,
                DVec3::new(
                    (aabb.width() + 0.01) * f64::from(offset_x),
                    (aabb.height() + 0.01) * f64::from(offset_y),
                    (aabb.depth() + 0.01) * f64::from(offset_z),
                ),
            );
        }
    }

    /// Builds a shulker box item containing the stored inventory.
    pub fn shulker_box_as_item(&self, state: BlockStateId) -> ItemStack {
        let mut item = ItemStack::new(REGISTRY.items.by_block(state.get_block()));
        item.apply_components(&self.collect_components());
        item
    }

    /// Calculates the bounding box of a shulker during its opening animation.
    ///
    /// The bounds are expanded in the opening direction based on the animation progress.
    #[must_use]
    pub fn get_progress_delta_aabb<I: Space>(
        size: f32,
        direction: Direction,
        progress_from: f32,
        progress_to: f32,
        position: DVec3,
    ) -> Aabb<DVec3, I> {
        let size = f64::from(size);
        let bounds =
            Aabb::<DVec3, I>::new(-size * 0.5, 0.0, -size * 0.5, size * 0.5, size, size * 0.5);

        let max_movement = f64::from(progress_from.max(progress_to));
        let min_movement = f64::from(progress_from.min(progress_to));

        let dir = DVec3::from(direction.offset_vec());

        bounds
            .expand_towards(dir * max_movement * size)
            .contract(-dir * (1.0 + min_movement) * size)
            .translate(position)
    }

    /// Block-local bounds of the box including the lid at its current progress.
    #[must_use]
    pub fn get_bounding_box(&self, state: BlockStateId) -> BlockLocalAabb {
        let bottom_center = DVec3::new(0.5, 0.0, 0.5);
        Self::get_progress_delta_aabb(
            1.0,
            state.get_value(ShulkerBoxBlock::FACING),
            -1.0,
            0.5 * self.progress(1.0),
            bottom_center,
        )
    }

    /// Returns whether the shulker box can open
    #[must_use]
    pub fn can_open(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) -> bool {
        if !matches!(self.animation_status(), AnimationStatus::Closed) {
            return true;
        }

        let direction = state.get_value(ShulkerBoxBlock::FACING);

        let lid_open_bounding_box: WorldAabb = Self::get_progress_delta_aabb(
            1.0,
            direction,
            0.0,
            0.5,
            DVec3::from(pos.get_bottom_center()),
        )
        .deflate(1.0E-6);

        let collision = WorldCollisionProvider::new(world);
        !collision.has_block_collision(&lid_open_bounding_box)
    }

    /// Vanilla `RandomizableContainer.unpackLootTable`: rolls a pending loot
    /// table into the box, with `player`'s luck.
    pub fn unpack_loot_table(&self, player: Option<&Player>) {
        self.container_ref.unpack_loot_table(player);
    }

    /// Checks if the block entity's container has any items
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.container.lock().is_empty()
    }

    /// The shulker box's items as vanilla `ItemContainerContents.fromItems`.
    #[must_use]
    pub fn container_contents(&self) -> ItemContainerContents {
        contents_from_items(&self.container.lock().items)
    }

    /// Interpolated progress for smooth rendering between ticks.
    #[must_use]
    pub fn progress(&self, partial_tick: f32) -> f32 {
        let animation = self.animation.lock();
        let new = animation.progress();
        let old = animation.old_progress();
        old + partial_tick * (new - old)
    }

    /// Get the current animation state
    #[must_use]
    pub fn animation_status(&self) -> AnimationStatus {
        self.animation.lock().animation_status
    }
}

impl BlockEntity for ShulkerBoxBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn tick(&self, world: &Arc<World>) {
        self.update_animation(world, self.base.pos, self.base.block_state());
    }

    fn trigger_event(&self, kind: i32, data: i32) -> bool {
        if kind == EVENT_SET_OPEN_COUNT {
            self.open_count.store(data, Ordering::Relaxed);
            if data == 0 {
                self.animation.lock().animation_status = AnimationStatus::Closing;
            }

            if data == 1 {
                self.animation.lock().animation_status = AnimationStatus::Opening;
            }

            true
        } else {
            false
        }
    }

    fn load_additional(&self, nbt: &BaseNbtCompound<'_>) {
        let nbt_view: NbtCompoundView<'_, '_> = nbt.into();
        self.name.load(nbt_view);
        let mut container = self.container.lock();
        container.items.fill(ItemStack::empty());

        if self.loot.load(nbt_view) {
            return;
        }
        let Some(items_list) = nbt_view.list("Items") else {
            return;
        };
        let Some(compounds) = items_list.compounds() else {
            return;
        };

        for compound in compounds {
            let Some(slot) = compound.byte("Slot") else {
                continue;
            };
            let slot = slot as usize;
            if slot >= SHULKER_BOX_SLOTS {
                continue;
            }
            if let Some(item) = ItemStack::from_borrowed_compound(&compound) {
                container.items[slot] = item;
            }
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.name.save(nbt);
        if self.loot.save(nbt) {
            return;
        }
        let container = self.container.lock();
        let mut items: Vec<NbtCompound> = Vec::new();
        for (slot, item) in container.items().iter().enumerate() {
            if item.is_empty() {
                continue;
            }
            let NbtTag::Compound(mut item_nbt) = item.clone().to_nbt_tag() else {
                continue;
            };
            item_nbt.insert("Slot", slot as i8);
            items.push(item_nbt);
        }
        nbt.insert("Items", NbtList::Compound(items));
    }

    /// Copies the container component into the block entity inventory
    fn container_name(&self) -> Option<&ContainerName> {
        Some(&self.name)
    }

    fn apply_implicit_components(&self, components: &mut ImplicitComponentGetter<'_>) {
        self.name.apply_implicit_components(components);
        apply_container_component(components, &mut self.container.lock().items);
        apply_container_loot_component(components, &self.loot);
    }

    /// Stores the block entity inventory in the container component
    fn collect_implicit_components(&self, components: &mut DataComponentMap) {
        self.name.collect_implicit_components(components);
        components.set(CONTAINER, Some(self.container_contents()));
        collect_container_loot_component(components, &self.loot);
    }

    fn container_ref(&self) -> Option<ContainerRef> {
        Some(self.container_ref.clone())
    }

    /// Vanilla `ShulkerBoxBlockEntity.startOpen`: every viewer sends the count
    /// in block event 1; the first also sounds and emits `container_open`.
    fn start_open(&self, player: &Player) {
        if self.base.is_removed() || player.is_spectator() {
            return;
        }
        let Some(world) = self.get_level() else {
            return;
        };
        let pos = self.base.pos;
        let open_count = {
            let previous = self.open_count.load(Ordering::Relaxed).max(0);
            let open_count = previous + 1;
            self.open_count.store(open_count, Ordering::Relaxed);
            open_count
        };
        world.block_event(
            pos,
            self.base.block_state().get_block(),
            EVENT_SET_OPEN_COUNT,
            open_count,
        );
        if open_count == 1 {
            world.game_event(
                &vanilla_game_events::CONTAINER_OPEN,
                pos,
                &GameEventContext::new(Some(player), None),
            );
            world.play_sound(
                &sound_events::BLOCK_SHULKER_BOX_OPEN,
                SoundSource::Blocks,
                pos,
                0.5,
                container_sound_pitch(),
                None,
            );
        }
    }

    /// Vanilla `ShulkerBoxBlockEntity.stopOpen`.
    fn stop_open(&self, player: &Player) {
        if self.base.is_removed() || player.is_spectator() {
            return;
        }
        let Some(world) = self.get_level() else {
            return;
        };
        let pos = self.base.pos;
        let open_count = self.open_count.fetch_sub(1, Ordering::Relaxed) - 1;
        world.block_event(
            pos,
            self.base.block_state().get_block(),
            EVENT_SET_OPEN_COUNT,
            open_count,
        );
        if open_count <= 0 {
            world.game_event(
                &vanilla_game_events::CONTAINER_CLOSE,
                pos,
                &GameEventContext::new(Some(player), None),
            );
            world.play_sound(
                &sound_events::BLOCK_SHULKER_BOX_CLOSE,
                SoundSource::Blocks,
                pos,
                0.5,
                container_sound_pitch(),
                None,
            );
        }
    }
}

impl Container for ShulkerBoxContainer {
    fn items(&self) -> &[ItemStack] {
        &self.items
    }

    fn items_mut(&mut self) -> &mut [ItemStack] {
        &mut self.items
    }

    fn get_container_size(&self) -> usize {
        SHULKER_BOX_SLOTS
    }

    fn set_item(&mut self, slot: usize, mut stack: ItemStack) {
        if slot < SHULKER_BOX_SLOTS {
            let max_stack_size = self.get_max_stack_size_for_item(&stack);
            if !stack.is_empty() && stack.count() > max_stack_size {
                stack.set_count(max_stack_size);
            }
            self.items[slot] = stack;
        }
    }

    fn get_max_stack_size(&self) -> i32 {
        64
    }

    /// Vanilla `ShulkerBoxBlockEntity.canPlaceItemThroughFace`: hoppers and
    /// droppers cannot insert another shulker box.
    fn can_place_item_through_face(
        &self,
        slot: usize,
        stack: &ItemStack,
        _direction: Direction,
    ) -> bool {
        self.can_place_item(slot, stack)
            && ITEM_BEHAVIORS
                .get_behavior(stack.item())
                .can_fit_inside_container_items()
    }

    fn set_changed(&mut self) {}
}

#[cfg(test)]
mod tests {
    use simdnbt::owned::NbtCompound;
    use steel_registry::{
        data_components::vanilla_components::CONTAINER_LOOT, init_vanilla_registry, vanilla_blocks,
    };
    use steel_utils::{BlockPos, ChunkPos, Downcast as _, types::UpdateFlags};

    use super::ShulkerBoxBlockEntity;
    use crate::{
        behavior::init_behaviors,
        block_entity::init_block_entities,
        test_support::{fresh_test_world, insert_ready_full_chunk},
    };

    #[test]
    fn shulker_box_keeps_and_rolls_vanilla_loot_tables() {
        init_vanilla_registry();
        init_behaviors();
        init_block_entities();
        let fixture = fresh_test_world("shulker_box_loot");
        let world = &fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let pos = BlockPos::new(1, 64, 1);
        assert!(world.set_block(
            pos,
            vanilla_blocks::SHULKER_BOX.default_state(),
            UpdateFlags::UPDATE_NONE,
        ));
        let block_entity = world
            .get_block_entity(pos)
            .expect("a placed shulker box has a block entity");
        let mut nbt = NbtCompound::new();
        nbt.insert("LootTable", "minecraft:chests/simple_dungeon");
        nbt.insert("LootTableSeed", 7_i64);
        block_entity
            .load_with_owned_components(&nbt)
            .expect("test NBT must load");
        let shulker_box = block_entity
            .downcast_ref::<ShulkerBoxBlockEntity>()
            .expect("the block entity is a shulker box");

        // The pending table survives a save and travels on the dropped item.
        let saved = block_entity.save_without_metadata();
        assert_eq!(
            saved.string("LootTable").map(ToString::to_string),
            Some("minecraft:chests/simple_dungeon".to_owned())
        );
        assert_eq!(saved.long("LootTableSeed"), Some(7));
        assert!(saved.list("Items").is_none());
        assert!(shulker_box.is_empty());
        let item = shulker_box.shulker_box_as_item(vanilla_blocks::SHULKER_BOX.default_state());
        assert!(item.get(CONTAINER_LOOT).is_some());

        // Breaking the box rolls the table into it first.
        shulker_box.unpack_loot_table(None);
        assert!(!shulker_box.is_empty());
        let saved = block_entity.save_without_metadata();
        assert!(saved.string("LootTable").is_none());
        assert!(saved.list("Items").is_some());
    }
}
