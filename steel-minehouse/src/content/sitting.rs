//! Sitting down (the `/sit` half of the mod `FSit` the Fabric server had).
//!
//! A player sits on the spot with `/sit` or by sneaking twice while looking down, and gets
//! up by sneaking. Whoever turned it on with `/fsit blocksit on` also sits down on a slab,
//! stairs, a log, a fence or a wall by clicking it with an empty hand.
//!
//! A sitting player rides an invisible marker armor stand with the tag
//! [`SEAT_TAG`]. A seat nobody sits on is removed, also one a stopped server left behind.

use std::{fs, sync::Arc};

use glam::DVec3;
use rustc_hash::{FxHashMap, FxHashSet};
use steel_core::{
    behavior::InteractionResult,
    command::{
        CommandContext, CommandError, CommandNode, CommandRegistration, CommandRegistrationError,
        CommandRegistry, literal,
    },
    entity::{
        Entity as _, LivingEntity as _, RemovalReason, SharedEntity, entities::ArmorStandEntity,
        next_entity_id,
    },
    player::Player,
    server::{
        Server,
        jobs::{JobPoll, ServerJob, ServerJobContext},
    },
    world::World,
};
use steel_registry::{
    blocks::{
        block_state_ext::BlockStateExt as _,
        properties::{BlockStateProperties, Half, SlabType},
    },
    items::item::BlockHitResult,
    vanilla_block_tags::BlockTag,
    vanilla_entities,
};
use steel_utils::{
    BlockPos, BlockStateId, Identifier, WorldAabb,
    locks::SyncMutex,
    types::{GameType, InteractionHand},
};
use uuid::Uuid;

use crate::{config, moderation::format::render};

/// The scoreboard tag of a seat.
const SEAT_TAG: &str = "minehouse_seat";
/// Two sneaks this many ticks apart or closer sit the player down (the mod's `window-ticks`).
const SNEAK_WINDOW_TICKS: u64 = 10;
/// How far down the player has to look for it, degrees (the mod's `min-pitch-down`).
const MIN_PITCH_DOWN: f32 = 60.0;
/// From how far a block can be sat on by a click, blocks.
const BLOCK_REACH: f64 = 3.0;
/// Every how many ticks seats left behind are looked for.
const SWEEP_EVERY_TICKS: u64 = 100;
/// How far around a player they are looked for, blocks.
const SWEEP_RADIUS: f64 = 32.0;
/// The file with the players who sit down by clicking blocks.
const SETTINGS: &str = "sitting.json";

/// The players who sit down by clicking blocks. Read once, written on every change.
static BLOCK_SITTERS: SyncMutex<Option<FxHashSet<Uuid>>> = SyncMutex::new(None);
/// The seats in use, with where they are, for [`clicked`] to know a block is taken.
static SEATS: SyncMutex<Vec<SharedEntity>> = SyncMutex::new(Vec::new());

fn with_block_sitters<T>(run: impl FnOnce(&mut FxHashSet<Uuid>) -> T) -> T {
    let mut sitters = BLOCK_SITTERS.lock();
    let sitters = sitters.get_or_insert_with(|| {
        fs::read_to_string(config::data_path(SETTINGS))
            .ok()
            .and_then(|text| serde_json::from_str::<Vec<Uuid>>(&text).ok())
            .map_or_else(FxHashSet::default, |uuids| uuids.into_iter().collect())
    });
    run(sitters)
}

fn save_block_sitters(sitters: &FxHashSet<Uuid>) {
    let mut uuids: Vec<&Uuid> = sitters.iter().collect();
    uuids.sort();
    let path = config::data_path(SETTINGS);
    let written = serde_json::to_string(&uuids)
        .map_err(|error| error.to_string())
        .and_then(|text| {
            if let Some(folder) = path.parent() {
                fs::create_dir_all(folder).map_err(|error| error.to_string())?;
            }
            fs::write(&path, text).map_err(|error| error.to_string())
        });
    if let Err(error) = written {
        log::warn!(
            "MineHouse sitting: {} was not written: {error}",
            path.display()
        );
    }
}

/// Whether a player in this state may sit down at all, or crawl.
pub(super) fn may_sit(player: &Player) -> bool {
    !player.is_passenger()
        && !player.is_dead_or_dying()
        && player.game_mode() != GameType::Spectator
        && !player.is_flying()
        && !player.is_fall_flying()
        && !player.is_under_water()
        && !player.is_sleeping()
}

/// Whether two sneaks `since_last` ticks apart, with the player looking down by `pitch`
/// degrees, sit the player down.
fn sneaks_sit(since_last: u64, pitch: f32) -> bool {
    since_last <= SNEAK_WINDOW_TICKS && pitch >= MIN_PITCH_DOWN
}

