//! Riding the metro: a cart that reaches a rail on a station block stops there, and a cart
//! sent to a station goes there without stopping, with the junctions on the way switched
//! for it. A port of the Fabric `Stations` and `Routes`. A train of coupled carts (see
//! [`trains`](super::trains)) rides as one: its front cart notices the stations, and what
//! the metro remembers of the train is kept with that cart.
//!
//! A cart stopped at a station is docked there: it is pulled to the middle of the station
//! rail and pushes do nothing, until it is sent on: `/metro <station>`, the countdown of a
//! hold, or its rider pressing forward. Where the network does not know another station
//! yet a cart with a rider counts down and leaves the same way, so the line gets learned.

use std::sync::Arc;

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_core::{
    behavior::InteractionResult,
    command::{
        CommandArgument, CommandContext, CommandError, CommandNode, CommandRegistration,
        CommandRegistrationError, CommandRegistry, argument, literal,
    },
    entity::{Entity, RemovalReason, entities::objects::vehicles::MinecartObserver},
    player::Player,
    world::World,
};
use steel_protocol::packets::game::SoundSource;
use steel_registry::{
    blocks::{
        block_state_ext::BlockStateExt as _,
        properties::{BlockStateProperties, RailShape},
    },
    sound_event::SoundEventRef,
    sound_events,
    vanilla_block_tags::BlockTag,
    vanilla_blocks,
};
use steel_utils::{
    BlockPos, BlockStateId, Direction, Identifier,
    locks::SyncMutex,
    types::{InteractionHand, UpdateFlags},
};
use text_components::{Modifier as _, TextComponent, format::Color};
use uuid::Uuid;

use super::{
    flight, metro, minecart_names,
    rail_graph::{self, Nav, Plan},
    speed, station_menu, stations,
    trains::{self, Decision, Moved},
};
use crate::moderation::format::render;

/// Speed given to a cart on departure, blocks a tick.
const DEPART_SPEED: f64 = 0.5;
/// Below this a cart counts as standing still, blocks a tick.
const STILL: f64 = 0.02;
/// Further than this from its station rail a docked cart lets go of it, blocks.
const DOCK_RANGE: f64 = 3.5;
/// Closer than this to the middle of the station rail a docked cart is put right on it.
const DOCK_SNAP: f64 = 0.25;
/// Fastest a docked cart is pulled to the middle of the station rail, blocks a tick.
const DOCK_PULL: f64 = 0.15;
/// This close to the middle of its station rail a docked cart has arrived.
const DOCK_SETTLED: f64 = 0.05;
/// How many rails short of a station a stopped cart is still pulled onto it.
const DOCK_REACH: i32 = 2;
/// How close a junction gets switched ahead of the cart, in blocks along the axes.
const SWITCH_AHEAD: u32 = 4;
/// How long a cart with a rider stands at a station it cannot be sent on from, in ticks.
const STOP_TICKS: i64 = 5 * 20;
/// A cart that has not ticked for this long is forgotten, in ticks.
const FORGET_AFTER: i64 = 1200;

/// The way a cart is on.
struct Route {
    destination: BlockPos,
    switches: Vec<(BlockPos, RailShape)>,
    nodes: Vec<BlockPos>,
    next: usize,
}

impl Route {
    fn new(plan: Plan) -> Self {
        Self {
            destination: plan.destination,
            switches: plan.switches,
            nodes: plan.nodes,
            next: 0,
        }
    }

    fn switch_at(&self, node: BlockPos) -> Option<RailShape> {
        self.switches
            .iter()
            .find_map(|&(at, shape)| (at == node).then_some(shape))
    }
}

/// A cart counting down at a station.
struct Hold {
    station: BlockPos,
    name: Option<String>,
    departure: DVec3,
    depart_at: i64,
    shown: i64,
}

/// What the metro remembers of a cart.
#[derive(Default)]
struct Cart {
    nav: Nav,
    /// The game time of its last tick.
    seen: i64,
    /// Where it was when its tick began.
    start: Option<DVec3>,
    /// How far it moved in its last tick.
    last_step: DVec3,
    /// The station block it stopped at last, until it is well away from it.
    last_station: Option<BlockPos>,
    /// The station rail it is docked at.
    dock: Option<BlockPos>,
    /// The cart of its train that is pulled to the middle of that rail.
    dock_anchor: Option<Uuid>,
    /// Whether its rider let go of forward since it docked.
    dock_armed: bool,
    route: Option<Route>,
    hold: Option<Hold>,
}

