//! Vanilla `EndCrystal`: the healing crystal on top of the End spikes, which
//! explodes when hit.

use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::{AtomicI32, Ordering};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_macros::entity_behavior;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::vanilla_damage_type_tags::DamageTypeTag;
use steel_registry::vanilla_entity_data::EndCrystalEntityData;
use steel_registry::{vanilla_damage_types, vanilla_entities, vanilla_game_events};
use steel_utils::{BlockPos, locks::SyncMutex};
use steel_utils::{DowncastType, DowncastTypeKey};

use crate::entity::damage::DamageSource;
use crate::entity::{Entity, EntityBase, EntityBaseLoad, EntityMovementEmission, EntitySyncedData};
use crate::entity::{RemovalReason, SharedEntity};
use crate::world::World;
use crate::world::explosion::{ExplosionInteraction, ExplosionOptions, explosion_damage_source};

/// Vanilla's power of the blast a hit crystal sets off.
const EXPLOSION_POWER: f32 = 6.0;
/// Vanilla seeds `time` with `random.nextInt(100000)`.
const INITIAL_TIME_BOUND: i32 = 100_000;

/// An End crystal.
#[entity_behavior(class = "EndCrystal")]
pub struct EndCrystalEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<EndCrystalEntityData>,
    /// Vanilla `time`: the client spin and bob counter, started at a random
    /// point.
    time: AtomicI32,
}

// SAFETY: This key is owned by Steel and uniquely identifies `EndCrystalEntity`.
unsafe impl DowncastType for EndCrystalEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/end_crystal");
}

impl EndCrystalEntity {
    /// Creates a new End Crystal entity.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(EndCrystalEntityData::new()),
            time: AtomicI32::new(rand::random_range(0..INITIAL_TIME_BOUND)),
        }
    }

    /// Creates an End Crystal entity from saved data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(EndCrystalEntityData::new()),
            time: AtomicI32::new(rand::random_range(0..INITIAL_TIME_BOUND)),
        }
    }

    /// Vanilla `time`.
    #[must_use]
    pub fn time(&self) -> i32 {
        self.time.load(Ordering::Relaxed)
    }

    /// Vanilla `onDestroyedBy`: tells the dragon fight a crystal is gone.
    // TODO(dragon-fight): call `EnderDragonFight.onCrystalDestroyed` once Steel
    // has the End dragon fight.
    const fn on_destroyed_by(_world: &World, _source: &DamageSource) {}

    /// Sets the optional beam target.
    pub fn set_beam_target(&self, target: Option<BlockPos>) {
        self.entity_data.lock().beam_target.set(target);
    }

    /// Returns the optional beam target.
    #[must_use]
    pub fn beam_target(&self) -> Option<BlockPos> {
        *self.entity_data.lock().beam_target.get()
    }

    /// Sets whether the crystal renders its bedrock base.
    pub fn set_show_bottom(&self, show_bottom: bool) {
        self.entity_data.lock().show_bottom.set(show_bottom);
    }

    /// Returns whether the crystal renders its bedrock base.
    #[must_use]
    pub fn shows_bottom(&self) -> bool {
        *self.entity_data.lock().show_bottom.get()
    }

    /// The explosion half of vanilla `EndCrystal.hurtServer`, fired after the
    /// crystal is removed.
    fn explode(&self, this: Option<SharedEntity>, source: &DamageSource) {
        let Some(world) = self.level() else {
            return;
        };
        let mut options = ExplosionOptions::new(
            self.position(),
            EXPLOSION_POWER,
            ExplosionInteraction::Block,
        );
        options.damage_source = source
            .causing_entity()
            .map(|cause| explosion_damage_source(this.clone(), Some(Arc::clone(cause))));
        options.source = this;
        world.explode(options);
    }

    const fn nbt_bool(value: bool) -> i8 {
        if value { 1 } else { 0 }
    }
}