/// Puts `player` on a new seat at `position`. `false` if the seat could not be made.
fn sit(player: &Player, position: DVec3) -> bool {
    let world = player.get_world();
    let stand = ArmorStandEntity::new(
        &vanilla_entities::ARMOR_STAND,
        next_entity_id(),
        position,
        Arc::downgrade(&world),
    );
    stand.set_marker(true);
    stand.set_invisible(true);
    stand.set_no_gravity(true);
    stand.set_silent(true);
    stand.set_invulnerable(true);
    stand.set_rotation((player.rotation().0, 0.0));
    stand.add_tag(SEAT_TAG.to_owned());
    let seat: SharedEntity = Arc::new(stand);
    if world.try_add_entity(Arc::clone(&seat)).is_err() {
        return false;
    }
    // The sneak that asked for the seat must not take the player off it again.
    player.set_crouching(false);
    if !player.start_riding(&seat, true) {
        seat.set_removed(RemovalReason::Discarded);
        return false;
    }
    SEATS.lock().push(seat);
    true
}

/// How high above the bottom of its block a seat on `state` is, if it can be sat on.
fn seat_height(state: BlockStateId) -> Option<f64> {
    let block = state.get_block();
    if block.has_tag(&BlockTag::SLABS) {
        let low = state.try_get_value(&BlockStateProperties::SLAB_TYPE) == Some(SlabType::Bottom);
        return Some(if low { 0.5 } else { 1.0 });
    }
    if block.has_tag(&BlockTag::STAIRS) {
        let low = state.try_get_value(&BlockStateProperties::HALF) == Some(Half::Bottom);
        return Some(if low { 0.5 } else { 1.0 });
    }
    [BlockTag::LOGS, BlockTag::FENCES, BlockTag::WALLS]
        .iter()
        .any(|tag| block.has_tag(tag))
        .then_some(1.0)
}

/// Where the seat on the block at `pos` is.
fn seat_on(pos: BlockPos, height: f64) -> DVec3 {
    DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + height,
        f64::from(pos.z()) + 0.5,
    )
}

/// `player` clicked the block at `hit` with `hand`: sits them down on it, if they asked for
/// that and the block is one to sit on. `None` leaves the click to vanilla.
pub(super) fn clicked(
    player: &Arc<Player>,
    world: &Arc<World>,
    hand: InteractionHand,
    hit: &BlockHitResult,
) -> Option<InteractionResult> {
    if hand != InteractionHand::MainHand || player.is_crouching() || !may_sit(player) {
        return None;
    }
    if !with_block_sitters(|sitters| sitters.contains(&player.uuid())) {
        return None;
    }
    if !player
        .inventory
        .lock()
        .get_item_in_hand(InteractionHand::MainHand)
        .is_empty()
    {
        return None;
    }
    let pos = hit.block_pos;
    let height = seat_height(world.get_block_state(pos))?;
    let above = world.get_block_state(pos.above());
    if !above.is_air() {
        return None;
    }
    let position = seat_on(pos, height);
    if player.position().distance(position) > BLOCK_REACH {
        return None;
    }
    let taken = SEATS.lock().iter().any(|seat| {
        !seat.is_removed() && seat.is_vehicle() && seat.position().distance(position) < 0.1
    });
    if taken {
        return None;
    }
    sit(player, position).then_some(InteractionResult::Success)
}

/// Watches sneaks and clears seats away.
#[derive(Default)]
struct Sitting {
    /// Per player: whether they sneaked on the last tick, and the tick the last sneak began.
    sneaks: FxHashMap<Uuid, (bool, Option<u64>)>,
}

impl Sitting {
    fn watch_sneaks(&mut self, server: &Server, tick: u64) {
        let players = server.get_players();
        self.sneaks
            .retain(|uuid, _| players.iter().any(|player| player.uuid() == *uuid));
        for player in players {
            let sneaking = player.is_crouching();
            let (was_sneaking, last_began) = self.sneaks.entry(player.uuid()).or_default();
            let began = sneaking && !*was_sneaking;
            *was_sneaking = sneaking;
            if !began {
                continue;
            }
            let since_last = last_began.map(|last| tick.saturating_sub(last));
            *last_began = Some(tick);
            let Some(since_last) = since_last else {
                continue;
            };
            if sneaks_sit(since_last, player.rotation().1)
                && may_sit(&player)
                && player.on_ground()
                && sit(&player, player.position())
            {
                *was_sneaking = false;
                *last_began = None;
            }
        }
    }