static CARTS: SyncMutex<Option<FxHashMap<Uuid, Cart>>> = SyncMutex::new(None);

/// Follows every minecart: learns the track from it and runs its stops.
pub struct Ride;

/// The observer given to `steel_core`.
pub static RIDE: Ride = Ride;

impl MinecartObserver for Ride {
    fn before_tick(&self, cart: &dyn Entity, world: &Arc<World>) {
        let driver = match trains::decide(cart, world) {
            Decision::Lone => None,
            Decision::Decided => return,
            Decision::Train(driver) => Some(driver),
        };
        let cart = driver.as_deref().unwrap_or(cart);
        {
            let mut carts = CARTS.lock();
            let state = carts
                .get_or_insert_default()
                .entry(cart.uuid())
                .or_default();
            state.seen = world.game_time();
            state.start = Some(cart.position());
            before_move(state, cart, world);
        }
        if driver.is_some()
            && let Some(turned) = trains::settle(cart, world)
        {
            hand_over(cart.uuid(), &*turned);
        }
    }

    fn ticked(&self, cart: &dyn Entity, world: &Arc<World>, rail: Option<BlockPos>) {
        match trains::moved(cart, world) {
            Moved::Lone | Moved::Driver => {}
            // A cart in the back of a train: the front one notices the stations.
            Moved::Follower => return,
            Moved::Turned(driver) => {
                hand_over(cart.uuid(), &*driver);
                return;
            }
        }
        let now = world.game_time();
        let mut carts_guard = CARTS.lock();
        let carts = carts_guard.get_or_insert_default();
        let state = carts.entry(cart.uuid()).or_default();
        if let Some(start) = state.start {
            state.last_step = cart.position() - start;
        }
        after_move(state, cart, world, rail);
        let step = state.last_step;
        if now % FORGET_AFTER == 0 {
            carts.retain(|_, state| (now - state.seen).abs() < FORGET_AFTER);
        }
        drop(carts_guard);
        if trains::carries_player(cart) {
            trains::smash_empty(cart, world, step);
        }
        station_menu::tick(cart, world);
    }

    fn interact(
        &self,
        cart: &dyn Entity,
        player: &Player,
        hand: InteractionHand,
    ) -> Option<InteractionResult> {
        minecart_names::interact(cart, player, hand)
            .or_else(|| trains::interact(cart, player, hand))
    }

    fn passes_through(&self, cart: &dyn Entity, other: &dyn Entity) -> bool {
        trains::passes_through(cart, other)
    }

    fn follows(&self, cart: &dyn Entity) -> bool {
        trains::follows(cart)
    }

    fn rolls_freely(&self, cart: &dyn Entity) -> bool {
        trains::is_linked(cart)
    }

    fn max_speed(&self, cart: &dyn Entity, vanilla: f64) -> f64 {
        speed::max_speed(cart, vanilla)
    }

    fn drive(&self, cart: &dyn Entity, world: &Arc<World>, pass: &mut dyn FnMut()) -> bool {
        speed::drive(cart, world, pass)
    }

    fn off_track(&self, cart: &dyn Entity, world: &Arc<World>) -> Option<f64> {
        flight::off_track(cart, world)
    }

    fn removed(&self, cart: &dyn Entity, reason: RemovalReason) {
        trains::removed(cart, reason);
        station_menu::cart_removed(cart);
    }
}

/// Is `cart` on the rail of a station it is to stop at? Fast carts end their substeps
/// there, so the end of the tick sees them arrive instead of jumping past.
pub(super) fn stops_at(cart: &dyn Entity, world: &World, rail: BlockPos) -> bool {
    let carts = CARTS.lock();
    let state = carts.as_ref().and_then(|carts| carts.get(&cart.uuid()));
    let last_station = state.and_then(|state| state.last_station);
    if state.is_some_and(|state| state.hold.is_some()) {
        return false;
    }
    if Some(rail.below()) == last_station {
        return false;
    }
    match state.and_then(|state| state.route.as_ref()) {
        // A train sent somewhere goes past every other station.
        Some(route) => rail == route.destination,
        None => rail_graph::is_station(world, rail),
    }
}

