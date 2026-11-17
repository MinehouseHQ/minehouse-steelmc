//! What a dispenser does with the item it fires (Vanilla `DispenseItemBehavior`).

use std::mem;
use std::sync::Arc;

use glam::DVec3;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{Axis, BlockStateProperties, Direction};
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::fluid::FluidRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_game_rules::TNT_EXPLODES;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{REGISTRY, RegistryExt as _, TaggedRegistryExt as _, sound_events};
use steel_registry::{
    level_events, vanilla_blocks, vanilla_fluids, vanilla_game_events, vanilla_items,
};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, Downcast as _, WorldAabb};

use crate::behavior::blocks::{CarvedPumpkinBlock, FireBlock, TntBlock, WitherSkullBlock};
use crate::behavior::context::BlockPlaceContext;
use crate::behavior::items::{BlockItem, BoneMealItem, can_light, empty_contents};
use crate::behavior::projectile_item::{
    DispenseConfig, ProjectileItem, spawn_projectile_using_shoot,
};
use crate::behavior::{BLOCK_BEHAVIORS, ITEM_BEHAVIORS, pickup_waterlogged_block};
use crate::block_entity::entities::DispenserBlockEntity;
use crate::entity::Entity as _;
use crate::entity::entities::SulfurCubeEntity;
use crate::inventory::prelude::EquipmentSlot;
use crate::world::World;
use crate::world::game_event::GameEventContext;

mod block_interactions;
mod entity_placement;
mod vehicles;

/// Item speed factor of a dispenser (Vanilla passes 6 to `spawnItem`).
const DISPENSER_SPEED: f64 = 6.0;

/// The dispenser firing an item (Vanilla `BlockSource`).
pub(crate) struct BlockSource<'a> {
    pub world: &'a Arc<World>,
    pub pos: BlockPos,
    pub state: BlockStateId,
    pub entity: &'a DispenserBlockEntity,
}

impl BlockSource<'_> {
    pub(crate) fn facing(&self) -> Direction {
        self.state.get_value(&BlockStateProperties::FACING)
    }

    /// Vanilla `BlockSource.center`: the middle of the dispenser block.
    pub(crate) fn center(&self) -> DVec3 {
        DVec3::new(
            f64::from(self.pos.x()) + 0.5,
            f64::from(self.pos.y()) + 0.5,
            f64::from(self.pos.z()) + 0.5,
        )
    }

    /// The point items leave from: 0.7 blocks from the center towards `facing`.
    pub(crate) fn dispense_position(&self) -> DVec3 {
        let (step_x, step_y, step_z) = self.facing().offset();
        self.center() + 0.7 * DVec3::new(f64::from(step_x), f64::from(step_y), f64::from(step_z))
    }

    fn target(&self) -> BlockPos {
        self.pos.relative(self.facing())
    }
}