    /// Removes the seats nobody sits on any more.
    fn clear_empty() {
        SEATS.lock().retain(|seat| {
            if seat.is_removed() {
                return false;
            }
            if seat.is_vehicle() {
                return true;
            }
            seat.set_removed(RemovalReason::Discarded);
            false
        });
    }

    /// Removes the empty seats near players that are not in [`SEATS`]: a stopped server
    /// saved them with their chunk.
    fn sweep(server: &Server) {
        for player in server.get_players() {
            let side = SWEEP_RADIUS * 2.0;
            let around = WorldAabb::of_size(player.position(), side, side, side);
            let left = player
                .get_world()
                .get_entities_in_aabb_matching(&around, |entity| {
                    !entity.is_vehicle() && entity.tags().iter().any(|tag| tag == SEAT_TAG)
                });
            for seat in left {
                seat.set_removed(RemovalReason::Discarded);
            }
        }
    }
}

impl ServerJob for Sitting {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        let Some(server) = context.server() else {
            return JobPoll::Finished;
        };
        Self::clear_empty();
        self.watch_sneaks(&server, context.tick_count);
        if context.tick_count.is_multiple_of(SWEEP_EVERY_TICKS) {
            Self::sweep(&server);
        }
        JobPoll::Pending
    }
}

fn sit_command(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let source = context.source();
    let Some(player) = source.player() else {
        return Err(CommandError::from("Сесть может только игрок."));
    };
    if player.forced_pose().is_some() || !may_sit(player) || !player.on_ground() {
        return Err(CommandError::from("Здесь и сейчас не сесть."));
    }
    if !sit(player, player.position()) {
        return Err(CommandError::from("Не получилось сесть."));
    }
    Ok(1)
}

fn block_sit_command(context: CommandContext<'_>, on: bool) -> Result<i32, CommandError> {
    let source = context.source();
    let Some(player) = source.player() else {
        return Err(CommandError::from("Настройка есть только у игрока."));
    };
    let uuid = player.uuid();
    with_block_sitters(|sitters| {
        let changed = if on {
            sitters.insert(uuid)
        } else {
            sitters.remove(&uuid)
        };
        if changed {
            save_block_sitters(sitters);
        }
    });
    let text = if on {
        "&7Клик пустой рукой по ступеням, плитам, брёвнам, заборам и стенам &aусаживает&7."
    } else {
        "&7Клик по блокам больше &cне усаживает&7."
    };
    source.send_success(&render(text, &[]), false);
    Ok(1)
}

fn sit_tree() -> CommandNode {
    literal("sit").executes(|context| sit_command(*context))
}

fn fsit_tree() -> CommandNode {
    literal("fsit").then(
        literal("blocksit")
            .then(literal("on").executes(|context| block_sit_command(*context, true)))
            .then(literal("off").executes(|context| block_sit_command(*context, false))),
    )
}

pub(super) fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "sit"), sit_tree).default_access(),
    )?;
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "fsit"), fsit_tree).default_access(),
    )?;
    Ok(())
}

/// Starts watching sneaks and seats.
pub(super) fn start(server: &Server) {
    server.jobs.spawn(Sitting::default());
}

#[cfg(test)]
mod tests {
    use steel_core::bootstrap::init_globals;
    use steel_registry::vanilla_blocks;

    use super::*;

    #[test]
    fn two_quick_sneaks_looking_down_sit() {
        assert!(sneaks_sit(4, 75.0));
        assert!(sneaks_sit(SNEAK_WINDOW_TICKS, MIN_PITCH_DOWN));
        assert!(!sneaks_sit(SNEAK_WINDOW_TICKS + 1, 90.0));
        assert!(!sneaks_sit(2, 30.0));
        assert!(!sneaks_sit(2, -80.0));
    }

    #[test]
    fn seats_are_at_the_top_of_what_is_sat_on() {
        init_globals();
        let slab = vanilla_blocks::OAK_SLAB.default_state();
        assert_eq!(seat_height(slab), Some(0.5));
        let top = slab.set_value(&BlockStateProperties::SLAB_TYPE, SlabType::Top);
        assert_eq!(seat_height(top), Some(1.0));
        let stairs = vanilla_blocks::OAK_STAIRS.default_state();
        assert_eq!(seat_height(stairs), Some(0.5));
        assert_eq!(
            seat_height(vanilla_blocks::OAK_LOG.default_state()),
            Some(1.0)
        );
        assert_eq!(
            seat_height(vanilla_blocks::OAK_FENCE.default_state()),
            Some(1.0)
        );
        assert_eq!(seat_height(vanilla_blocks::STONE.default_state()), None);
        assert_eq!(
            seat_on(BlockPos::new(-3, 64, 7), 0.5),
            DVec3::new(-2.5, 64.5, 7.5)
        );
    }
}