/// A fast cart moved a part of its tick. The track is followed rail by rail, so no rail
/// of a trip, junction or station is jumped over.
pub(super) fn passed(cart: &dyn Entity, world: &Arc<World>) {
    let Some(minecart) = cart.as_abstract_minecart() else {
        return;
    };
    let rail = minecart.current_block_pos_or_rail_below();
    if !world
        .get_block_state(rail)
        .get_block()
        .has_tag(&BlockTag::RAILS)
    {
        return;
    }
    let mut carts = CARTS.lock();
    let state = carts
        .get_or_insert_default()
        .entry(cart.uuid())
        .or_default();
    if state.hold.is_some() {
        return;
    }
    let learned = rail_graph::follow(world, &mut state.nav, rail);
    if learned.node {
        reached_node(state, cart, world, rail, learned.entry);
    }
    on_rail(state, world, rail);
}

/// A train turned around: what the metro remembers of it goes to the cart that drives it
/// now. That cart is on another rail, so its place on the track is found again.
fn hand_over(from: Uuid, driver: &dyn Entity) {
    let mut carts = CARTS.lock();
    let carts = carts.get_or_insert_default();
    let Some(mut state) = carts.remove(&from) else {
        return;
    };
    state.nav.reset();
    state.start = Some(driver.position());
    carts.insert(driver.uuid(), state);
}

const fn horizontal(vector: DVec3) -> DVec3 {
    DVec3::new(vector.x, 0.0, vector.z)
}

fn bottom_center(pos: BlockPos) -> DVec3 {
    DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()),
        f64::from(pos.z()) + 0.5,
    )
}

fn is_rail(state: BlockStateId) -> bool {
    state.get_block().has_tag(&BlockTag::RAILS)
}

fn set_horizontal(cart: &dyn Entity, speed: DVec3) {
    cart.set_velocity(DVec3::new(speed.x, cart.velocity().y, speed.z));
}

/// Runs `action` for every player riding the cart or another cart of its train.
pub(super) fn riders(cart: &dyn Entity, mut action: impl FnMut(&Player)) {
    let mut aboard = |cart: &dyn Entity| {
        for passenger in cart.passengers() {
            if let Some(player) = passenger.as_player() {
                action(player);
            }
        }
    };
    aboard(cart);
    for other in trains::others(cart) {
        aboard(&*other);
    }
}

fn has_riders(cart: &dyn Entity) -> bool {
    let mut any = false;
    riders(cart, |_| any = true);
    any
}

fn announce(cart: &dyn Entity, text: &str) {
    riders(cart, |player| {
        player.send_overlay_message(&TextComponent::plain(text.to_owned()));
    });
}

fn title(cart: &dyn Entity, name: Option<&str>, subtitle: &str) {
    let title = name.map_or_else(|| "Станция".to_owned(), |name| format!("Станция «{name}»"));
    riders(cart, |player| {
        player.send_title_times(5, 50, 10);
        player.send_subtitle(TextComponent::plain(subtitle.to_owned()));
        player.send_title(TextComponent::plain(title.clone()));
    });
}

fn sound(world: &World, sound: SoundEventRef, pos: BlockPos, pitch: f32) {
    world.play_sound(sound, SoundSource::Neutral, pos, 1.5, pitch, None);
}

fn station_name(world: &World, station: BlockPos) -> Option<String> {
    stations::with(|stations| {
        stations.add(&world.key, station);
        stations
            .get(&world.key, station)
            .and_then(|s| s.name.clone())
    })
}

fn reachable(world: &World, rail: BlockPos) -> Vec<(BlockPos, u32)> {
    rail_graph::with(|graphs| {
        graphs
            .of(&world.key)
            .reachable_stations(rail, |pos| rail_graph::kind(world, pos))
    })
}

/// An unpowered powered rail on the station closes it.
fn open(world: &World, station: BlockPos) -> bool {
    let rail = world.get_block_state(station.above());
    rail.get_block() != &vanilla_blocks::POWERED_RAIL
        || rail.get_value(&BlockStateProperties::POWERED)
}

