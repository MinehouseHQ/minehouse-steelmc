//! Crawling (the `/crawl` of the mod `FSit` the Fabric server had).
//!
//! `/crawl` puts the player into the swimming pose on land, and sneaking or `/crawl` again
//! gets them up. A client decides its own pose, and takes the swimming one only where it
//! has no room for anything taller. So the crawling player alone is sent an invisible
//! shulker just above their head, which is never in the world, and it follows them. The
//! server keeps the pose for everybody else with [`Player::set_forced_pose`].

use std::{f64::consts::PI, sync::Arc};

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_core::{
    command::{
        CommandContext, CommandError, CommandNode, CommandRegistration, CommandRegistrationError,
        CommandRegistry, literal,
    },
    entity::{Entity as _, next_entity_id},
    player::Player,
    server::{
        Server,
        jobs::{JobPoll, ServerJob, ServerJobContext},
    },
};
use steel_protocol::packets::game::{
    CAddEntity, CEntityPositionSync, CRemoveEntities, CSetEntityData, PositionPath,
};
use steel_registry::{
    RegistryEntry as _, entity_data::EntityPose, vanilla_entities,
    vanilla_entity_data::ShulkerEntityData,
};
use steel_utils::{Direction, Identifier, locks::SyncMutex};
use uuid::Uuid;

use super::sitting;
use crate::moderation::format::render;

/// How far above the feet of a crawling player the bottom of the shulker is, blocks. Less
/// than the height of a sneaking player (1.5), for the client not to take that pose, and
/// enough above a crawling one (0.6) to crawl up a slab or a stair.
const ROOF_ABOVE_FEET: f64 = 1.45;
/// The shared flag of an invisible entity.
const INVISIBLE: i8 = 1 << 5;

/// Where the shulker of one player is and how far it is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Roof {
    x: i32,
    y: i32,
    z: i32,
    /// The shulker's `peek`, 0 to 100.
    peek: i8,
}

impl Roof {
    /// The roof for a player whose feet are at `feet`. A shulker stands on the block grid,
    /// and one hanging from the block above opens downwards by up to a block, so the block
    /// is the first one at or above the wanted height and the lid makes up the rest.
    fn over(feet: DVec3) -> Self {
        let bottom = feet.y + ROOF_ABOVE_FEET;
        let block = bottom.ceil();
        Self {
            x: floor(feet.x),
            y: floor(block),
            z: floor(feet.z),
            peek: peek_for(block - bottom),
        }
    }

    fn position(self) -> DVec3 {
        DVec3::new(
            f64::from(self.x) + 0.5,
            f64::from(self.y),
            f64::from(self.z) + 0.5,
        )
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "world coordinates fit an i32"
)]
const fn floor(value: f64) -> i32 {
    value.floor() as i32
}

/// The `peek` at which a shulker's box is `opened` blocks longer: the inverse of vanilla
/// `getPhysicalPeek`, `0.5 - sin((0.5 + peek) * PI) * 0.5`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the value is clamped to 0..=100"
)]
fn peek_for(opened: f64) -> i8 {
    let peek = (1.0 - 2.0 * opened.clamp(0.0, 1.0)).acos() / PI;
    (peek * 100.0).round().clamp(0.0, 100.0) as i8
}

/// One crawling player.
struct Crawl {
    /// The entity id of the shulker on the player's client.
    shulker: i32,
    data: ShulkerEntityData,
    roof: Roof,
    /// The world the shulker was sent in: a client forgets its entities on leaving it.
    world: usize,
    /// Whether the player sneaked on the last tick.
    sneaking: bool,
}

static CRAWLS: SyncMutex<Option<FxHashMap<Uuid, Crawl>>> = SyncMutex::new(None);

fn world_of(player: &Player) -> usize {
    Arc::as_ptr(&player.get_world()).addr()
}

/// Whether `player` crawls by this module.
fn is_crawling(player: &Player) -> bool {
    CRAWLS
        .lock()
        .as_ref()
        .is_some_and(|crawls| crawls.contains_key(&player.uuid()))
}

fn start_crawling(player: &Player) {
    let roof = Roof::over(player.position());
    let mut data = ShulkerEntityData::new();
    data.base_mut().shared_flags.set(INVISIBLE);
    data.base_mut().silent.set(true);
    data.attach_face.set(Direction::Up);
    data.peek.set(roof.peek);
    let shulker = next_entity_id();
    player.send_bundle(|bundle| {
        bundle.add(CAddEntity {
            id: shulker,
            uuid: Uuid::new_v4(),
            entity_type: vanilla_entities::SHULKER.id() as i32,
            position: roof.position(),
            velocity: DVec3::ZERO,
            x_rot: 0,
            y_rot: 0,
            head_y_rot: 0,
            data: 0,
        });
        bundle.add(CSetEntityData::new(shulker, data.pack_all()));
    });
    // Sent now, what is dirty later is only what changed since.
    let _ = data.pack_dirty();
    player.set_forced_pose(Some(EntityPose::Swimming));
    CRAWLS.lock().get_or_insert_default().insert(
        player.uuid(),
        Crawl {
            shulker,
            data,
            roof,
            world: world_of(player),
            sneaking: player.is_crouching(),
        },
    );
}

