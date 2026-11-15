//! The cloud: a cheap building block that can be put into thin air and melts for a while
//! under whoever stands on it. A port of the Fabric `CloudBlock` and `Clouds`.

use std::sync::Arc;

use glam::DVec3;
use steel_core::{
    behavior::{
        BlockBehavior, BlockHitResult, BlockItem, BlockLootContext, BlockPlaceContext,
        EntityFallDamage, EntityFallOnContext, InteractionResult, ItemBehavior,
        PathComputationType, PlacementSource, UseItemContext, UseOnContext,
    },
    entity::{Entity, damage::DamageSource},
    world::{ClipBlockShape, ClipFluid, ScheduledTickAccess as _, World},
};
use steel_protocol::packets::game::SoundSource;
use steel_registry::{
    blocks::{
        Block, BlockLightProperties, BlockRegistry, StateBooleanData, StateBooleanOverwrite,
        StateByteData, behavior::BlockConfig, block_state_ext::BlockStateExt as _,
        properties::IntProperty, shapes::VoxelShape,
    },
    fluid::FluidRef,
    item_stack::ItemStack,
    particle_type::ParticleData,
    sound_event::SoundEventHolder,
    sound_events, sound_types, vanilla_blocks, vanilla_damage_types, vanilla_particle_types,
};
use steel_utils::{BlockPos, BlockStateId, Direction, Identifier, WorldAabb, types::UpdateFlags};

use super::CLOUD;

/// 0 a cloud, 1 melting (someone stood on it), 2 gone for now.
pub static FADE: IntProperty = IntProperty::new("fade", 0, 2);
const SOLID: u8 = 0;
const FADING: u8 = 1;
const GONE: u8 = 2;

/// From the first step on a cloud until it is gone: 1.5 s.
const FADE_TICKS: i32 = 30;
/// A gone cloud gathers again after 10 s.
const RETURN_TICKS: i32 = 200;
/// Someone is where the cloud would come back: try again in a second.
const RETRY_TICKS: i32 = 20;
/// Share of the fall damage left when landing on a cloud: like a hay bale, 80% less.
const FALL_DAMAGE: f32 = 0.2;
/// How far from the eyes a cloud put into the air lands, in blocks.
const AIR_DISTANCE: f64 = 3.0;
/// Vanilla `MapColor.SNOW`.
const MAP_COLOR_SNOW: u8 = 8;

/// A whole block, or nothing once the cloud is gone.
const fn shape(offset: u16) -> VoxelShape {
    if offset == GONE as u16 {
        VoxelShape::EMPTY
    } else {
        VoxelShape::FULL_BLOCK
    }
}

const fn no_shape(_offset: u16) -> VoxelShape {
    VoxelShape::EMPTY
}

/// A block that does not occlude dims light by one when it is a full cube, and not at all
/// when it has no shape: vanilla `BlockBehaviour.getLightBlock`.
const fn light(offset: u16) -> BlockLightProperties {
    BlockLightProperties {
        light_emission: 0,
        light_dampening: if offset == GONE as u16 { 0 } else { 1 },
        use_shape_for_light_occlusion: false,
    }
}

/// A full cube until it is gone, as vanilla works both out from the collision shape.
const FULL_UNTIL_GONE: StateBooleanData =
    StateBooleanData::new(true, &[StateBooleanOverwrite::new(GONE as u16, false)]);

/// The cloud. A gone cloud is still this block (`fade` 2, no shape, air to the client), and
/// its return is a scheduled block tick saved with the chunk, so clouds of builds always
/// come back, even across a restart.
pub static CLOUD_BLOCK: Block = Block::new(
    Identifier::new_static("minehouse-tweaks", "cloud"),
    BlockConfig::new()
        .destroy_time(0.0)
        .can_occlude(false)
        .sound_type(sound_types::WOOL),
    &[&FADE],
)
.with_shapes(shape, shape, shape, no_shape, no_shape, shape)
.with_light_properties(light)
.with_map_color(StateByteData::new(MAP_COLOR_SNOW, &[]))
.with_suffocating(FULL_UNTIL_GONE)
.with_redstone_conductor(FULL_UNTIL_GONE);

/// Has clients see a cloud as `slot` (a note block state the resource pack draws as a
/// cloud; white wool without one), a gone cloud as air, and a cloud breaking as wool.
pub(super) fn show_to_clients(blocks: &mut BlockRegistry, slot: Option<BlockStateId>) {
    let wool = blocks.get_default_state_id(&vanilla_blocks::WHITE_WOOL);
    let air = blocks.get_default_state_id(&vanilla_blocks::AIR);
    let first = blocks.get_base_state_id(&CLOUD_BLOCK);
    for fade in [SOLID, FADING, GONE] {
        let state = blocks.set_property(first, &FADE, fade);
        let shown = if fade == GONE {
            air
        } else {
            slot.unwrap_or(wool)
        };
        blocks.show_to_clients_as(state, shown);
        blocks.show_break_to_clients_as(state, if fade == GONE { air } else { wool });
    }
}