/// The behavior a dispenser uses for an item.
#[derive(Clone, Copy)]
pub(crate) enum DispenseBehavior {
    /// Drops one item (Vanilla `DefaultDispenseItemBehavior`).
    Default,
    /// Pours a water or lava bucket.
    FilledBucket(FluidRef),
    /// Pours a mob bucket and releases its mob.
    MobBucket,
    /// Fills an empty bucket from the fluid in front.
    EmptyBucket,
    /// Lights fire, campfires, candles or TNT.
    FlintAndSteel,
    /// Grows the crop or water plant in front.
    BoneMeal,
    /// Places a shulker box in front.
    ShulkerBox,
    /// Shears the entity in front.
    Shears,
    /// Completes a golem in front, or puts the pumpkin on an entity's head.
    CarvedPumpkin,
    /// Completes a wither in front, or puts the skull on an entity's head.
    WitherSkeletonSkull,
    /// Shoots the item as a projectile (Vanilla `ProjectileDispenseBehavior`).
    Projectile,
    /// Feeds a block to a sulfur cube in front, or drops it.
    SulfurCubeBlock,
    /// Feeds TNT to a sulfur cube in front, or primes it there.
    Tnt,
    /// Equips the item on the entity in front, or drops it (Vanilla
    /// `EquipmentDispenseItemBehavior`).
    Equipment,
    /// Spawns the egg's entity in front (Vanilla `SpawnEggItemBehavior`).
    SpawnEgg,
    /// Places an armor stand in front.
    ArmorStand,
    /// Puts a chest on a tamed donkey, mule or llama in front, or drops it.
    Chest,
    /// Launches a boat of this type onto the water in front, or drops it.
    Boat(EntityTypeRef),
    /// Puts a minecart of this type on the rail in front, or drops it.
    Minecart(EntityTypeRef),
    /// Places the powder snow in front, or drops the bucket.
    SolidBucket,
    /// Fills a bottle from a full hive or water in front.
    GlassBottle,
    /// Charges the respawn anchor in front.
    Glowstone,
    /// Brushes a scute off the armadillo in front.
    Brush,
    /// Waxes the copper block in front.
    Honeycomb,
    /// Turns the dirt in front into mud with a water bottle.
    Potion,
}

impl DispenseBehavior {
    /// The behavior Vanilla registers for `item` in `DispenserBlock.DISPENSER_REGISTRY`.
    pub(crate) fn for_item(item: &ItemStack) -> Self {
        if item.is(&vanilla_items::WATER_BUCKET) {
            Self::FilledBucket(&vanilla_fluids::WATER)
        } else if item.is(&vanilla_items::LAVA_BUCKET) {
            Self::FilledBucket(&vanilla_fluids::LAVA)
        } else if item.is(&vanilla_items::BUCKET) {
            Self::EmptyBucket
        } else if ITEM_BEHAVIORS
            .get_behavior(item.item)
            .as_mob_bucket()
            .is_some()
        {
            Self::MobBucket
        } else if item.is(&vanilla_items::FLINT_AND_STEEL) {
            Self::FlintAndSteel
        } else if item.is(&vanilla_items::BONE_MEAL) {
            Self::BoneMeal
        } else if item.is(&vanilla_items::SHEARS) {
            Self::Shears
        } else if item.is(&vanilla_items::CARVED_PUMPKIN) {
            Self::CarvedPumpkin
        } else if item.is(&vanilla_items::WITHER_SKELETON_SKULL) {
            Self::WitherSkeletonSkull
        } else if shulker_box_block(item).is_some() {
            Self::ShulkerBox
        } else if item.is(&vanilla_items::TNT) {
            Self::Tnt
        } else if item.is(&vanilla_items::ARMOR_STAND) {
            Self::ArmorStand
        } else if item.is(&vanilla_items::CHEST) {
            Self::Chest
        } else if item.is(&vanilla_items::POWDER_SNOW_BUCKET) {
            Self::SolidBucket
        } else if item.is(&vanilla_items::GLASS_BOTTLE) {
            Self::GlassBottle
        } else if item.is(&vanilla_items::GLOWSTONE) {
            Self::Glowstone
        } else if item.is(&vanilla_items::BRUSH) {
            Self::Brush
        } else if item.is(&vanilla_items::HONEYCOMB) {
            Self::Honeycomb
        } else if item.is(&vanilla_items::POTION) {
            Self::Potion
        } else if let Some(boat) = ITEM_BEHAVIORS.get_behavior(item.item).as_boat_item() {
            Self::Boat(boat.entity_type())
        } else if let Some(cart) = ITEM_BEHAVIORS.get_behavior(item.item).as_minecart_item() {
            Self::Minecart(cart.entity_type())
        } else if is_dispensed_projectile(item) {
            Self::Projectile
        } else if item.get_equippable().is_some() {
            Self::Equipment
        } else if REGISTRY
            .items
            .is_in_tag(item.item, &ItemTag::SULFUR_CUBE_SWALLOWABLE)
        {
            Self::SulfurCubeBlock
        } else if entity_placement::is_spawn_egg(item) {
            Self::SpawnEgg
        } else {
            Self::Default
        }
    }