fn stop_crawling(player: &Player) {
    let crawl = CRAWLS
        .lock()
        .as_mut()
        .and_then(|crawls| crawls.remove(&player.uuid()));
    if let Some(crawl) = crawl {
        end(player, &crawl);
    }
}

fn end(player: &Player, crawl: &Crawl) {
    player.set_forced_pose(None);
    if crawl.world == world_of(player) {
        player.send_packet(CRemoveEntities::single(crawl.shulker));
    }
}

/// Moves the shulkers along and gets up the players who sneak or cannot crawl any more.
struct Crawling;

impl Crawling {
    /// `false` once the player stopped crawling.
    fn tick(player: &Player, crawl: &mut Crawl) -> bool {
        let sneaked = player.is_crouching() && !crawl.sneaking;
        crawl.sneaking = player.is_crouching();
        // A respawned player is a new one, without the pose.
        if sneaked
            || player.forced_pose() != Some(EntityPose::Swimming)
            || !sitting::may_sit(player)
            || crawl.world != world_of(player)
        {
            return false;
        }
        let roof = Roof::over(player.position());
        if roof == crawl.roof {
            return true;
        }
        crawl.roof = roof;
        crawl.data.peek.set(roof.peek);
        let peek = crawl.data.pack_dirty();
        let shulker = crawl.shulker;
        player.send_bundle(|bundle| {
            bundle.add(CEntityPositionSync {
                entity_id: shulker,
                path: PositionPath::Linear(roof.position()),
                y_rot: 0.0,
                x_rot: 0.0,
                on_ground: false,
            });
            if let Some(peek) = peek {
                bundle.add(CSetEntityData::new(shulker, peek));
            }
        });
        true
    }
}

impl ServerJob for Crawling {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        let Some(server) = context.server() else {
            return JobPoll::Finished;
        };
        let mut crawls = CRAWLS.lock();
        let Some(crawls) = crawls.as_mut().filter(|crawls| !crawls.is_empty()) else {
            return JobPoll::Pending;
        };
        let players = server.get_players();
        crawls.retain(|uuid, crawl| {
            let Some(player) = players.iter().find(|player| player.uuid() == *uuid) else {
                return false;
            };
            let goes_on = Self::tick(player, crawl);
            if !goes_on {
                end(player, crawl);
            }
            goes_on
        });
        JobPoll::Pending
    }
}

fn crawl_command(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let source = context.source();
    let Some(player) = source.player() else {
        return Err(CommandError::from("Ползать может только игрок."));
    };
    if is_crawling(player) {
        stop_crawling(player);
        return Ok(1);
    }
    if player.forced_pose().is_some() || !sitting::may_sit(player) || !player.on_ground() {
        return Err(CommandError::from("Здесь и сейчас не лечь."));
    }
    start_crawling(player);
    player.send_overlay_message(&render("&7Шифт, чтобы встать", &[]));
    Ok(1)
}

fn crawl_tree() -> CommandNode {
    literal("crawl").executes(|context| crawl_command(*context))
}

pub(super) fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "crawl"), crawl_tree)
            .default_access(),
    )?;
    Ok(())
}

/// Starts moving the shulkers of crawling players.
pub(super) fn start(server: &Server) {
    server.jobs.spawn(Crawling);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vanilla `getPhysicalPeek`.
    fn opened(peek: i8) -> f64 {
        0.5 - ((0.5 + f64::from(peek) / 100.0) * PI).sin() * 0.5
    }

    #[test]
    fn the_roof_leaves_room_to_crawl_only() {
        for feet in [64.0, 64.2, 64.45, 64.5, 64.55, 64.9375, -12.3, 70.999] {
            let roof = Roof::over(DVec3::new(0.5, feet, 0.5));
            let bottom = f64::from(roof.y) - opened(roof.peek);
            let headroom = bottom - feet;
            assert!(
                headroom < 1.5,
                "a sneaking player fits at {feet}: {headroom}"
            );
            assert!(
                headroom > 1.3,
                "no room to crawl up a slab at {feet}: {headroom}"
            );
        }
    }

    #[test]
    fn the_roof_is_over_the_block_of_the_player() {
        let roof = Roof::over(DVec3::new(-0.3, 64.0, 7.9));
        assert_eq!((roof.x, roof.y, roof.z), (-1, 66, 7));
        assert_eq!(roof.position(), DVec3::new(-0.5, 66.0, 7.5));
    }

    #[test]
    fn peek_is_the_inverse_of_how_far_a_shulker_opens() {
        assert_eq!(peek_for(0.0), 0);
        assert_eq!(peek_for(0.5), 50);
        assert_eq!(peek_for(1.0), 100);
        assert!((opened(peek_for(0.3)) - 0.3).abs() < 0.02);
    }
}