impl Entity for EndCrystalEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn movement_emission(&self) -> EntityMovementEmission {
        EntityMovementEmission::None
    }

    /// Vanilla `EndCrystal.tick`.
    // TODO(dragon-fight): vanilla also keeps fire lit under the crystal while
    // the level has a dragon fight; Steel has no End dragon fight yet.
    fn tick(self: Arc<Self>) {
        self.time.fetch_add(1, Ordering::Relaxed);
        self.apply_effects_from_blocks();
        self.handle_portal();
    }

    /// Vanilla `EndCrystal.hurtServer`: any hit but the dragon's removes the
    /// crystal in a power 6 explosion, unless the hit was an explosion itself.
    fn hurt(&self, world: &World, source: &DamageSource, _amount: f32) -> bool {
        if self.is_invulnerable_to_base(source)
            || source
                .causing_entity()
                .is_some_and(|entity| entity.entity_type() == &vanilla_entities::ENDER_DRAGON)
        {
            return false;
        }
        if !self.is_removed() {
            let this = world.get_entity_by_id(self.id());
            self.set_removed(RemovalReason::Killed);
            if !source.is(&DamageTypeTag::IS_EXPLOSION) {
                self.explode(this, source);
            }
            Self::on_destroyed_by(world, source);
        }
        true
    }

    /// Vanilla `EndCrystal.kill`.
    fn kill(&self, world: &World) {
        Self::on_destroyed_by(
            world,
            &DamageSource::environment(&vanilla_damage_types::GENERIC),
        );
        self.set_removed(RemovalReason::Killed);
        self.game_event(&vanilla_game_events::ENTITY_DIE);
    }

    fn is_pickable(&self) -> bool {
        true
    }

    fn blocks_building(&self) -> bool {
        true
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        if let Some(target) = self.beam_target() {
            nbt.insert(
                "beam_target",
                NbtTag::IntArray(vec![target.x(), target.y(), target.z()]),
            );
        }

        nbt.insert("ShowBottom", Self::nbt_bool(self.shows_bottom()));
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        if let Some(target) = nbt.int_array("beam_target")
            && target.len() == 3
        {
            self.set_beam_target(Some(BlockPos::new(target[0], target[1], target[2])));
        }

        if let Some(show_bottom) = nbt.byte("ShowBottom") {
            self.set_show_bottom(show_bottom != 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::inventory::container::Container as _;
    use steel_registry::blocks::block_state_ext::BlockStateExt as _;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_items};
    use steel_utils::ChunkPos;
    use steel_utils::types::{InteractionHand, UpdateFlags};
    use steel_utils::{Direction, Downcast as _, WorldAabb};

    use crate::behavior::{
        BlockHitResult, ITEM_BEHAVIORS, InteractionResult, UseOnContext, init_behaviors,
    };
    use crate::entity::{init_entities, next_entity_id};
    use crate::test_support::{
        TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk,
    };

    const AREA: WorldAabb = WorldAabb::new(0.0, 60.0, 0.0, 16.0, 80.0, 16.0);

    fn world_with_floor(name: &'static str) -> TestWorld {
        init_vanilla_registry();
        init_behaviors();
        init_entities();
        let world_fixture = fresh_test_world(name);
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let stone = vanilla_blocks::STONE.default_state();
        for x in 0..16 {
            for z in 0..16 {
                world.set_block(BlockPos::new(x, 64, z), stone, UpdateFlags::UPDATE_ALL);
            }
        }
        world_fixture
    }

    fn add_crystal(world: &Arc<World>) -> Arc<EndCrystalEntity> {
        let crystal = Arc::new(EndCrystalEntity::new(
            &vanilla_entities::END_CRYSTAL,
            next_entity_id(),
            DVec3::new(8.5, 65.0, 8.5),
            Arc::downgrade(world),
        ));
        let entity: SharedEntity = Arc::<EndCrystalEntity>::clone(&crystal);
        world
            .try_add_entity(entity)
            .expect("the crystal attaches to the loaded chunk");
        crystal
    }

    fn crystals(world: &World) -> Vec<SharedEntity> {
        world
            .get_entities_in_aabb(&AREA)
            .into_iter()
            .filter(|entity| entity.entity_type().key == vanilla_entities::END_CRYSTAL.key)
            .collect()
    }

    #[test]
    fn a_hit_crystal_explodes_and_breaks_blocks() {
        let world_fixture = world_with_floor("end_crystal_hit");
        let world = &world_fixture.world;
        let crystal = add_crystal(world);

        assert!(crystal.hurt(
            world,
            &DamageSource::environment(&vanilla_damage_types::GENERIC),
            1.0
        ));
        assert!(crystal.is_removed());
        assert!(
            world.get_block_state(BlockPos::new(8, 64, 8)).is_air(),
            "the power 6 blast breaks the floor under the crystal"
        );
    }

    #[test]
    fn an_explosion_removes_a_crystal_without_a_second_blast() {
        let world_fixture = world_with_floor("end_crystal_chain");
        let world = &world_fixture.world;
        let crystal = add_crystal(world);

        assert!(crystal.hurt(
            world,
            &DamageSource::environment(&vanilla_damage_types::EXPLOSION),
            1.0
        ));
        assert!(crystal.is_removed());
        assert!(!world.get_block_state(BlockPos::new(8, 64, 8)).is_air());
    }

    fn use_crystal_item(world_fixture: &TestWorld, pos: BlockPos) -> InteractionResult {
        let world = &world_fixture.world;
        let player = TestPlayerBuilder::new(Arc::clone(world), "Placer", next_entity_id()).build();
        player
            .inventory
            .lock()
            .set_item(0, ItemStack::with_count(&vanilla_items::END_CRYSTAL, 2));
        let mut context = UseOnContext::new(
            &player,
            InteractionHand::MainHand,
            BlockHitResult {
                block_pos: pos,
                direction: Direction::Up,
                location: DVec3::new(8.5, 65.0, 8.5),
                miss: false,
                inside: false,
                world_border_hit: false,
            },
            world,
            Arc::clone(&player.inventory),
        );
        ITEM_BEHAVIORS
            .get_behavior(&vanilla_items::END_CRYSTAL)
            .use_on(&mut context)
    }

    #[test]
    fn the_item_places_a_crystal_only_on_free_obsidian_or_bedrock() {
        let world_fixture = world_with_floor("end_crystal_item");
        let world = &world_fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        assert_eq!(
            use_crystal_item(&world_fixture, pos),
            InteractionResult::Fail,
            "stone does not hold a crystal"
        );

        world.set_block(
            pos,
            vanilla_blocks::OBSIDIAN.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        assert_eq!(
            use_crystal_item(&world_fixture, pos),
            InteractionResult::Success
        );
        let placed = crystals(world);
        assert_eq!(placed.len(), 1);
        let crystal = placed[0]
            .downcast_ref::<EndCrystalEntity>()
            .expect("an End crystal");
        assert!(!crystal.shows_bottom());
        assert_eq!(crystal.position(), DVec3::new(8.5, 65.0, 8.5));

        assert_eq!(
            use_crystal_item(&world_fixture, pos),
            InteractionResult::Fail,
            "a crystal already stands there"
        );
        assert_eq!(crystals(world).len(), 1);
    }

    #[test]
    fn end_crystal_does_not_duplicate_shared_invulnerable_state() {
        let crystal = EndCrystalEntity::new(
            &vanilla_entities::END_CRYSTAL,
            1,
            DVec3::new(1.5, 2.5, 3.5),
            Weak::new(),
        );
        crystal.set_invulnerable(true);

        let mut nbt = NbtCompound::new();
        crystal.save_additional(&mut nbt);

        assert_eq!(nbt.byte("Invulnerable"), None);
    }

    #[test]
    fn end_crystal_is_pickable_like_vanilla() {
        let crystal = EndCrystalEntity::new(
            &vanilla_entities::END_CRYSTAL,
            1,
            DVec3::new(1.5, 2.5, 3.5),
            Weak::new(),
        );

        assert!(crystal.is_pickable());
    }

    #[test]
    fn end_crystal_blocks_building_like_vanilla() {
        let crystal =
            EndCrystalEntity::new(&vanilla_entities::END_CRYSTAL, 1, DVec3::ZERO, Weak::new());

        assert!(crystal.blocks_building());
    }
}