    /// Fires `stack` and returns what stays in the slot.
    pub(crate) fn dispense(self, source: &BlockSource<'_>, stack: ItemStack) -> ItemStack {
        match self {
            Self::Default => default_dispense(source, stack),
            Self::Projectile => {
                let Some(projectile_item) =
                    ITEM_BEHAVIORS.get_behavior(stack.item).as_projectile_item()
                else {
                    return default_dispense(source, stack);
                };
                let config = projectile_item.create_dispense_config();
                let result = dispense_projectile(source, projectile_item, &config, stack);
                source.world.level_event(
                    config
                        .override_dispense_event
                        .unwrap_or(level_events::SOUND_DISPENSER_PROJECTILE_LAUNCH),
                    source.pos,
                    0,
                    None,
                );
                play_animation(source);
                result
            }
            Self::FilledBucket(_)
            | Self::MobBucket
            | Self::EmptyBucket
            | Self::Equipment
            | Self::Chest
            | Self::SpawnEgg
            | Self::ArmorStand
            | Self::Boat(_)
            | Self::Minecart(_)
            | Self::SolidBucket
            | Self::Potion
            | Self::SulfurCubeBlock => {
                let result = self.execute_dropping_on_failure(source, stack);
                play_default_sound_and_animation(source);
                result
            }
            Self::FlintAndSteel
            | Self::BoneMeal
            | Self::ShulkerBox
            | Self::Shears
            | Self::Tnt
            | Self::CarvedPumpkin
            | Self::WitherSkeletonSkull
            | Self::GlassBottle
            | Self::Glowstone
            | Self::Brush
            | Self::Honeycomb => {
                let (result, success) = match self {
                    Self::FlintAndSteel => flint_and_steel(source, stack),
                    Self::BoneMeal => bone_meal(source, stack),
                    Self::Shears => shears(source, stack),
                    Self::Tnt => tnt(source, stack),
                    Self::CarvedPumpkin => carved_pumpkin(source, stack),
                    Self::WitherSkeletonSkull => wither_skeleton_skull(source, stack),
                    Self::GlassBottle => block_interactions::glass_bottle(source, stack),
                    Self::Glowstone => block_interactions::glowstone(source, stack),
                    Self::Brush => block_interactions::brush(source, stack),
                    Self::Honeycomb => block_interactions::honeycomb(source, stack),
                    _ => shulker_box(source, stack),
                };
                source.world.level_event(
                    if success {
                        level_events::SOUND_DISPENSER_DISPENSE
                    } else {
                        level_events::SOUND_DISPENSER_FAIL
                    },
                    source.pos,
                    0,
                    None,
                );
                play_animation(source);
                result
            }
        }
    }

    /// The `execute` step of the behaviors that always play the plain dispense
    /// sound (`DefaultDispenseItemBehavior` subclasses).
    fn execute_dropping_on_failure(self, source: &BlockSource<'_>, stack: ItemStack) -> ItemStack {
        let mut stack = stack;
        let fitted = match self {
            Self::FilledBucket(fluid) => return filled_bucket(source, stack, fluid),
            Self::MobBucket => return mob_bucket(source, stack),
            Self::EmptyBucket => return empty_bucket(source, stack),
            Self::SpawnEgg => return entity_placement::spawn_egg(source, stack),
            Self::ArmorStand => return entity_placement::armor_stand(source, stack),
            Self::Boat(entity_type) => return vehicles::boat(source, stack, entity_type),
            Self::Minecart(entity_type) => return vehicles::minecart(source, stack, entity_type),
            Self::SolidBucket => return block_interactions::solid_bucket(source, stack),
            Self::Potion => return block_interactions::water_potion(source, stack),
            Self::Equipment => dispense_equipment(source, &mut stack),
            Self::Chest => entity_placement::chest_on_chested_horse(source, &mut stack),
            Self::SulfurCubeBlock => dispense_into_sulfur_cube(source, &mut stack),
            _ => false,
        };
        if fitted {
            stack
        } else {
            default_execute(source, stack)
        }
    }
}

