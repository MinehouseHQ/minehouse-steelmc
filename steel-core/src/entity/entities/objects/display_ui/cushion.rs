//! Vanilla `Cushion`: a block-attached seat players place on top of blocks.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::data_components::vanilla_components::{CUSHION_COLOR, CUSTOM_NAME};
use steel_registry::dye_color::DyeColor;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::particle_type::{BlockParticleOption, ParticleData};
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_data::CushionEntityData;
use steel_registry::vanilla_game_rules::ENTITY_DROPS;
use steel_registry::{
    REGISTRY, RegistryExt as _, sound_events, vanilla_blocks, vanilla_damage_types,
    vanilla_entities, vanilla_items, vanilla_particle_types,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, DowncastType, DowncastTypeKey, Identifier, WorldAabb};

use crate::behavior::{BlockCollisionContext, FLUID_BEHAVIORS, InteractionResult};
use crate::entity::block_attached_entity::{BlockAttachedEntity, BlockAttachedEntityBase};
use crate::entity::damage::DamageSource;
use crate::entity::entities::LightningBoltEntity;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityMoveError, EntitySyncedData,
    InsideBlockEffectCollector, SharedEntity, collided_with_fluid,
};
use crate::physics::{CollisionWorld as _, MoveResult, MoverType, WorldCollisionProvider};
use crate::player::Player;
use crate::world::{Explosion, World};

/// Vanilla `Cushion.LIGHTNING_DROP_INVULNERABLE_TICKS`.
const LIGHTNING_DROP_INVULNERABLE_TICKS: i32 = 20;

/// Vanilla `Cushion.hasAnchorBelow` and `isAnchorBuried` slice thickness (1/64 of a block).
const ANCHOR_SLICE: f64 = 0.015_625;

/// How far below the cushion vanilla `Cushion.hasAnchorBelow` searches for anchor blocks.
const ANCHOR_SEARCH_DEPTH: f64 = 0.125;

/// Vanilla `Cushion`.
#[entity_behavior(class = "Cushion")]
pub struct CushionEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<CushionEntityData>,
    block_attached_entity_base: BlockAttachedEntityBase,
}

// SAFETY: This key is owned by Steel and uniquely identifies `CushionEntity`.
unsafe impl DowncastType for CushionEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/cushion");
}

