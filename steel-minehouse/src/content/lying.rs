//! Lying down (the `/lay` of the mod `FSit` the Fabric server had).
//!
//! `/lay` lays the player down where they stand, as others see it: in the pose of a
//! sleeping player, without a bed, without the night passing and without the "leave bed"
//! screen. Sneaking, a step aside or `/lay` again gets them up. A client decides its own
//! pose, so in their own third person view the player still stands.

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_core::{
    command::{
        CommandContext, CommandError, CommandNode, CommandRegistration, CommandRegistrationError,
        CommandRegistry, literal,
    },
    entity::Entity as _,
    server::{
        Server,
        jobs::{JobPoll, ServerJob, ServerJobContext},
    },
};
use steel_registry::entity_data::EntityPose;
use steel_utils::{Identifier, locks::SyncMutex};
use uuid::Uuid;

use super::sitting;
use crate::moderation::format::render;

/// How far a lying player may be moved before they are up again, blocks.
const STAYS_WITHIN: f64 = 0.2;

/// One lying player.
struct Lying {
    /// Where they lay down.
    at: DVec3,
    /// Whether they sneaked on the last tick.
    sneaking: bool,
}

static LYING: SyncMutex<Option<FxHashMap<Uuid, Lying>>> = SyncMutex::new(None);

/// Whether a player lying at `at` and now at `now` still lies.
fn stays(at: DVec3, now: DVec3) -> bool {
    at.distance_squared(now) <= STAYS_WITHIN * STAYS_WITHIN
}

/// Gets the lying players up once they sneak, move or cannot lie any more.
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
        lying.retain(|uuid, lay| {
            let Some(player) = players.iter().find(|player| player.uuid() == *uuid) else {
                return false;
            };
            let sneaked = player.is_crouching() && !lay.sneaking;
            lay.sneaking = player.is_crouching();
            // A respawned player is a new one, without the pose.
            let goes_on = !sneaked
                && player.forced_pose() == Some(EntityPose::Sleeping)
                && sitting::may_sit(player)
                && stays(lay.at, player.position());
            if !goes_on && player.forced_pose() == Some(EntityPose::Sleeping) {
                player.set_forced_pose(None);
            }
            goes_on
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
    if lying.remove(&player.uuid()).is_some() {
        player.set_forced_pose(None);
        return Ok(1);
    }
    if player.forced_pose().is_some() || !sitting::may_sit(player) || !player.on_ground() {
        return Err(CommandError::from("Здесь и сейчас не лечь."));
    }
    player.set_forced_pose(Some(EntityPose::Sleeping));
    lying.insert(
        player.uuid(),
        Lying {
            at: player.position(),
            sneaking: player.is_crouching(),
        },
    );
    player.send_overlay_message(&render("&7Шифт, чтобы встать", &[]));
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
    fn a_step_aside_gets_a_lying_player_up() {
        let at = DVec3::new(10.5, 64.0, -3.5);
        assert!(stays(at, at));
        assert!(stays(at, at + DVec3::new(0.1, 0.0, 0.1)));
        assert!(!stays(at, at + DVec3::new(0.3, 0.0, 0.0)));
        assert!(!stays(at, at + DVec3::new(0.0, -1.0, 0.0)));
    }
}