/// Whether `DispenseItemBehavior.bootStrap` registers `item` with
/// `registerProjectileBehavior` (the trident is a `ProjectileItem` but is not
/// registered).
fn is_dispensed_projectile(item: &ItemStack) -> bool {
    let projectiles: [ItemRef; 13] = [
        &vanilla_items::ARROW,
        &vanilla_items::TIPPED_ARROW,
        &vanilla_items::SPECTRAL_ARROW,
        &vanilla_items::EGG,
        &vanilla_items::BLUE_EGG,
        &vanilla_items::BROWN_EGG,
        &vanilla_items::SNOWBALL,
        &vanilla_items::EXPERIENCE_BOTTLE,
        &vanilla_items::SPLASH_POTION,
        &vanilla_items::LINGERING_POTION,
        &vanilla_items::FIREWORK_ROCKET,
        &vanilla_items::FIRE_CHARGE,
        &vanilla_items::WIND_CHARGE,
    ];
    projectiles.iter().any(|projectile| item.is(projectile))
}

/// Vanilla `ProjectileDispenseBehavior.execute`.
fn dispense_projectile(
    source: &BlockSource<'_>,
    projectile_item: &dyn ProjectileItem,
    config: &DispenseConfig,
    mut stack: ItemStack,
) -> ItemStack {
    let direction = source.facing();
    let position = config.position.resolve(source.center(), direction);
    let projectile = projectile_item.as_projectile(source.world, position, &stack, direction);
    spawn_projectile_using_shoot(
        source.world,
        &projectile,
        &stack,
        direction.offset_vec().as_dvec3(),
        config.power,
        config.uncertainty,
    );
    stack.shrink(1);
    stack
}

/// Vanilla `DefaultDispenseItemBehavior.dispense`.
pub(crate) fn default_dispense(source: &BlockSource<'_>, mut stack: ItemStack) -> ItemStack {
    let one = stack.split(1);
    spawn_item(
        source.world,
        one,
        DISPENSER_SPEED,
        source.facing(),
        source.dispense_position(),
    );
    play_default_sound_and_animation(source);
    stack
}

fn play_default_sound_and_animation(source: &BlockSource<'_>) {
    source
        .world
        .level_event(level_events::SOUND_DISPENSER_DISPENSE, source.pos, 0, None);
    play_animation(source);
}

fn play_animation(source: &BlockSource<'_>) {
    source.world.level_event(
        level_events::PARTICLES_SHOOT_SMOKE,
        source.pos,
        source.facing().get_3d_data_value(),
        None,
    );
}

/// Vanilla `DefaultDispenseItemBehavior.spawnItem`.
pub(crate) fn spawn_item(
    world: &Arc<World>,
    stack: ItemStack,
    speed: f64,
    direction: Direction,
    position: DVec3,
) {
    let y_offset = if direction.axis() == Axis::Y {
        0.125
    } else {
        0.15625
    };
    let pos = DVec3::new(position.x, position.y - y_offset, position.z);
    let (step_x, _, step_z) = direction.offset();
    let power = rand::random::<f64>() * 0.1 + 0.2;
    let spread = 0.017_227_5 * speed;
    let velocity = DVec3::new(
        triangle(f64::from(step_x) * power, spread),
        triangle(0.2, spread),
        triangle(f64::from(step_z) * power, spread),
    );
    world.spawn_item_with_velocity(pos, stack, velocity);
}

/// Vanilla `RandomSource.triangle(mean, deviation)`.
fn triangle(mean: f64, deviation: f64) -> f64 {
    mean + deviation * (rand::random::<f64>() - rand::random::<f64>())
}