fn is_cloud(state: BlockStateId) -> bool {
    state.get_block() == &CLOUD_BLOCK
}

fn gone(state: BlockStateId) -> bool {
    state.get_value(&FADE) == GONE
}

fn center(pos: BlockPos, above: f64) -> DVec3 {
    DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + 0.5 + above,
        f64::from(pos.z()) + 0.5,
    )
}

fn puff(world: &World, position: DVec3, count: i32, spread: DVec3, speed: f64) {
    world.send_particles(
        ParticleData::simple(&vanilla_particle_types::CLOUD),
        position,
        count,
        spread,
        speed,
    );
}

/// Starts melting a cloud under a player or a mob. True when it was a whole cloud.
fn start_fading(world: &Arc<World>, pos: BlockPos, state: BlockStateId) -> bool {
    if !is_cloud(state) || state.get_value(&FADE) != SOLID {
        return false;
    }
    world.set_block(
        pos,
        state.set_value(&FADE, FADING),
        UpdateFlags::UPDATE_CLIENTS,
    );
    world.schedule_block_tick_default(pos, &CLOUD_BLOCK, FADE_TICKS);
    puff(
        world,
        center(pos, 0.5),
        3,
        DVec3::new(0.35, 0.05, 0.35),
        0.005,
    );
    true
}

/// How the cloud block behaves.
pub(super) struct CloudBehavior;

impl BlockBehavior for CloudBehavior {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(CLOUD_BLOCK.default_state())
    }

    fn step_on(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos, entity: &dyn Entity) {
        if entity.is_living_entity() && !entity.is_spectator() {
            start_fading(world, pos, state);
        }
    }

    /// Landing on a cloud is soft.
    fn fall_on(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        context: EntityFallOnContext<'_>,
    ) -> Option<EntityFallDamage> {
        let spectator = context.source_entity.is_some_and(Entity::is_spectator);
        if context.entity.is_living_entity && !spectator {
            start_fading(world, pos, state);
        }
        Some(EntityFallDamage::new(
            context.fall_distance,
            FALL_DAMAGE,
            DamageSource::environment(&vanilla_damage_types::FALL),
        ))
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        let spread = DVec3::splat(0.3);
        match state.get_value(&FADE) {
            FADING => {
                world.set_block(pos, state.set_value(&FADE, GONE), UpdateFlags::UPDATE_ALL);
                world.schedule_block_tick_default(pos, &CLOUD_BLOCK, RETURN_TICKS);
                puff(world, center(pos, 0.0), 10, spread, 0.02);
                world.play_block_sound(&sound_events::BLOCK_WOOL_BREAK, pos, 0.6, 1.4, None);
            }
            GONE => {
                let (x, y, z) = (f64::from(pos.x()), f64::from(pos.y()), f64::from(pos.z()));
                let here = WorldAabb::new(x, y, z, x + 1.0, y + 1.0, z + 1.0);
                let in_the_way = !world
                    .get_entities_in_aabb_matching(&here, |entity| {
                        !entity.is_spectator() && entity.is_alive()
                    })
                    .is_empty();
                if in_the_way {
                    world.schedule_block_tick_default(pos, &CLOUD_BLOCK, RETRY_TICKS);
                    return;
                }
                world.set_block(pos, state.set_value(&FADE, SOLID), UpdateFlags::UPDATE_ALL);
                puff(world, center(pos, 0.0), 6, spread, 0.01);
                world.play_block_sound(&sound_events::BLOCK_WOOL_PLACE, pos, 0.4, 1.6, None);
            }
            _ => {}
        }
    }

    /// A gone cloud is air to the client, so a block put there takes its place (and the
    /// cloud does not come back).
    fn can_be_replaced(&self, state: BlockStateId, _context: &BlockPlaceContext<'_>) -> bool {
        gone(state)
    }

    fn can_be_replaced_by_fluid(&self, state: BlockStateId, _fluid: FluidRef) -> bool {
        gone(state)
    }

    fn is_pathfindable(&self, state: BlockStateId, _computation_type: PathComputationType) -> bool {
        gone(state)
    }

    fn get_drops(
        &self,
        _state: BlockStateId,
        _context: &BlockLootContext<'_>,
    ) -> Option<Vec<ItemStack>> {
        Some(vec![CLOUD.stack(1)])
    }
}

/// The block `distance` blocks from the eyes along the look.
fn air_target(eye: DVec3, look: DVec3, distance: f64) -> BlockPos {
    let target = eye + look * distance;
    BlockPos::new(
        target.x.floor() as i32,
        target.y.floor() as i32,
        target.z.floor() as i32,
    )
}

