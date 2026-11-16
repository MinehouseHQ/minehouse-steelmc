//! The closed End (the Fabric `EndLock`). While it is closed nobody can put an eye of ender
//! into a portal frame, operators included, and whoever still ends up in the End (a portal
//! lit before, a command) is sent out within a second, to where the exit portal leads. Only
//! a spectator with `minehouse.end.bypass` may stay.
//!
//! Off unless `config/minehouse/tweaks.toml` says `closed = true` under `[end]`.

use std::{
    fs,
    sync::{Arc, OnceLock},
};

use serde::{Deserialize, Serialize};
use steel_core::{
    behavior::InteractionResult,
    permission::{PermissionExpr, PermissionKey},
    player::Player,
    server::{
        Server,
        jobs::{JobPoll, ServerJob, ServerJobContext},
    },
    world::World,
};
use steel_registry::{
    blocks::block_state_ext::BlockStateExt as _, items::item::BlockHitResult, vanilla_blocks,
    vanilla_items,
};
use steel_utils::types::{GameType, InteractionHand};

use crate::{config, moderation::format::render};

/// The config module: `config/minehouse/tweaks.toml`.
const CONFIG: &str = "tweaks";
/// Who may stay in the closed End as a spectator.
const BYPASS: &str = "minehouse.end.bypass";
/// Every how many ticks the End is looked over.
const CHECK_EVERY_TICKS: u64 = 20;

/// The `[end]` table of the config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EndConfig {
    /// Whether the End is closed (the Fabric `end.disabled`).
    pub closed: bool,
    /// What a player is told, with `&` color codes (the Fabric `end.message`).
    pub message: String,
}

impl Default for EndConfig {
    fn default() -> Self {
        Self {
            closed: false,
            message: "&5Энд пока закрыт.".to_owned(),
        }
    }
}

/// `config/minehouse/tweaks.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct TweaksConfig {
    end: EndConfig,
    cloud: CloudConfig,
    minecarts: MinecartConfig,
}

/// The `[minecarts]` table of the config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MinecartConfig {
    /// The top speed of a fast cart, blocks a second (the Fabric `minehouse:minecart_speed`).
    pub max_speed: u32,
}

impl MinecartConfig {
    /// The speed of a vanilla cart on powered rails, the least the top speed can be.
    const VANILLA_SPEED: u32 = 8;
    /// The highest top speed accepted.
    const HIGHEST_SPEED: u32 = 100;
}

impl Default for MinecartConfig {
    fn default() -> Self {
        Self { max_speed: 80 }
    }
}

/// The top speed of a fast cart, blocks a second. Read once.
pub(super) fn minecart_top_speed() -> f64 {
    static SPEED: OnceLock<u32> = OnceLock::new();
    f64::from(*SPEED.get_or_init(|| {
        config::load::<TweaksConfig>(&config::path(CONFIG))
            .inspect_err(|error| log::error!("MineHouse tweaks: {error}, carts keep 80 blocks/s"))
            .map_or_else(|_| MinecartConfig::default(), |tweaks| tweaks.minecarts)
            .max_speed
            .clamp(MinecartConfig::VANILLA_SPEED, MinecartConfig::HIGHEST_SPEED)
    }))
}

/// The `[cloud]` table of the config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudConfig {
    /// Whether the cloud block can be crafted. Clouds that already exist stay as they are.
    pub craft: bool,
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self { craft: true }
    }
}

/// Whether the cloud block can be crafted. Read when the registry is built, so before the
/// End settings; a missing or unreadable config leaves the recipe on.
pub(super) fn cloud_is_crafted() -> bool {
    let path = config::path(CONFIG);
    let Ok(text) = fs::read_to_string(&path) else {
        return true;
    };
    toml::from_str::<TweaksConfig>(&text)
        .inspect_err(|error| log::error!("MineHouse tweaks: {error}, the cloud stays craftable"))
        .map_or(true, |tweaks| tweaks.cloud.craft)
}

static END: OnceLock<EndConfig> = OnceLock::new();

/// The `[end]` settings, read once.
fn settings() -> &'static EndConfig {
    END.get_or_init(|| {
        config::load::<TweaksConfig>(&config::path(CONFIG))
            .inspect_err(|error| log::error!("MineHouse tweaks: {error}, the End stays open"))
            .map_or_else(|_| EndConfig::default(), |tweaks| tweaks.end)
    })
}

/// Whether a player in the End has to leave: all but spectators allowed to stay.
#[must_use]
pub const fn must_leave(spectator: bool, bypass: bool) -> bool {
    !(spectator && bypass)
}

/// `player` clicked the block at `hit` with `hand`: turns down an eye of ender on a portal
/// frame while the End is closed. `None` leaves the click to vanilla.
pub(super) fn clicked(
    player: &Arc<Player>,
    world: &Arc<World>,
    hand: InteractionHand,
    hit: &BlockHitResult,
) -> Option<InteractionResult> {
    let end = settings();
    if !end.closed {
        return None;
    }
    let frame =
        world.get_block_state(hit.block_pos).get_block() == &vanilla_blocks::END_PORTAL_FRAME;
    let eye = player
        .inventory
        .lock()
        .get_item_in_hand(hand)
        .is(&vanilla_items::ENDER_EYE);
    if !(eye && frame) {
        return None;
    }
    player.send_overlay_message(&render(&end.message, &[]));
    Some(InteractionResult::Fail)
}

fn bypass_permission() -> Option<PermissionExpr> {
    PermissionKey::parse(BYPASS)
        .inspect_err(|error| log::error!("Bad permission {BYPASS}: {error}"))
        .ok()
        .map(PermissionExpr::key)
}

/// Sends players out of the closed End.
struct EndLock {
    bypass: Option<PermissionExpr>,
}

impl EndLock {
    fn check(&self, server: &Server) {
        for player in server.get_players() {
            if !player.get_world().is_end_dimension_type() {
                continue;
            }
            let bypass = self
                .bypass
                .as_ref()
                .is_some_and(|permission| player.has_permission(permission));
            if !must_leave(player.game_mode() == GameType::Spectator, bypass) {
                continue;
            }
            if player.leave_end() {
                player.send_message(&render(&settings().message, &[]));
                log::info!("End is closed, sent {} back out", player.gameprofile.name());
            }
        }
    }
}

impl ServerJob for EndLock {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        let Some(server) = context.server() else {
            return JobPoll::Finished;
        };
        if context.tick_count.is_multiple_of(CHECK_EVERY_TICKS) {
            self.check(&server);
        }
        JobPoll::Pending
    }
}

/// Starts watching the End, if it is closed.
pub(super) fn start(server: &Server) {
    if !settings().closed {
        return;
    }
    log::info!("MineHouse: the End is closed");
    server.jobs.spawn(EndLock {
        bypass: bypass_permission(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_end_is_open_unless_the_config_closes_it() {
        let config: TweaksConfig = toml::from_str("").unwrap_or_default();
        assert!(!config.end.closed);
        assert_eq!(config.end.message, "&5Энд пока закрыт.");

        assert!(config.cloud.craft);
        let no_cloud: TweaksConfig = toml::from_str("[cloud]\ncraft = false\n").unwrap_or_default();
        assert!(!no_cloud.cloud.craft);
        let closed: Result<TweaksConfig, _> = toml::from_str("[end]\nclosed = true\n");
        assert_eq!(closed.map(|config| config.end.closed), Ok(true));
    }

    #[test]
    fn only_a_spectator_with_the_permission_stays() {
        assert!(!must_leave(true, true));
        assert!(must_leave(true, false));
        assert!(must_leave(false, true));
        assert!(must_leave(false, false));
    }
}