/// Vanilla `DefaultDispenseItemBehavior.consumeWithRemainder`.
fn consume_with_remainder(
    source: &BlockSource<'_>,
    mut stack: ItemStack,
    remainder: ItemStack,
) -> ItemStack {
    stack.shrink(1);
    if stack.is_empty() {
        return remainder;
    }
    let rest = source.entity.insert_item(remainder);
    if !rest.is_empty() {
        spawn_item(
            source.world,
            rest,
            DISPENSER_SPEED,
            source.facing(),
            source.dispense_position(),
        );
        play_default_sound_and_animation(source);
    }
    stack
}

fn filled_bucket(source: &BlockSource<'_>, stack: ItemStack, fluid: FluidRef) -> ItemStack {
    if empty_contents(source.world, fluid, source.target(), None, true) {
        return consume_with_remainder(source, stack, ItemStack::new(&vanilla_items::BUCKET));
    }
    default_dispense(source, stack)
}

fn mob_bucket(source: &BlockSource<'_>, stack: ItemStack) -> ItemStack {
    let Some(mob_bucket) = ITEM_BEHAVIORS.get_behavior(stack.item).as_mob_bucket() else {
        return default_dispense(source, stack);
    };
    let target = source.target();
    if mob_bucket.empty_contents(source.world, target, None, true) {
        mob_bucket.check_extra_content(source.world, None, &stack, target);
        return consume_with_remainder(source, stack, ItemStack::new(&vanilla_items::BUCKET));
    }
    default_dispense(source, stack)
}

fn empty_bucket(source: &BlockSource<'_>, stack: ItemStack) -> ItemStack {
    let pos = source.target();
    let state = source.world.get_block_state(pos);
    let behavior = BLOCK_BEHAVIORS.get_behavior(state.get_block());
    // TODO: Remove fallback once all waterloggable blocks implement pickup_block.
    let picked = behavior
        .pickup_block(source.world, pos, state, None)
        .or_else(|| pickup_waterlogged_block(behavior, source.world, pos, state, None));
    let Some(result) = picked.filter(|result| !result.filled_bucket.is_empty()) else {
        return default_execute(source, stack);
    };
    source.world.game_event(
        &vanilla_game_events::FLUID_PICKUP,
        pos,
        &GameEventContext::new(None, None),
    );
    let filled = ItemStack::new(result.filled_bucket.item());
    consume_with_remainder(source, stack, filled)
}

/// Vanilla `SulfurCubeBlockDispenseItemBehavior.dispenseBlock`: the first
/// sulfur cube in front that swallows the block takes one of it.
fn dispense_into_sulfur_cube(source: &BlockSource<'_>, stack: &mut ItemStack) -> bool {
    let target = source.target();
    let search_box = WorldAabb::new(
        f64::from(target.x()),
        f64::from(target.y()),
        f64::from(target.z()),
        f64::from(target.x() + 1),
        f64::from(target.y() + 1),
        f64::from(target.z() + 1),
    );
    let cubes = source
        .world
        .get_entities_in_aabb_matching(&search_box, |entity| {
            entity.downcast_ref::<SulfurCubeEntity>().is_some()
        });
    for entity in cubes {
        if let Some(cube) = entity.downcast_ref::<SulfurCubeEntity>()
            && cube.equip_item(stack)
        {
            stack.shrink(1);
            return true;
        }
    }
    false
}

/// Vanilla TNT dispense behavior: feeds a sulfur cube, otherwise primes TNT
/// in front of the dispenser.
fn tnt(source: &BlockSource<'_>, mut stack: ItemStack) -> (ItemStack, bool) {
    let world = source.world;
    if !world.get_game_rule(&TNT_EXPLODES) {
        return (stack, false);
    }
    if dispense_into_sulfur_cube(source, &mut stack) {
        return (stack, true);
    }
    let target = source.target();
    let primed = TntBlock::primed_entity(world, target, None);
    let position = primed.position();
    if let Err(error) = world.try_add_entity(primed) {
        log::debug!("failed to add dispensed primed TNT: {error}");
    }
    world.play_sound_at(
        &sound_events::ENTITY_TNT_PRIMED,
        SoundSource::Blocks,
        position,
        1.0,
        1.0,
        None,
    );
    world.game_event(
        &vanilla_game_events::ENTITY_PLACE,
        target,
        &GameEventContext::new(None, None),
    );
    stack.shrink(1);
    (stack, true)
}