/// Start of a cart's tick: a held or docked cart is kept at its station, a departing one
/// gets its speed.
fn before_move(state: &mut Cart, cart: &dyn Entity, world: &Arc<World>) {
    let now = world.game_time();
    if let Some(hold) = &mut state.hold {
        if now < hold.depart_at {
            let left = (hold.depart_at - now + 19) / 20;
            if left != hold.shown {
                hold.shown = left;
                let place = hold
                    .name
                    .as_ref()
                    .map_or_else(|| "Станция".to_owned(), |name| format!("Станция «{name}»"));
                announce(cart, &format!("{place} · отправление через {left} с"));
            }
        } else if open(world, hold.station) {
            let go = hold.departure;
            sound(
                world,
                &sound_events::BLOCK_NOTE_BLOCK_CHIME,
                hold.station.above(),
                0.8,
            );
            announce(cart, "Осторожно, двери закрываются!");
            state.hold = None;
            state.dock = None;
            set_horizontal(cart, go);
            return;
        } else if hold.shown != 0 {
            hold.shown = 0;
            announce(cart, "Станция закрыта, ждём отправления");
        }
    }
    if let Some(rail) = state.dock {
        let anchor = state
            .dock_anchor
            .filter(|&anchor| anchor != cart.uuid())
            .and_then(|anchor| world.get_entity_by_uuid(&anchor));
        let drives = anchor.is_none();
        let anchor = anchor.as_deref().unwrap_or(cart);
        if still_docked(state, cart, anchor, world, rail) {
            set_horizontal(cart, pull(anchor, rail, drives));
            return;
        }
        state.dock = None;
        state.dock_anchor = None;
    }
    if state.hold.is_some() {
        set_horizontal(cart, DVec3::ZERO);
    }
}

/// Whether the dock of the cart is still there. A rider pressing forward after the cart
/// stopped sends it on by hand, the way a line not known yet gets explored; holding it
/// from before does not.
fn still_docked(
    state: &mut Cart,
    cart: &dyn Entity,
    anchor: &dyn Entity,
    world: &World,
    rail: BlockPos,
) -> bool {
    let there = state.route.is_none()
        && is_rail(world.get_block_state(rail))
        && rail_graph::is_station(world, rail)
        && horizontal(bottom_center(rail) - anchor.position()).length() < DOCK_RANGE;
    if !there {
        return false;
    }
    let mut forward = false;
    riders(cart, |player| {
        forward |= player.last_client_input().forward();
    });
    if !forward {
        state.dock_armed = true;
    }
    !(forward && state.dock_armed)
}

/// Towards the middle of the station rail, slowing down near it. Close to it a cart that
/// moves by itself (`drives`) is put right there: a powered rail would rock it back and
/// forth.
fn pull(cart: &dyn Entity, rail: BlockPos, drives: bool) -> DVec3 {
    let position = cart.position();
    let to = horizontal(bottom_center(rail) - position);
    let distance = to.length();
    if distance < DOCK_SNAP {
        if drives && distance > 1.0e-3 {
            let middle = bottom_center(rail);
            drop(cart.teleport_to(DVec3::new(middle.x, position.y, middle.z)));
        }
        return DVec3::ZERO;
    }
    to * (DOCK_PULL.min(distance) / distance)
}

fn dock(state: &mut Cart, cart: &dyn Entity, rail: BlockPos) {
    state.dock = Some(rail);
    state.dock_anchor = Some(cart.uuid());
    state.dock_armed = false;
    state.last_station = Some(rail.below());
    set_horizontal(cart, DVec3::ZERO);
}

/// End of a cart's tick: learns the track, switches junctions ahead, and sees whether the
/// cart just reached a station.
fn after_move(state: &mut Cart, cart: &dyn Entity, world: &Arc<World>, rail: Option<BlockPos>) {
    if state.hold.is_some() {
        return;
    }
    match rail {
        Some(pos) => {
            let learned = rail_graph::follow(world, &mut state.nav, pos);
            if learned.node {
                reached_node(state, cart, world, pos, learned.entry);
            }
            on_rail(state, world, pos);
        }
        None => state.nav.reset(),
    }
    if state.dock.is_some() {
        return;
    }
    let position = cart.position();
    let rail_pos = rail.unwrap_or_else(|| BlockPos::containing(position.x, position.y, position.z));
    if let Some(last) = state.last_station
        && last.above() != rail_pos
        && (bottom_center(last) + DVec3::new(0.0, 0.5, 0.0)).distance_squared(position) > 9.0
    {
        state.last_station = None;
    }

    let rail_state = world.get_block_state(rail_pos);
    let station = rail_pos.below();
    let step = horizontal(state.last_step);
    let moving = step.length() > STILL;
    if !moving
        && state.route.is_none()
        && is_rail(rail_state)
        && (world.game_time() + i64::from(cart.id())) % 10 == 0
        && let Some(near) = near_station(world, rail_pos, rail_state, state.last_station)
        && !reachable(world, near).is_empty()
    {
        // Standing on a station or just short of one (a cart put there, one pushed in by
        // hand): it docks there.
        dock(state, cart, near);
        return;
    }
    if !is_rail(rail_state)
        || Some(station) == state.last_station
        || !rail_graph::is_station(world, rail_pos)
    {
        return;
    }
    if let Some(route) = &state.route {
        if rail_pos == route.destination {
            arrive_routed(state, cart, world, rail_pos);
        }
        return;
    }
    if moving {
        arrive(state, cart, world, rail_pos, rail_state, step.normalize());
    }
}