impl CushionEntity {
    /// Vanilla `Cushion(EntityType, Level)`, placed where the generic entity factory puts it.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(CushionEntityData::new()),
            block_attached_entity_base: BlockAttachedEntityBase::new(BlockPos::containing(
                position.x, position.y, position.z,
            )),
        }
    }

    /// Creates a cushion from persistent entity data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        let position = load.position;
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(CushionEntityData::new()),
            block_attached_entity_base: BlockAttachedEntityBase::new(BlockPos::containing(
                position.x, position.y, position.z,
            )),
        }
    }

    /// Vanilla `Cushion.getColor`.
    #[must_use]
    pub fn color(&self) -> DyeColor {
        *self.entity_data.lock().color.get()
    }

    /// Vanilla `Cushion.setColor`.
    pub fn set_color(&self, color: DyeColor) {
        self.entity_data.lock().color.set(color);
    }

    /// Vanilla `Items.CUSHION.pick(color)`: the cushion item of `color`.
    #[must_use]
    pub fn cushion_item(color: DyeColor) -> ItemRef {
        REGISTRY
            .items
            .by_key(&Identifier::vanilla(format!(
                "{}_cushion",
                color.serialized_name()
            )))
            .unwrap_or(&vanilla_items::WHITE_CUSHION)
    }

    /// Vanilla `Blocks.WOOL.pick(color)`: the wool block of `color`.
    fn wool_block(color: DyeColor) -> BlockRef {
        REGISTRY
            .blocks
            .by_key(&Identifier::vanilla(format!(
                "{}_wool",
                color.serialized_name()
            )))
            .unwrap_or(&vanilla_blocks::WHITE_WOOL)
    }

    /// Vanilla `Cushion.destroyIfInFire`: a cushion touching fire burns up.
    pub fn destroy_if_in_fire(&self, world: &World) {
        if self.is_removed() {
            return;
        }
        let touches_fire = blocks_in(next_deflated(self.bounding_box())).any(|pos| {
            world
                .get_block_state(pos)
                .get_block()
                .has_tag(&BlockTag::FIRE)
        });
        if touches_fire {
            self.hurt(
                world,
                &DamageSource::environment(&vanilla_damage_types::IN_FIRE),
                1.0,
            );
        }
    }

    /// Vanilla `Cushion.canBePlacedAt`.
    #[must_use]
    pub fn can_be_placed_at(world: &Arc<World>, bounding_box: WorldAabb) -> bool {
        Self::would_survive_at(world, bounding_box) && !is_anchor_buried(world, bounding_box)
    }

    /// Vanilla `Cushion.wouldSurviveAt`.
    #[must_use]
    pub fn would_survive_at(world: &World, bounding_box: WorldAabb) -> bool {
        has_anchor_below(world, bounding_box)
            && !is_covered_by_suffocating_blocks(world, bounding_box)
    }

    /// Vanilla `Cushion.showBreakingParticles`: wool splinters of the cushion's color.
    fn show_breaking_particles(&self) {
        let Some(world) = self.level() else {
            return;
        };
        let dimensions = self.base.dimensions();
        let position = self.position();
        let width = f64::from(dimensions.width / 4.0);
        world.send_particles(
            ParticleData::new(
                &vanilla_particle_types::BLOCK,
                BlockParticleOption::new(Self::wool_block(self.color()).default_state()),
            ),
            DVec3::new(
                position.x,
                position.y + f64::from(dimensions.height) * (2.0 / 3.0),
                position.z,
            ),
            10,
            DVec3::new(width, f64::from(dimensions.height / 4.0), width),
            0.05,
        );
    }

    /// Vanilla `Cushion.isBreakingDeniedFor` and `isBreakingDeniedAtPosFor`.
    fn is_breaking_denied_for(&self, world: &World, source: &DamageSource) -> bool {
        source
            .causing_entity()
            .and_then(|entity| entity.as_player())
            .is_some_and(|player| {
                !player.abilities.lock().may_build
                    || !world.may_interact(player, self.block_attached_entity_base.pos())
            })
    }

    /// Vanilla `Cushion.getCushionItemStackWithData`.
    fn cushion_item_stack_with_data(&self) -> ItemStack {
        let mut item = ItemStack::new(Self::cushion_item(self.color()));
        if let Some(name) = self.custom_name() {
            item.set(CUSTOM_NAME, name);
        }
        item
    }
}

/// Vanilla `AABB.nextDeflated`: the box shrunk by one ulp on every side.
const fn next_deflated(aabb: WorldAabb) -> WorldAabb {
    WorldAabb::new(
        aabb.min_x().next_up(),
        aabb.min_y().next_up(),
        aabb.min_z().next_up(),
        aabb.max_x().next_down(),
        aabb.max_y().next_down(),
        aabb.max_z().next_down(),
    )
}

/// Vanilla `LevelReader.findBlocksIn(AABB)` and `BlockPos.betweenClosed(AABB)`:
/// every block between the cells containing the box corners.
fn blocks_in(aabb: WorldAabb) -> impl Iterator<Item = BlockPos> {
    BlockPos::between_closed(
        BlockPos::containing(aabb.min_x(), aabb.min_y(), aabb.min_z()),
        BlockPos::containing(aabb.max_x(), aabb.max_y(), aabb.max_z()),
    )
}

/// Vanilla `Cushion.hasAnchorBelow`: some block's outline reaches the 1/64 slice under the cushion.
fn has_anchor_below(world: &World, bounding_box: WorldAabb) -> bool {
    let anchor_box = WorldAabb::new(
        bounding_box.min_x(),
        bounding_box.min_y() - ANCHOR_SLICE,
        bounding_box.min_z(),
        bounding_box.max_x().next_down(),
        bounding_box.min_y(),
        bounding_box.max_z().next_down(),
    );
    blocks_in(anchor_box.expand_towards(DVec3::new(0.0, -ANCHOR_SEARCH_DEPTH, 0.0))).any(|pos| {
        world
            .get_block_state(pos)
            .get_outline_shape_at(pos)
            .bounds()
            .is_some_and(|bounds| bounds.at_block(pos).intersects(anchor_box))
    })
}