/// `DefaultDispenseItemBehavior.execute`: the drop without sound and animation,
/// which the caller plays.
fn default_execute(source: &BlockSource<'_>, mut stack: ItemStack) -> ItemStack {
    let one = stack.split(1);
    spawn_item(
        source.world,
        one,
        DISPENSER_SPEED,
        source.facing(),
        source.dispense_position(),
    );
    stack
}

/// Vanilla `FlintAndSteelDispenseItemBehavior.execute`: success starts true
/// and is only cleared when TNT refuses to prime, so a blocked target still
/// wears the flint and steel down.
fn flint_and_steel(source: &BlockSource<'_>, mut stack: ItemStack) -> (ItemStack, bool) {
    let world = source.world;
    let direction = source.facing();
    let pos = source.target();
    let state = world.get_block_state(pos);
    let mut success = true;
    if try_ignite_explosive_entities(source) {
        // A sulfur cube in front takes the spark instead of the block.
    } else if FireBlock::can_be_placed_at(world, pos, direction) {
        world.set_block(
            pos,
            FireBlock::get_state(world.as_ref(), pos),
            UpdateFlags::UPDATE_ALL,
        );
        world.game_event(
            &vanilla_game_events::BLOCK_PLACE,
            pos,
            &GameEventContext::new(None, None),
        );
    } else if can_light(state) {
        world.set_block(
            pos,
            state.set_value(&BlockStateProperties::LIT, true),
            UpdateFlags::UPDATE_ALL,
        );
        world.game_event(
            &vanilla_game_events::BLOCK_CHANGE,
            pos,
            &GameEventContext::new(None, None),
        );
    } else if state.get_block() == &vanilla_blocks::TNT {
        if TntBlock::prime(world, pos) {
            world.remove_block(pos, false);
        } else {
            success = false;
        }
    }
    if success {
        stack.hurt_and_break(1, false);
    }
    (stack, success)
}

/// Vanilla `FlintAndSteelDispenseItemBehavior.tryIgniteExplosiveEntities`:
/// lights the first sulfur cube in front that can still explode.
fn try_ignite_explosive_entities(source: &BlockSource<'_>) -> bool {
    let target = source.target();
    let search_box = WorldAabb::new(
        f64::from(target.x()),
        f64::from(target.y()),
        f64::from(target.z()),
        f64::from(target.x() + 1),
        f64::from(target.y() + 1),
        f64::from(target.z() + 1),
    );
    let cubes = source
        .world
        .get_entities_in_aabb_matching(&search_box, |entity| {
            entity
                .downcast_ref::<SulfurCubeEntity>()
                .is_some_and(SulfurCubeEntity::can_explode)
        });
    let Some(cube) = cubes
        .first()
        .and_then(|entity| entity.downcast_ref::<SulfurCubeEntity>())
    else {
        return false;
    };
    cube.prime_time(false);
    true
}

fn bone_meal(source: &BlockSource<'_>, mut stack: ItemStack) -> (ItemStack, bool) {
    let pos = source.target();
    let success = BoneMealItem::grow(source.world, pos)
        || BoneMealItem::grow_water_plant(source.world, pos, None);
    if success {
        stack.shrink(1);
        source.world.level_event(
            level_events::PARTICLES_AND_SOUND_PLANT_GROWTH,
            pos,
            15,
            None,
        );
    }
    (stack, success)
}