/// The station rail a cart standing on `rail_pos` docks at: that one, or one at most
/// [`DOCK_REACH`] rails further along the track. Never `left`, the station it just left.
fn near_station(
    world: &World,
    rail_pos: BlockPos,
    rail: BlockStateId,
    left: Option<BlockPos>,
) -> Option<BlockPos> {
    let is_station_rail = |pos: BlockPos| {
        Some(pos.below()) != left
            && is_rail(world.get_block_state(pos))
            && rail_graph::is_station(world, pos)
    };
    if is_station_rail(rail_pos) {
        return Some(rail_pos);
    }
    let shape = rail.get_value(&BlockStateProperties::RAIL_SHAPE);
    for (x, z) in rail_graph::exits(shape) {
        for reach in 1..=DOCK_REACH {
            let at = rail_pos.offset(x * reach, 0, z * reach);
            if is_station_rail(at) {
                return Some(at);
            }
            if !is_rail(world.get_block_state(at)) {
                break;
            }
        }
    }
    None
}

/// Whether there is more track past this rail, going `forward`.
fn continues(world: &World, rail_pos: BlockPos, rail: BlockStateId, forward: DVec3) -> bool {
    let shape = rail.get_value(&BlockStateProperties::RAIL_SHAPE);
    let mut best = None;
    let mut best_dot = 0.0;
    for (x, z) in rail_graph::exits(shape) {
        let dot = f64::from(x) * forward.x + f64::from(z) * forward.z;
        if dot > best_dot {
            best_dot = dot;
            best = Some((x, z));
        }
    }
    best.is_some_and(|(x, z)| {
        (-1..=1).any(|dy| is_rail(world.get_block_state(rail_pos.offset(x, dy, z))))
    })
}

/// A cart that was not sent anywhere reached a station rail going `forward`.
fn arrive(
    state: &mut Cart,
    cart: &dyn Entity,
    world: &Arc<World>,
    rail_pos: BlockPos,
    rail: BlockStateId,
    forward: DVec3,
) {
    let station = rail_pos.below();
    let network = !reachable(world, rail_pos).is_empty();
    let name = station_name(world, station);
    metro::sync(world, station);
    state.last_station = Some(station);
    set_horizontal(cart, DVec3::ZERO);
    sound(world, &sound_events::BLOCK_NOTE_BLOCK_BELL, rail_pos, 1.0);
    if !has_riders(cart) || network {
        // The cart stops here. With a rider it waits to be sent on; an empty cart never
        // goes anywhere by itself. With no other station known yet it is left free, so a
        // push can take it on and learn the line.
        if network {
            dock(state, cart, rail_pos);
        }
        title(cart, name.as_deref(), "Куда ехать: /metro <станция>");
        return;
    }
    let terminus = !continues(world, rail_pos, rail, forward);
    let sign = if terminus { -1.0 } else { 1.0 };
    dock(state, cart, rail_pos);
    title(
        cart,
        name.as_deref(),
        if terminus {
            "Конечная. Поезд дальше не идёт"
        } else {
            ""
        },
    );
    state.hold = Some(Hold {
        station,
        name,
        departure: forward * (sign * DEPART_SPEED),
        depart_at: world.game_time() + STOP_TICKS,
        shown: -1,
    });
}

/// The cart got to the station rail it was sent to: it stops there.
fn arrive_routed(state: &mut Cart, cart: &dyn Entity, world: &Arc<World>, rail: BlockPos) {
    state.route = None;
    dock(state, cart, rail);
    let name = station_name(world, rail.below());
    metro::sync(world, rail.below());
    sound(world, &sound_events::BLOCK_NOTE_BLOCK_BELL, rail, 1.0);
    title(cart, name.as_deref(), "Вы приехали");
}