/// Vanilla `Cushion.isAnchorBuried`: block collisions fill the whole 1/64 slice the cushion rests on.
fn is_anchor_buried(world: &Arc<World>, bounding_box: WorldAabb) -> bool {
    let resting_slice = next_deflated(WorldAabb::new(
        bounding_box.min_x(),
        bounding_box.min_y(),
        bounding_box.min_z(),
        bounding_box.max_x(),
        bounding_box.min_y() + ANCHOR_SLICE,
        bounding_box.max_z(),
    ));
    let colliders = WorldCollisionProvider::new(world)
        .get_block_collisions_with_context(&resting_slice, BlockCollisionContext::empty());
    boxes_cover(resting_slice, &colliders)
}

/// Whether the union of `boxes` covers all of `target`, the result of vanilla
/// joining `target` with every box using `BooleanOp.ONLY_FIRST` until it is empty.
fn boxes_cover(target: WorldAabb, boxes: &[WorldAabb]) -> bool {
    if boxes.is_empty() {
        return false;
    }
    let edges = |min: f64, max: f64, box_edges: &dyn Fn(&WorldAabb) -> (f64, f64)| {
        let mut edges = vec![min, max];
        for aabb in boxes {
            let (low, high) = box_edges(aabb);
            edges.extend(
                [low, high]
                    .into_iter()
                    .filter(|edge| *edge > min && *edge < max),
            );
        }
        edges.sort_by(f64::total_cmp);
        edges.dedup();
        edges
    };
    let xs = edges(target.min_x(), target.max_x(), &|aabb| {
        (aabb.min_x(), aabb.max_x())
    });
    let ys = edges(target.min_y(), target.max_y(), &|aabb| {
        (aabb.min_y(), aabb.max_y())
    });
    let zs = edges(target.min_z(), target.max_z(), &|aabb| {
        (aabb.min_z(), aabb.max_z())
    });
    let mid = |edges: &[f64]| -> Vec<f64> {
        edges
            .windows(2)
            .map(|pair| f64::midpoint(pair[0], pair[1]))
            .collect()
    };
    let (xs, ys, zs) = (mid(&xs), mid(&ys), mid(&zs));
    xs.iter().all(|&x| {
        ys.iter().all(|&y| {
            zs.iter().all(|&z| {
                boxes.iter().any(|aabb| {
                    aabb.min_x() < x
                        && x < aabb.max_x()
                        && aabb.min_y() < y
                        && y < aabb.max_y()
                        && aabb.min_z() < z
                        && z < aabb.max_z()
                })
            })
        })
    })
}

/// Vanilla `Cushion.isCoveredBySuffocatingBlocks`.
fn is_covered_by_suffocating_blocks(world: &World, bounding_box: WorldAabb) -> bool {
    blocks_in(next_deflated(bounding_box)).all(|pos| world.get_block_state(pos).is_suffocating())
}

impl BlockAttachedEntity for CushionEntity {
    fn block_attached_entity_base(&self) -> &BlockAttachedEntityBase {
        &self.block_attached_entity_base
    }

    /// Vanilla `Cushion.survives`.
    fn survives(&self) -> bool {
        self.level()
            .is_some_and(|world| Self::would_survive_at(&world, self.bounding_box()))
    }

    /// Vanilla `Cushion.dropItem`.
    fn drop_item(&self, caused_by: Option<&dyn Entity>) {
        self.play_sound(&sound_events::ENTITY_CUSHION_BREAK, 1.0, 1.0);
        self.show_breaking_particles();
        let Some(world) = self.level() else {
            return;
        };
        if !world.get_game_rule(&ENTITY_DROPS)
            || caused_by
                .and_then(Entity::as_player)
                .is_some_and(Player::has_infinite_materials)
        {
            return;
        }
        if let Some(item) = self.spawn_at_location(self.cushion_item_stack_with_data(), 0.0)
            && caused_by
                .is_some_and(|entity| entity.entity_type() == &vanilla_entities::LIGHTNING_BOLT)
        {
            item.set_invulnerable_time(LIGHTNING_DROP_INVULNERABLE_TICKS);
        }
    }

    /// Vanilla `Cushion.recalculateBoundingBox`: the plain entity box around its exact position.
    fn recalculate_bounding_box(&self) -> Result<(), EntityMoveError> {
        self.base
            .set_bounding_box(self.make_bounding_box_at(self.position()));
        Ok(())
    }

