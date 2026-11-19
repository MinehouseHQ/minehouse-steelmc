//! Lying down (the `/lay` of the mod `FSit` the Fabric server had).
//!
//! `/lay` lays the player down where they stand, the way `FSit` does it for a client without
//! the mod: the player sleeps without a bed. They get the sleeping pose and a sleeping
//! position at the bottom of the world under them, and every client in the world is shown a
//! bed there, which only tells it which way the sleeper lies. The player's own client then
//! shows them lying and offers its "leave bed" button. The night does not pass for it.
//!
//! The button, `/lay` again, damage or anything else that wakes a sleeper gets them up.

use std::sync::Arc;

use rustc_hash::FxHashMap;
use steel_core::{
    command::{
        CommandContext, CommandError, CommandNode, CommandRegistration, CommandRegistrationError,
        CommandRegistry, literal,
    },
    entity::{Entity as _, LivingEntity as _},
    player::Player,
    server::{
        Server,
        jobs::{JobPoll, ServerJob, ServerJobContext},
    },
    world::World,
};
use steel_protocol::packets::game::CBlockUpdate;
use steel_registry::{
    blocks::{
        block_state_ext::BlockStateExt as _,
        properties::{BedPart, BlockStateProperties},
    },
    entity_data::EntityPose,
    vanilla_blocks,
};
use steel_utils::{BlockPos, BlockStateId, Direction, Identifier, locks::SyncMutex};
use uuid::Uuid;

use super::sitting;
use crate::moderation::format::render;

/// Every how many ticks the clients are shown the beds again: one that came into the world
/// or loaded the chunk since has the real block there.
const SHOW_BEDS_EVERY_TICKS: u64 = 20;

/// One lying player.
struct Lying {
    /// The world they lay down in.
    world: Arc<World>,
    /// Where the clients are shown a bed.
    bed: BlockPos,
    /// The bed they are shown.
    state: BlockStateId,
}

static LYING: SyncMutex<Option<FxHashMap<Uuid, Lying>>> = SyncMutex::new(None);

/// Which way the bed of a sleeper who looks along `yaw` faces, so that they lie the way
/// they stood (as `FSit` turns it).
fn bed_facing(yaw: f32) -> Direction {
    Direction::from_yaw(yaw).opposite()
}

/// The bed a client is shown for a sleeper who looks along `yaw`: its head.
fn bed_for(yaw: f32) -> BlockStateId {
    vanilla_blocks::BLACK_BED
        .default_state()
        .set_value(&BlockStateProperties::HORIZONTAL_FACING, bed_facing(yaw))
        .set_value(&BlockStateProperties::BED_PART, BedPart::Head)
}

/// Where the bed of `player` is shown: in their block column, at the bottom of the
/// world, out of sight.
fn bed_under(world: &World, player: &Player) -> BlockPos {
    let at = player.block_position();
    BlockPos::new(at.x(), world.get_min_y(), at.z())
}

fn show(world: &World, pos: BlockPos, block_state: BlockStateId) {
    world.players.iter_players(|_, player| {
        player.send_packet(CBlockUpdate { pos, block_state });
        true
    });
}

impl Lying {
    /// Whether `player` still lies as they lay down.
    fn goes_on(&self, player: &Player) -> bool {
        player.lies_without_bed()
            && player.sleeping_pos() == Some(self.bed)
            && Arc::ptr_eq(&self.world, &player.get_world())
    }

    /// Shows the clients the block that is really there.
    fn hide_bed(&self) {
        show(&self.world, self.bed, self.world.get_block_state(self.bed));
    }
}

/// Gets `player` up, if they still lie.
fn get_up(player: &Player, lay: &Lying) {
    let lies = lay.goes_on(player);
    if player.lies_without_bed() {
        player.set_forced_pose(None);
    }
    if lies {
        player.stop_sleeping();
    }
    lay.hide_bed();
}

/// Keeps the beds shown and tidies up after the players who got up.
struct LyingDown;

impl ServerJob for LyingDown {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        let Some(server) = context.server() else {
            return JobPoll::Finished;
        };
        let mut lying = LYING.lock();
        let Some(lying) = lying.as_mut().filter(|lying| !lying.is_empty()) else {
            return JobPoll::Pending;
        };
        let players = server.get_players();
        let show_beds = context.tick_count.is_multiple_of(SHOW_BEDS_EVERY_TICKS);
        lying.retain(|uuid, lay| {
            let player = players.iter().find(|player| player.uuid() == *uuid);
            // Woken by the button, by damage, by a teleport: the pose is all that is left.
            let Some(player) = player.filter(|player| lay.goes_on(player)) else {
                if let Some(player) = player
                    && player.lies_without_bed()
                {
                    player.set_forced_pose(None);
                }
                lay.hide_bed();
                return false;
            };
            if player.is_dead_or_dying() {
                get_up(player, lay);
                return false;
            }
            if show_beds {
                show(&lay.world, lay.bed, lay.state);
            }
            true
        });
        JobPoll::Pending
    }
}

fn lay_command(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let source = context.source();
    let Some(player) = source.player() else {
        return Err(CommandError::from("Лечь может только игрок."));
    };
    let mut lying = LYING.lock();
    let lying = lying.get_or_insert_default();
    if let Some(lay) = lying.remove(&player.uuid()) {
        get_up(player, &lay);
        return Ok(1);
    }
    if player.forced_pose().is_some() || !sitting::may_sit(player) || !player.on_ground() {
        return Err(CommandError::from("Здесь и сейчас не лечь."));
    }
    let world = player.get_world();
    let bed = bed_under(&world, player);
    let state = bed_for(player.rotation().0);
    show(&world, bed, state);
    player.set_forced_pose(Some(EntityPose::Sleeping));
    player.set_sleeping_pos(bed);
    player.set_velocity(glam::DVec3::ZERO);
    lying.insert(player.uuid(), Lying { world, bed, state });
    player.send_overlay_message(&render(
        "&7Чтобы встать, покинь кровать или введи /lay",
        &[],
    ));
    Ok(1)
}

fn lay_tree() -> CommandNode {
    literal("lay").executes(|context| lay_command(*context))
}

pub(super) fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "lay"), lay_tree).default_access(),
    )?;
    Ok(())
}

/// Starts watching the lying players.
pub(super) fn start(server: &Server) {
    server.jobs.spawn(LyingDown);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bed_lies_the_way_the_player_stood() {
        // Yaw 0 looks south, yaw 90 west.
        assert_eq!(bed_facing(0.0), Direction::North);
        assert_eq!(bed_facing(90.0), Direction::East);
        assert_eq!(bed_facing(-90.0), Direction::West);
    }
}