fn set_shape(world: &Arc<World>, node: BlockPos, shape: RailShape) {
    let state = world.get_block_state(node);
    if state.get_block() == &vanilla_blocks::RAIL
        && state.get_value(&BlockStateProperties::RAIL_SHAPE) != shape
    {
        // Only the shape: no neighbour updates, so redstone next to it does not flip it
        // back at once.
        world.set_block(
            node,
            state.set_value(&BlockStateProperties::RAIL_SHAPE, shape),
            UpdateFlags::UPDATE_CLIENTS,
        );
    }
}

/// A routed cart is on `pos`: switches the next junction if it is close, and the one it
/// stands on.
fn on_rail(state: &Cart, world: &Arc<World>, pos: BlockPos) {
    let Some(route) = &state.route else {
        return;
    };
    for &node in route.nodes.iter().skip(route.next).take(2) {
        let distance = (node.0 - pos.0).abs().element_sum().unsigned_abs();
        if let Some(shape) = route.switch_at(node)
            && distance <= SWITCH_AHEAD
        {
            set_shape(world, node, shape);
        }
    }
    if let Some(shape) = route.switch_at(pos) {
        set_shape(world, pos, shape);
    }
}

/// A cart got onto a node of the network, through side `entry`.
fn reached_node(
    state: &mut Cart,
    cart: &dyn Entity,
    world: &Arc<World>,
    node: BlockPos,
    entry: Option<Direction>,
) {
    let Some(route) = &mut state.route else {
        return;
    };
    if node == route.destination {
        return;
    }
    if let Some(at) = route.nodes.iter().position(|&known| known == node) {
        if at >= route.next {
            route.next = at + 1;
        }
        return;
    }
    let Some(entry) = entry else {
        return;
    };
    // Off the planned way (a junction did not switch, the track changed): plan again.
    let destination = route.destination;
    let plan = rail_graph::with(|graphs| {
        graphs
            .of(&world.key)
            .plan(node, Some(entry), destination, |pos| {
                rail_graph::kind(world, pos)
            })
    });
    if let Some(plan) = plan {
        state.route = Some(Route::new(plan));
        on_rail(state, world, node);
    } else {
        state.route = None;
        announce(cart, "Маршрут потерян: отсюда до станции не проехать");
    }
}

/// The rail under a cart: the one it is docked to, or the one it stands on.
fn rail_of(cart: &dyn Entity, world: &World, docked: Option<BlockPos>) -> BlockPos {
    let position = cart.position();
    let mut rail = BlockPos::containing(position.x, position.y, position.z);
    if is_rail(world.get_block_state(rail.below())) {
        rail = rail.below();
    }
    docked.unwrap_or(rail)
}

/// The station rail `cart` (the front one of its train) stands still on, when it may be
/// sent somewhere from there: it is not on its way anywhere and not counting down.
pub(super) fn standing_at(cart: &dyn Entity, world: &World) -> Option<BlockPos> {
    if !cart.is_on_rails() {
        return None;
    }
    let docked = {
        let carts = CARTS.lock();
        let state = carts.as_ref().and_then(|carts| carts.get(&cart.uuid()));
        if state.is_some_and(|state| state.route.is_some() || state.hold.is_some()) {
            return None;
        }
        state.and_then(|state| state.dock)
    };
    if horizontal(cart.velocity()).length() > STILL {
        return None;
    }
    // A docked cart is first pulled to the middle of its rail; the map hangs in front of
    // where the rider is, so it waits for the cart to get there.
    if docked.is_some_and(|rail| {
        horizontal(bottom_center(rail) - cart.position()).length() > DOCK_SETTLED
    }) {
        return None;
    }
    let rail = rail_of(cart, world, docked);
    (is_rail(world.get_block_state(rail)) && rail_graph::is_station(world, rail)).then_some(rail)
}

/// Sends `cart` from station rail `from` to station rail `destination`, as its rider
/// chose on the map. `false` if the network as known has no way there.
pub(super) fn send(
    cart: &dyn Entity,
    world: &Arc<World>,
    from: BlockPos,
    destination: BlockPos,
) -> bool {
    depart(cart, world, from, destination)
}