/// Vanilla `ShearsDispenseItemBehavior`.
// TODO: Harvest full beehives once beehive honey harvesting exists.
fn shears(source: &BlockSource<'_>, mut stack: ItemStack) -> (ItemStack, bool) {
    let pos = source.target();
    let success = shear_entity(source.world, pos, &stack);
    if success {
        stack.hurt_and_break(1, false);
    }
    (stack, success)
}

fn shear_entity(world: &Arc<World>, pos: BlockPos, tool: &ItemStack) -> bool {
    let aabb = WorldAabb::new(
        f64::from(pos.x()),
        f64::from(pos.y()),
        f64::from(pos.z()),
        f64::from(pos.x() + 1),
        f64::from(pos.y() + 1),
        f64::from(pos.z() + 1),
    );
    for entity in world.get_entities_in_aabb(&aabb) {
        if entity.is_spectator() {
            continue;
        }
        if entity.shear_off_all_leash_connections(None) {
            return true;
        }
        if entity.is_alive()
            && let Some(shearable) = entity.as_shearable()
            && shearable.ready_for_shearing()
        {
            shearable.shear(world, SoundSource::Blocks, tool);
            world.game_event(
                &vanilla_game_events::SHEAR,
                pos,
                &GameEventContext::new(None, None),
            );
            return true;
        }
    }
    false
}

/// The shulker box block a shulker box item places, if `item` is one.
///
/// Vanilla registers `ShulkerBoxDispenseBehavior` for every `ShulkerBoxBlock` item;
/// those are the only block items that cannot go inside container items.
fn shulker_box_block(item: &ItemStack) -> Option<BlockRef> {
    if ITEM_BEHAVIORS
        .get_behavior(item.item())
        .can_fit_inside_container_items()
    {
        return None;
    }
    let block = REGISTRY.blocks.by_key(&item.item().key)?;
    (!BLOCK_BEHAVIORS
        .get_behavior(block)
        .fits_inside_container_items())
    .then_some(block)
}

/// Vanilla `ShulkerBoxDispenseBehavior.execute`.
fn shulker_box(source: &BlockSource<'_>, mut stack: ItemStack) -> (ItemStack, bool) {
    let Some(block) = shulker_box_block(&stack) else {
        return (stack, false);
    };
    let direction = source.facing();
    let pos = source.target();
    let against = if source.world.get_block_state(pos.below()).is_air() {
        direction
    } else {
        Direction::Up
    };
    let context = BlockPlaceContext::directional(source.world, pos, direction, &mut stack, against);
    let success = BlockItem::new(block).place(context).consumes_action();
    (stack, success)
}

