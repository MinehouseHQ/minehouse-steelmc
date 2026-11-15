//! A player's very first join: one line in chat for everyone and a few waves of fireworks
//! around the newcomer (the Fabric `FirstJoin`).
//!
//! "First" means the player's stats have never counted a `leave_game`: the server adds one
//! on every disconnect, so anyone who has played before has at least 1.
//!
//! The rockets are for show: their blast hurts no one (the newcomer, players around, pets).

use std::{
    collections::{BTreeMap, BTreeSet},
    f64::consts::TAU,
    sync::Arc,
};

use glam::DVec3;
use steel_core::{
    entity::{Entity as _, SharedEntity, entities::FireworkRocketEntity, next_entity_id},
    player::Player,
    server::{
        Server,
        jobs::{JobPoll, ServerJob, ServerJobContext},
    },
};
use steel_registry::{
    data_components::vanilla_components::{
        FIREWORKS, FireworkExplosion, FireworkExplosionShape, Fireworks,
    },
    item_stack::ItemStack,
    stat::vanilla_stat_types,
    vanilla_custom_stats, vanilla_entities, vanilla_items,
};
use uuid::Uuid;

use crate::moderation::format::render;

/// The line everyone sees (the Fabric `first_join.message`).
const MESSAGE: &str = "&6✦ &fНа сервере новый игрок: &e{player}&f! Поприветствуем!";
/// How many waves of rockets (the Fabric `first_join.waves`).
const WAVES: u32 = 4;
/// Rockets in a wave (the Fabric `first_join.rockets`).
const ROCKETS: u32 = 6;
/// How far from the player the ring of rockets is, in blocks (the Fabric `first_join.radius`).
const RADIUS: f64 = 3.0;
/// Lets the client finish loading the world before the show.
const START_DELAY_TICKS: u64 = 40;
const WAVE_GAP_TICKS: u64 = 15;
const COLORS: [i32; 9] = [
    0x00FF_5555,
    0x00FF_AA00,
    0x00FF_FF55,
    0x0055_FF55,
    0x0055_FFFF,
    0x0055_55FF,
    0x00FF_55FF,
    0x00FF_FFFF,
    0x00A8_55F7,
];
const SHAPES: [FireworkExplosionShape; 4] = [
    FireworkExplosionShape::LargeBall,
    FireworkExplosionShape::SmallBall,
    FireworkExplosionShape::Star,
    FireworkExplosionShape::Burst,
];

/// Whether `player` has never left the server before.
fn is_first_join(player: &Player) -> bool {
    let leave_game = vanilla_stat_types::CUSTOM.get(&vanilla_custom_stats::LEAVE_GAME);
    player
        .stats()
        .iter()
        .all(|(stat, count)| *stat != leave_game || *count == 0)
}

fn pick<T: Copy>(of: &[T]) -> Option<T> {
    of.get(rand::random_range(0..of.len())).copied()
}

/// A rocket of one or two random stars.
fn rocket_item() -> ItemStack {
    let stars = rand::random_range(1..=2);
    let explosions = (0..stars)
        .filter_map(|_| {
            Some(FireworkExplosion::new(
                pick(&SHAPES)?,
                vec![pick(&COLORS)?],
                vec![pick(&COLORS)?],
                rand::random(),
                rand::random(),
            ))
        })
        .collect();
    let mut stack = ItemStack::new(&vanilla_items::FIREWORK_ROCKET);
    if let Ok(fireworks) = Fireworks::new(rand::random_range(1..=2), explosions) {
        stack.set(FIREWORKS, fireworks);
    }
    stack
}

/// Where the rockets of a wave around `center` start: an even ring turned by `turn`.
fn ring(center: DVec3, turn: f64) -> impl Iterator<Item = DVec3> {
    (0..ROCKETS).map(move |index| {
        let angle = turn + TAU * f64::from(index) / f64::from(ROCKETS);
        DVec3::new(
            center.x + angle.cos() * RADIUS,
            center.y + 0.5,
            center.z + angle.sin() * RADIUS,
        )
    })
}

/// A ring of rockets around the player. Returns how many were spawned.
fn launch_wave(player: &Player) -> usize {
    let world = player.get_world();
    ring(player.position(), rand::random::<f64>() * TAU)
        .filter(|position| {
            let rocket = FireworkRocketEntity::launched(
                &vanilla_entities::FIREWORK_ROCKET,
                next_entity_id(),
                *position,
                Arc::downgrade(&world),
                rocket_item(),
            );
            rocket.set_harmless(true);
            let entity: SharedEntity = Arc::new(rocket);
            world.try_add_entity(entity).is_ok()
        })
        .count()
}

/// A show still running: the tick of its next wave and the waves left.
struct Show {
    next_wave: u64,
    waves_left: u32,
}

/// Watches who comes online and runs the shows. A job of the server tick, never finished.
#[derive(Default)]
struct FirstJoin {
    online: BTreeSet<Uuid>,
    shows: BTreeMap<Uuid, Show>,
}

impl FirstJoin {
    fn joined(&mut self, server: &Server, player: &Player, tick: u64) {
        if !is_first_join(player) {
            return;
        }
        let name = player.gameprofile.name();
        log::info!("{name} joined for the first time");
        server.broadcast_system_chat(&render(MESSAGE, &[("player", name)]), None);
        self.shows.insert(
            player.gameprofile.id(),
            Show {
                next_wave: tick + START_DELAY_TICKS,
                waves_left: WAVES,
            },
        );
    }
}

impl ServerJob for FirstJoin {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        let Some(server) = context.server() else {
            return JobPoll::Finished;
        };
        let tick = context.tick_count;
        let players = server.get_players();
        let now: BTreeSet<Uuid> = players
            .iter()
            .map(|player| player.gameprofile.id())
            .collect();
        for player in &players {
            if !self.online.contains(&player.gameprofile.id()) {
                self.joined(&server, player, tick);
            }
        }
        self.online = now;

        // Whoever left takes the show along.
        self.shows.retain(|id, show| {
            let Some(player) = players.iter().find(|player| player.gameprofile.id() == *id) else {
                return false;
            };
            if show.next_wave > tick {
                return true;
            }
            launch_wave(player);
            show.next_wave = tick + WAVE_GAP_TICKS;
            show.waves_left = show.waves_left.saturating_sub(1);
            show.waves_left > 0
        });
        JobPoll::Pending
    }
}

/// Starts watching for newcomers.
pub(super) fn start(server: &Server) {
    server.jobs.spawn(FirstJoin::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wave_is_an_even_ring_around_the_player() {
        let center = DVec3::new(10.0, 64.0, -5.0);
        let ring: Vec<DVec3> = ring(center, 0.3).collect();
        assert_eq!(ring.len(), 6);
        for position in &ring {
            let flat = DVec3::new(position.x - center.x, 0.0, position.z - center.z);
            assert!((flat.length() - RADIUS).abs() < 1e-9);
            assert!((position.y - 64.5).abs() < 1e-9);
        }
        // Neighbors are a sixth of a turn apart: the side of a hexagon is its radius.
        assert!((ring[0].distance(ring[1]) - RADIUS).abs() < 1e-9);
    }

    #[test]
    fn a_rocket_carries_one_or_two_stars_and_flies_one_or_two() {
        steel_registry::init_vanilla_registry();
        for _ in 0..50 {
            let rocket = rocket_item();
            let fireworks = rocket.get(FIREWORKS).expect("a rocket has fireworks");
            assert!((1..=2).contains(&fireworks.explosions().len()));
            assert!((1..=2).contains(&fireworks.flight_duration()));
        }
    }
}