/// What a stack of clouds does in a player's hand.
pub(super) struct CloudItem;

/// The one [`CloudItem`].
pub(super) static CLOUD_ITEM: CloudItem = CloudItem;

impl ItemBehavior for CloudItem {
    /// Aimed at a block, a cloud is placed like any other block.
    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        BlockItem::new(&CLOUD_BLOCK).place(context.build_place_context())
    }

    /// Right click with clouds while not looking at a block puts one [`AIR_DISTANCE`] blocks
    /// in front of the eyes (so looking straight down it lands two blocks under the feet).
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        let (player, world) = (context.player, context.world);
        let position = player.position();
        let eye = DVec3::new(position.x, player.get_eye_y(), position.z);
        let look = player.look_angle();

        // Aimed at a block in reach: the normal placement already had its go.
        let reach = eye + look * player.block_interaction_range();
        let aimed = world.clip(eye, reach, ClipBlockShape::Outline, ClipFluid::None);
        if !aimed.miss {
            return InteractionResult::Pass;
        }

        let pos = air_target(eye, look, AIR_DISTANCE);
        if !world.is_block_within_world_border(pos) {
            return InteractionResult::Fail;
        }

        let (yaw, pitch) = player.rotation();
        let hit = BlockHitResult {
            location: center(pos, 0.0),
            direction: Direction::ordered_by_nearest(yaw, pitch)[0].opposite(),
            block_pos: pos,
            miss: false,
            inside: false,
            world_border_hit: false,
        };
        let place = BlockPlaceContext::new(
            world,
            PlacementSource::player_hand(player, &context.inv),
            &hit,
        );
        // Only straight into air (or grass, snow layers and the like), never next to
        // something else.
        if !place.replaces_clicked_block() {
            return InteractionResult::Fail;
        }
        if !BlockItem::new(&CLOUD_BLOCK).place(place).consumes_action() {
            return InteractionResult::Fail;
        }

        // The placement plays its sound for everyone but the placer, whose client plays it
        // itself when it places a block. This client placed nothing, so it gets the sound.
        let sound = &CLOUD_BLOCK.config.sound_type;
        world.play_sound_to_players(
            &SoundEventHolder::Registry(sound.place_sound),
            SoundSource::Blocks,
            center(pos, 0.0),
            f32::midpoint(sound.volume, 1.0),
            sound.pitch * 0.8,
            0.0,
            &[Arc::clone(player)],
        );
        puff(world, center(pos, 0.0), 6, DVec3::splat(0.3), 0.01);
        // The server swings the hand, for the player too: the client does not swing when
        // it uses a block item on air.
        InteractionResult::SuccessServer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cloud_is_a_whole_block_until_it_is_gone() {
        for fade in [SOLID, FADING] {
            let offset = u16::from(fade);
            assert_eq!(
                CLOUD_BLOCK.get_collision_shape(offset),
                VoxelShape::FULL_BLOCK
            );
            assert_eq!(
                CLOUD_BLOCK.get_outline_shape(offset),
                VoxelShape::FULL_BLOCK
            );
            assert!(CLOUD_BLOCK.suffocating.value(offset));
            assert_eq!(CLOUD_BLOCK.get_light_properties(offset).light_dampening, 1);
        }
        let gone = u16::from(GONE);
        assert_eq!(CLOUD_BLOCK.get_collision_shape(gone), VoxelShape::EMPTY);
        assert_eq!(CLOUD_BLOCK.get_outline_shape(gone), VoxelShape::EMPTY);
        assert!(!CLOUD_BLOCK.suffocating.value(gone));
        assert_eq!(CLOUD_BLOCK.get_light_properties(gone).light_dampening, 0);
        // It never hides the faces of the blocks around it.
        assert_eq!(CLOUD_BLOCK.get_occlusion_shape(0), VoxelShape::EMPTY);
    }

    #[test]
    fn a_cloud_put_into_the_air_lands_three_blocks_along_the_look() {
        let eye = DVec3::new(0.5, 65.62, 0.5);
        // Straight down: two blocks under the feet (the feet are at 64).
        assert_eq!(
            air_target(eye, DVec3::NEG_Y, AIR_DISTANCE),
            BlockPos::new(0, 62, 0)
        );
        // Straight ahead, at eye height.
        assert_eq!(
            air_target(eye, DVec3::X, AIR_DISTANCE),
            BlockPos::new(3, 65, 0)
        );
        // Negative coordinates round down, not towards zero.
        assert_eq!(
            air_target(DVec3::new(-0.5, 65.62, -0.5), DVec3::NEG_Z, AIR_DISTANCE),
            BlockPos::new(-1, 65, -4)
        );
    }
}