    /// Vanilla `Cushion.tickAtCheckInterval`: fluids act on it, and fire burns it.
    fn tick_at_check_interval(&self) {
        let Some(world) = self.level() else {
            return;
        };
        let block_pos = self.block_position();
        let fluid_state = world.get_block_state(block_pos).get_fluid_state();
        let position = self.position();
        if collided_with_fluid(&world, fluid_state, block_pos, position, position, self) {
            let mut effects = InsideBlockEffectCollector::new();
            FLUID_BEHAVIORS
                .get_behavior(fluid_state.fluid_id)
                .entity_inside(&world, block_pos, self, &mut effects);
            effects.apply_and_clear(self);
        }
        self.destroy_if_in_fire(&world);
    }
}

impl Entity for CushionEntity {
    /// Vanilla `Cushion.getPickResult`: the cushion of this color.
    fn pick_result(&self) -> Option<ItemStack> {
        Some(ItemStack::new(Self::cushion_item(self.color())))
    }

    /// Vanilla `Cushion.thunderHit`: lightning breaks the cushion.
    fn thunder_hit(&self, _world: &World, lightning: &LightningBoltEntity) {
        if !self.is_removed() {
            self.kill_attributed(Some(lightning));
            self.drop_item(Some(lightning));
        }
    }

    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn tick(self: Arc<Self>) {
        self.tick_block_attached_entity();
    }

    fn is_pickable(&self) -> bool {
        self.is_pickable_block_attached_entity()
    }

    fn ignore_explosion(&self, explosion: &dyn Explosion) -> bool {
        self.ignore_explosion_block_attached_entity(explosion)
    }

    /// Vanilla `Cushion.hurtServer`: players who may not build there cannot break it.
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        !self.is_breaking_denied_for(world, source)
            && self.hurt_block_attached_entity(world, source, amount)
    }

    fn kill(&self, _world: &World) {
        self.kill_attributed(None);
    }

    fn skip_attack_interaction(&self, source: &SharedEntity) -> bool {
        self.skip_attack_interaction_block_attached_entity(source)
    }

    fn push_impulse(&self, impulse: DVec3) {
        self.push_impulse_block_attached_entity(impulse);
    }

    fn move_entity(self: Arc<Self>, mover_type: MoverType, delta: DVec3) -> Option<MoveResult> {
        self.move_entity_block_attached_entity(mover_type, delta)
    }

    fn refresh_dimensions(&self) {
        self.refresh_dimensions_block_attached_entity();
    }

    /// Vanilla `Cushion.setPos`: keeps the exact position, then attaches to its block.
    fn try_set_position(&self, pos: DVec3) -> Result<(), EntityMoveError> {
        self.base.try_set_position(pos)?;
        self.try_set_position_block_attached_entity(pos)
    }

    /// Vanilla `Cushion.interact`: sit down unless sneaking or already taken.
    fn interact(
        &self,
        player: &Player,
        _hand: InteractionHand,
        _location: DVec3,
    ) -> InteractionResult {
        if player.is_secondary_use_active() || self.is_vehicle() {
            return InteractionResult::Pass;
        }
        let Some(cushion) = self
            .level()
            .and_then(|world| world.get_entity_by_id(self.id()))
        else {
            return InteractionResult::Consume;
        };
        if player.start_riding(&cushion, false) {
            self.play_sound(&sound_events::ENTITY_CUSHION_SIT, 1.0, 1.0);
            return InteractionResult::Success;
        }
        InteractionResult::Consume
    }

    /// Vanilla `Cushion.removePassenger`: the get-up sound while the cushion stays.
    fn on_passenger_removed(&self) {
        if !self.is_removed() {
            self.play_sound(&sound_events::ENTITY_CUSHION_GET_UP, 1.0, 1.0);
        }
    }

    /// Vanilla `Cushion.applyImplicitComponents`.
    fn apply_implicit_item_components(&self, item_stack: &ItemStack) {
        if let Some(color) = item_stack.get(CUSHION_COLOR) {
            self.set_color(*color);
        }
    }

    /// Vanilla `Cushion.addAdditionalSaveData`.
    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_block_attached_entity(nbt);
        nbt.insert("color", self.color().serialized_name());
    }

    /// Vanilla `Cushion.readAdditionalSaveData`.
    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_block_attached_entity(nbt);
        self.set_color(
            nbt.string("color")
                .and_then(|color| DyeColor::from_serialized_name(&color.to_str()))
                .unwrap_or(DyeColor::White),
        );
    }
}

#[cfg(test)]
mod tests;