/// Vanilla `DispenseItemBehavior.bootStrap` carved pumpkin behavior.
fn carved_pumpkin(source: &BlockSource<'_>, mut stack: ItemStack) -> (ItemStack, bool) {
    let world = source.world;
    let target = source.target();
    if world.get_block_state(target).is_air() && CarvedPumpkinBlock::can_spawn_golem(world, target)
    {
        world.set_block(
            target,
            vanilla_blocks::CARVED_PUMPKIN.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        world.game_event(
            &vanilla_game_events::BLOCK_PLACE,
            target,
            &GameEventContext::new(None, None),
        );
        stack.shrink(1);
        return (stack, true);
    }
    let success = dispense_equipment(source, &mut stack);
    (stack, success)
}

/// Vanilla `DispenseItemBehavior.bootStrap` wither skeleton skull behavior.
fn wither_skeleton_skull(source: &BlockSource<'_>, mut stack: ItemStack) -> (ItemStack, bool) {
    let world = source.world;
    let target = source.target();
    if world.get_block_state(target).is_air()
        && WitherSkullBlock::can_spawn_mob(world, target, &stack)
    {
        // Vanilla `RotationSegment.convertToSegment(direction)`: `toYRot` is
        // `(data2d & 3) * 90`.
        #[expect(clippy::cast_precision_loss, reason = "at most 270")]
        let y_rot = ((source.facing().get_2d_data_value() & 3) * 90) as f32;
        world.set_block(
            target,
            vanilla_blocks::WITHER_SKELETON_SKULL
                .default_state()
                .set_value(
                    &BlockStateProperties::ROTATION_16,
                    steel_math::convert_to_rotation_segment(y_rot),
                ),
            UpdateFlags::UPDATE_ALL,
        );
        world.game_event(
            &vanilla_game_events::BLOCK_PLACE,
            target,
            &GameEventContext::new(None, None),
        );
        WitherSkullBlock::check_spawn(world, target);
        stack.shrink(1);
        return (stack, true);
    }
    let success = dispense_equipment(source, &mut stack);
    (stack, success)
}

/// Vanilla `EquipmentDispenseItemBehavior.dispenseEquipment`: equips one item on
/// the first living entity in the block in front that accepts it.
pub(crate) fn dispense_equipment(source: &BlockSource<'_>, stack: &mut ItemStack) -> bool {
    let target = source.target();
    let search_box = WorldAabb::new(
        f64::from(target.x()),
        f64::from(target.y()),
        f64::from(target.z()),
        f64::from(target.x() + 1),
        f64::from(target.y() + 1),
        f64::from(target.z() + 1),
    );
    let candidates = source
        .world
        .get_entities_in_aabb_matching(&search_box, |entity| {
            entity
                .as_living_entity()
                .is_some_and(|living| living.can_equip_with_dispenser(stack))
        });
    let Some(living) = candidates
        .first()
        .and_then(|entity| entity.as_living_entity())
    else {
        return false;
    };

    let slot = match living.as_mob() {
        Some(mob) => mob.get_equipment_slot_for_item(stack),
        None => match stack.get_equippable() {
            Some(equippable) if living.can_use_slot(equippable.slot) => equippable.slot,
            _ => EquipmentSlot::MainHand,
        },
    };
    let equip = stack.split(1);
    let equipped = equip.copy_with_count(equip.count());
    let mut previous = ItemStack::empty();
    let mut equip = Some(equip);
    living.with_equipment_slot_mut(slot, &mut |item_stack| {
        if let Some(equip) = equip.take() {
            previous = mem::replace(item_stack, equip);
        }
    });
    living.on_equip_item(slot, &previous, &equipped);
    if let Some(mob) = living.as_mob() {
        mob.set_guaranteed_drop(slot);
        mob.set_persistence_required();
    }
    true
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_items};

    use super::*;
    use crate::behavior::init_behaviors;

    #[test]
    fn registered_projectiles_dispense_as_projectiles_but_tridents_drop() {
        init_vanilla_registry();
        init_behaviors();
        for item in [
            &vanilla_items::ARROW,
            &vanilla_items::SPECTRAL_ARROW,
            &vanilla_items::TIPPED_ARROW,
            &vanilla_items::BLUE_EGG,
            &vanilla_items::SNOWBALL,
            &vanilla_items::EXPERIENCE_BOTTLE,
            &vanilla_items::SPLASH_POTION,
            &vanilla_items::FIREWORK_ROCKET,
            &vanilla_items::FIRE_CHARGE,
            &vanilla_items::WIND_CHARGE,
        ] {
            let stack = ItemStack::new(item);
            assert!(
                matches!(
                    DispenseBehavior::for_item(&stack),
                    DispenseBehavior::Projectile
                ),
                "{item:?}"
            );
            assert!(
                ITEM_BEHAVIORS
                    .get_behavior(stack.item)
                    .as_projectile_item()
                    .is_some(),
                "{item:?}"
            );
        }

        let trident = ItemStack::new(&vanilla_items::TRIDENT);
        assert!(matches!(
            DispenseBehavior::for_item(&trident),
            DispenseBehavior::Default
        ));
        assert!(
            ITEM_BEHAVIORS
                .get_behavior(trident.item)
                .as_projectile_item()
                .is_some()
        );
    }
}