/// Sends `cart`, standing on station rail `from`, to the station rail `destination`.
/// `false` if the network as known has no way there.
fn depart(cart: &dyn Entity, world: &Arc<World>, from: BlockPos, destination: BlockPos) -> bool {
    let plan = rail_graph::with(|graphs| {
        graphs
            .of(&world.key)
            .plan(from, None, destination, |pos| rail_graph::kind(world, pos))
    });
    let Some(plan) = plan else {
        return false;
    };
    let (x, z) = plan.start.offset_xz();
    let mut carts = CARTS.lock();
    let state = carts
        .get_or_insert_default()
        .entry(cart.uuid())
        .or_default();
    state.route = Some(Route::new(plan));
    // Let go of the station first, or the speed would be taken for a push and ignored.
    state.dock = None;
    state.dock_anchor = None;
    state.hold = None;
    state.last_station = Some(from.below());
    set_horizontal(
        cart,
        DVec3::new(f64::from(x), 0.0, f64::from(z)) * DEPART_SPEED,
    );
    on_rail(state, world, from);
    sound(world, &sound_events::BLOCK_NOTE_BLOCK_CHIME, from, 0.8);
    true
}

/// Registers `/metro`, for every player.
pub(super) fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "metro"), tree).default_access(),
    )?;
    Ok(())
}

fn tree() -> CommandNode {
    literal("metro").executes(|context| list(*context)).then(
        argument("station", CommandArgument::greedy_string())
            .executes(|context| go(*context, context.string("station")?)),
    )
}

fn error(text: &str) -> CommandError {
    CommandError::from(text.to_owned())
}

/// The cart the player rides, the world, the station rail it stands on, and the stations
/// that can be reached from there with their names.
type Standing = (
    Arc<dyn Entity>,
    Arc<World>,
    BlockPos,
    Vec<(BlockPos, String, u32)>,
);

fn standing(context: CommandContext<'_>) -> Result<Standing, CommandError> {
    let Some(player) = context.source().player() else {
        return Err(error("Команда только для игрока."));
    };
    let cart = player
        .vehicle()
        .ok_or_else(|| error("Сядь в вагонетку на станции метро."))?;
    // In a train the cart in front is the one that stands at the station.
    let cart = trains::driver_of(&*cart).unwrap_or(cart);
    let world = cart
        .level()
        .ok_or_else(|| error("Вагонетка не в мире."))?
        .into_arc();
    let docked = CARTS
        .lock()
        .as_ref()
        .and_then(|carts| carts.get(&cart.uuid()))
        .and_then(|state| state.dock);
    let rail = rail_of(&*cart, &world, docked);
    if !is_rail(world.get_block_state(rail)) || !rail_graph::is_station(&world, rail) {
        return Err(error("Вагонетка должна стоять на станции метро."));
    }
    let named = reachable(&world, rail)
        .into_iter()
        .filter_map(|(at, distance)| {
            let name = stations::with(|stations| {
                stations
                    .get(&world.key, at.below())
                    .and_then(|s| s.name.clone())
            })?;
            Some((at, name, distance))
        })
        .collect();
    Ok((cart, world, rail, named))
}

fn list(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let (_, _, _, named) = standing(context)?;
    if named.is_empty() {
        return Err(error(
            "Отсюда пока никуда не доехать: до других станций с названием ещё никто не ездил.",
        ));
    }
    let listed: Vec<String> = named
        .iter()
        .map(|(_, name, distance)| format!("{name} ({distance} м)"))
        .collect();
    context.source().send_success(
        &render(
            "&7Отсюда можно доехать: &f{stations}&7. Команда: &f/metro <станция>",
            &[("stations", &listed.join(", "))],
        ),
        false,
    );
    Ok(i32::try_from(named.len()).unwrap_or(i32::MAX))
}

fn go(context: CommandContext<'_>, wanted: &str) -> Result<i32, CommandError> {
    let (cart, world, rail, named) = standing(context)?;
    let wanted = wanted.trim().to_lowercase();
    let found = named
        .iter()
        .find(|(_, name, _)| name.to_lowercase() == wanted)
        .or_else(|| {
            named
                .iter()
                .find(|(_, name, _)| name.to_lowercase().starts_with(&wanted))
        });
    let Some((destination, name, _)) = found else {
        return Err(error("Такой станции отсюда не видно. Список: /metro"));
    };
    if !depart(&*cart, &world, rail, *destination) {
        return Err(error("До этой станции отсюда не проехать."));
    }
    riders(&*cart, |player| {
        player.send_overlay_message(
            &TextComponent::plain(format!("Едем до станции «{name}»")).color(Color::Yellow),
        );
    });
    Ok(1)
}
