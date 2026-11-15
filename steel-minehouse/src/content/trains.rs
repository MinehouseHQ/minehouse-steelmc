//! Minecart trains: carts coupled with a chain move as one. A port of the Fabric `Trains`
//! and `Consist`.
//!
//! Sneak and use a chain on one cart, then on another next to it. Each cart couples to at
//! most two others, so a train is a line that can be pushed or pulled from either end;
//! that is what lets a train turn back at a terminus. Sneak and use shears to uncouple.
//!
//! Only the cart in front, the driver, has physics: the server moves it along its rail as
//! a lone cart. Every other cart is put on the driver's trail, the way the driver went, at
//! a fixed distance behind the one before it. So a train cannot stretch, bunch up or
//! derail cart by cart, and a junction switched after the front passed it does not send
//! the rear somewhere else. Whichever cart gets a push gives it to the whole train. A
//! train pushed backwards turns around: its rear cart becomes the driver.
//!
//! Not ported: a train with a player knocking empty carts off the line.
//!
//! A cart behind the driver has no pass along the track of its own (see
//! [`speed`](super::speed)).

use std::{collections::VecDeque, str::FromStr as _, sync::Arc};

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_core::{
    behavior::InteractionResult,
    entity::{Entity, RemovalReason},
    player::Player,
    world::World,
};
use steel_protocol::packets::game::SoundSource;
use steel_registry::{
    REGISTRY, RegistryExt as _, item_stack::ItemStack, sound_event::SoundEventRef, sound_events,
    vanilla_entities, vanilla_items,
};
use steel_utils::{BlockPos, Identifier, locks::SyncMutex, types::InteractionHand};
use uuid::Uuid;

use super::{couplings::Couplings, metro};
use crate::config;

/// Center to center, in blocks. A cart is a bit under a block long.
const SPACING: f64 = 1.5;
/// A coupling stretched past this (a derailment, a teleport) breaks and drops its chain.
const BREAK_DISTANCE: f64 = 6.0;
/// The second cart must be picked within this many ticks of the first.
const SELECT_TICKS: i64 = 30 * 20;
/// How far apart two carts can be coupled, blocks.
const LINK_REACH: f64 = 4.0;
/// The most carts in one train.
const MAX_CARTS: usize = 12;
/// Slower than this a cart counts as standing, blocks a tick.
const MOVING: f64 = 0.01;
/// Points of the trail closer than this to the last one are not kept, blocks.
const MIN_POINT: f64 = 0.05;
/// How fast a gap between two carts goes to [`SPACING`] while moving, blocks a tick.
const SETTLE: f64 = 0.05;

const fn horizontal(vector: DVec3) -> DVec3 {
    DVec3::new(vector.x, 0.0, vector.z)
}

fn set_horizontal(cart: &dyn Entity, speed: DVec3) {
    cart.set_velocity(DVec3::new(speed.x, cart.velocity().y, speed.z));
}

/// A coupled train moving as one.
struct Consist {
    /// Front (the driver) to back.
    carts: Vec<Uuid>,
    /// Center to center along the trail, between each cart and the one behind it.
    gaps: Vec<f64>,
    /// Where the driver went, newest first, the driver's own place not included.
    trail: VecDeque<DVec3>,
    /// The speed every cart was left with last tick: anything else is a push.
    assigned: DVec3,
    /// The game time its speed was last decided at.
    decided_at: i64,
    /// Where the driver was when that was decided.
    from: DVec3,
}

/// The polyline from its start to `length` along it.
fn cut(path: &[DVec3], length: f64) -> Vec<DVec3> {
    let mut out: Vec<DVec3> = path.first().copied().into_iter().collect();
    let mut left = length;
    for pair in path.windows(2) {
        let &[from, to] = pair else {
            break;
        };
        if left <= 1.0e-6 {
            break;
        }
        let distance = from.distance(to);
        if distance >= left {
            out.push(from + (to - from) * (left / distance));
            return out;
        }
        out.push(to);
        left -= distance;
    }
    out
}

impl Consist {
    /// The rear cart becomes the driver; the trail is the way back along the train.
    /// `driver_at` is where the driver is.
    fn reverse(&mut self, driver_at: DVec3) {
        let mut path = vec![driver_at];
        path.extend(self.trail.iter().copied());
        let length: f64 = self.gaps.iter().sum();
        let mut back = cut(&path, length);
        back.reverse();
        // The first point is where the new driver is.
        self.trail = back.into_iter().skip(1).collect();
        self.carts.reverse();
        self.gaps.reverse();
    }

    /// Puts where the driver is now on the trail.
    fn record(&mut self, driver_at: DVec3) {
        let far = self
            .trail
            .front()
            .is_none_or(|last| last.distance_squared(driver_at) > MIN_POINT * MIN_POINT);
        if far {
            self.trail.push_front(driver_at);
        }
    }

    /// Lets the gaps go towards [`SPACING`] while the train moves. Gives their sum.
    fn settle_gaps(&mut self, moving: bool) -> f64 {
        let mut needed = 0.0;
        for gap in &mut self.gaps {
            if moving {
                *gap += (SPACING - *gap).clamp(-SETTLE, SETTLE);
            }
            needed += *gap;
        }
        needed
    }

    /// Where every cart behind the driver goes: on the trail, each its gap behind the one
    /// before it. `driver_at` is where the driver is.
    fn spots(&self, driver_at: DVec3) -> Vec<DVec3> {
        let mut points = self.trail.iter().copied();
        let mut from = driver_at;
        let mut to = points.next().unwrap_or(from);
        let mut walked = 0.0;
        let mut target = 0.0;
        let mut spots = Vec::with_capacity(self.gaps.len());
        for gap in &self.gaps {
            target += gap;
            let spot = loop {
                let distance = from.distance(to);
                if walked + distance >= target && distance > 1.0e-9 {
                    break from + (to - from) * ((target - walked) / distance);
                }
                let Some(next) = points.next() else {
                    // Trail too short (just built from carts on a curve): where it ends.
                    break to;
                };
                walked += distance;
                from = to;
                to = next;
            };
            spots.push(spot);
        }
        spots
    }

    /// Forgets the trail past the last cart.
    fn trim(&mut self, driver_at: DVec3, needed: f64) {
        let mut kept = 0.0;
        let mut previous = driver_at;
        let mut keep = 0;
        for &point in &self.trail {
            keep += 1;
            kept += previous.distance(point);
            previous = point;
            if kept > needed + 2.0 {
                break;
            }
        }
        self.trail.truncate(keep);
    }
}

/// A cart a player picked to couple, and until when.
struct Selection {
    cart: Uuid,
    until: i64,
}

/// The couplings and the trains made of them.
struct Trains {
    couplings: Couplings,
    consists: FxHashMap<u32, Consist>,
    /// The consist each cart moves with.
    member: FxHashMap<Uuid, u32>,
    next: u32,
    selected: FxHashMap<Uuid, Selection>,
}

static TRAINS: SyncMutex<Option<Trains>> = SyncMutex::new(None);

/// Runs `action` on the trains of the server, reading the couplings on the first use.
fn with<T>(action: impl FnOnce(&mut Trains) -> T) -> T {
    let mut trains = TRAINS.lock();
    let trains = trains.get_or_insert_with(|| Trains {
        couplings: Couplings::load(config::data_path("metro-couplings.json")),
        consists: FxHashMap::default(),
        member: FxHashMap::default(),
        next: 0,
        selected: FxHashMap::default(),
    });
    action(trains)
}

/// How many carts have a coupling.
pub(super) fn count() -> usize {
    with(|trains| trains.couplings.count())
}

/// The cart with this id, when it is in `world` and alive.
fn resolve(world: &World, id: Uuid) -> Option<Arc<dyn Entity>> {
    world
        .get_entity_by_uuid(&id)
        .filter(|entity| !entity.is_removed() && entity.as_abstract_minecart().is_some())
}

fn entities(world: &World, carts: &[Uuid]) -> Option<Vec<Arc<dyn Entity>>> {
    carts.iter().map(|&id| resolve(world, id)).collect()
}

/// From the second cart to the driver, flat.
fn forward(carts: &[Arc<dyn Entity>]) -> DVec3 {
    let (Some(driver), Some(second)) = (carts.first(), carts.get(1)) else {
        return DVec3::ZERO;
    };
    horizontal(driver.position() - second.position()).normalize_or_zero()
}

fn has_player(cart: &dyn Entity) -> bool {
    cart.passengers()
        .iter()
        .any(|passenger| passenger.as_player().is_some())
}

fn drop_chain(cart: &dyn Entity, chain: &str) {
    let item = Identifier::from_str(chain)
        .ok()
        .and_then(|key| REGISTRY.items.by_key(&key));
    if let Some(item) = item {
        drop(cart.spawn_at_location(ItemStack::new(item), 0.0));
    }
}

impl Trains {
    /// Every cart in `world` coupled to `cart`, directly or through others, `cart` first.
    fn train(&self, world: &World, cart: Uuid) -> Vec<Uuid> {
        self.couplings
            .train(cart, MAX_CARTS * 2, |id| resolve(world, id).is_some())
    }

    /// The couplings of these carts changed: their trains are worked out again.
    fn invalidate(&mut self, carts: &[Uuid]) {
        for cart in carts {
            let Some(id) = self.member.remove(cart) else {
                continue;
            };
            if let Some(consist) = self.consists.remove(&id) {
                for other in consist.carts {
                    self.member.remove(&other);
                }
            }
        }
    }

    /// Still the same carts, all of them here?
    fn valid(&self, world: &World, id: u32) -> bool {
        let Some(consist) = self.consists.get(&id) else {
            return false;
        };
        let here = consist
            .carts
            .iter()
            .all(|&cart| self.member.get(&cart) == Some(&id) && resolve(world, cart).is_some());
        here && consist
            .carts
            .first()
            .is_some_and(|&driver| self.train(world, driver).len() == consist.carts.len())
    }

    /// The consist `cart` is part of, built if needed; `None` for a lone cart.
    fn consist(&mut self, world: &World, cart: Uuid, now: i64) -> Option<u32> {
        if !self.couplings.is_linked(cart) {
            self.invalidate(&[cart]);
            return None;
        }
        if let Some(&id) = self.member.get(&cart) {
            let decided = self
                .consists
                .get(&id)
                .is_some_and(|consist| consist.decided_at == now);
            if decided || self.valid(world, id) {
                return Some(id);
            }
            self.invalidate(&[cart]);
        }
        self.build(world, cart)
    }

    /// The train of `any` as a consist, or `None` for a lone cart or a coupling that broke.
    fn build(&mut self, world: &World, any: Uuid) -> Option<u32> {
        let all = self.train(world, any);
        if all.len() < 2 {
            return None;
        }
        // A train is a line: walk it from one end.
        let mut order = self.couplings.line(&all)?;
        let mut carts = entities(world, &order)?;
        for (pair, ids) in carts.windows(2).zip(order.windows(2)) {
            let ([first, second], &[first_id, second_id]) = (pair, ids) else {
                continue;
            };
            if first.position().distance(second.position()) > BREAK_DISTANCE {
                // Derailed or teleported apart: the coupling breaks, the parts are trains
                // of their own.
                let chain = self
                    .couplings
                    .unlink(first_id, second_id)
                    .and_then(|broken| broken.chain);
                self.invalidate(&all);
                if let Some(chain) = chain {
                    drop_chain(&**first, &chain);
                }
                return None;
            }
        }

        // The end it moves towards is the front.
        let speed: DVec3 = carts.iter().map(|cart| horizontal(cart.velocity())).sum();
        if speed.dot(forward(&carts)) < 0.0 {
            order.reverse();
            carts.reverse();
        }
        let places: Vec<DVec3> = carts.iter().map(|cart| cart.position()).collect();
        let gaps = places
            .windows(2)
            .map(|pair| match pair {
                &[first, second] => first.distance(second).max(0.8),
                _ => SPACING,
            })
            .collect();
        let driver = carts.first()?;
        let consist = Consist {
            gaps,
            trail: places.iter().skip(1).copied().collect(),
            assigned: horizontal(driver.velocity()),
            decided_at: i64::MIN,
            from: driver.position(),
            carts: order,
        };
        let id = self.next;
        self.next = self.next.wrapping_add(1);
        self.invalidate(&all);
        for &cart in &consist.carts {
            self.member.insert(cart, id);
        }
        self.consists.insert(id, consist);
        Some(id)
    }

    /// Carts of one train, coupled directly or not. At speed a rear cart, moved before
    /// the ones in front of it in the same tick, can reach where they still stand.
    fn same_train(&self, world: &World, first: Uuid, second: Uuid) -> bool {
        if !self.couplings.is_linked(first) || !self.couplings.is_linked(second) {
            return false;
        }
        self.couplings.are_linked(first, second) || self.train(world, first).contains(&second)
    }

    /// Whether a player rides `cart` or any cart of its train.
    fn carries_player(&self, world: &World, cart: &dyn Entity) -> bool {
        if has_player(cart) {
            return true;
        }
        if !self.couplings.is_linked(cart.uuid()) {
            return false;
        }
        self.train(world, cart.uuid())
            .into_iter()
            .skip(1)
            .filter_map(|id| resolve(world, id))
            .any(|other| has_player(&*other))
    }
}

/// What a cart is to its train at the start of its tick.
pub(super) enum Decision {
    /// Not coupled, or its train is not all here: it moves by itself.
    Lone,
    /// Its train got its speed earlier this tick.
    Decided,
    /// The first cart of its train to tick. The pushes on all of them went to this cart,
    /// the driver; [`settle`] is to be called once its speed is final.
    Train(Arc<dyn Entity>),
}

/// Start of a cart's tick, before the server moves it. For the first cart of a train to
/// tick the pushes on all of them are gathered into the driver.
pub(super) fn decide(cart: &dyn Entity, world: &World) -> Decision {
    let now = world.game_time();
    with(|trains| {
        let Some(id) = trains.consist(world, cart.uuid(), now) else {
            return Decision::Lone;
        };
        let Some(consist) = trains.consists.get_mut(&id) else {
            return Decision::Lone;
        };
        if consist.decided_at == now {
            return Decision::Decided;
        }
        let Some(carts) = entities(world, &consist.carts) else {
            trains.invalidate(&[cart.uuid()]);
            return Decision::Lone;
        };
        let Some(driver) = carts.first() else {
            return Decision::Lone;
        };
        consist.decided_at = now;
        // The cart pushed hardest sets the speed. When every cart got the same new speed
        // that is the speed, not the sum.
        let mut best = DVec3::ZERO;
        for pushed in &carts {
            let push = horizontal(pushed.velocity()) - consist.assigned;
            if push.length_squared() > best.length_squared() {
                best = push;
            }
        }
        set_horizontal(&**driver, consist.assigned + best);
        consist.from = driver.position();
        Decision::Train(Arc::clone(driver))
    })
}

/// The driver has its speed for this tick. A train sent backwards turns around: gives the
/// cart that is the driver now, when that changed.
pub(super) fn settle(driver: &dyn Entity, world: &World) -> Option<Arc<dyn Entity>> {
    with(|trains| {
        let id = *trains.member.get(&driver.uuid())?;
        let consist = trains.consists.get_mut(&id)?;
        let carts = entities(world, &consist.carts)?;
        let speed = horizontal(driver.velocity());
        consist.assigned = speed;
        if speed.dot(forward(&carts)) >= -1.0e-4 {
            return None;
        }
        consist.reverse(driver.position());
        let rear = carts.last()?;
        set_horizontal(&**rear, speed);
        consist.from = rear.position();
        Some(Arc::clone(rear))
    })
}

/// What a cart is to its train at the end of its tick.
pub(super) enum Moved {
    /// It moves by itself.
    Lone,
    /// Its place is set by the driver.
    Follower,
    /// It drove, and the rest of the train was put behind it.
    Driver,
    /// Its own physics (a slope) took it back into its train: the train turned around,
    /// this cart drives it now.
    Turned(Arc<dyn Entity>),
}

/// A cart to put in place: where, and the speed it is left with.
type Placed = (Arc<dyn Entity>, DVec3, DVec3);

/// End of a cart's tick. After the driver moved everyone else goes onto its trail.
pub(super) fn moved(cart: &dyn Entity, world: &World) -> Moved {
    let (result, placed) = with(|trains| place(trains, cart, world));
    let mut steps = Vec::with_capacity(placed.len());
    for (follower, spot, speed) in placed {
        steps.push((Arc::clone(&follower), spot - follower.position()));
        put(&*follower, spot, speed);
    }
    if !steps.is_empty() && carries_player(cart) {
        for (follower, step) in steps {
            smash_empty(&*follower, world, step);
        }
    }
    result
}

/// A cart with a player, or of a train with one, knocks empty carts off the line: every
/// empty rideable cart it went through this tick (the whole `step`, a fast cart moves
/// blocks a tick) breaks and drops as an item. Chest, hopper, furnace and TNT carts are
/// not broken; they are passed through.
pub(super) fn smash_empty(cart: &dyn Entity, world: &World, step: DVec3) {
    let swept = cart.bounding_box().expand_towards(-step);
    for other in world.get_entities_in_aabb(&swept) {
        if other.entity_type() != &vanilla_entities::MINECART
            || other.uuid() == cart.uuid()
            || other.is_removed()
            || other.is_vehicle()
            || with(|trains| trains.same_train(world, cart.uuid(), other.uuid()))
        {
            continue;
        }
        if let Some(minecart) = other.as_abstract_minecart() {
            minecart.default_destroy(world);
        }
    }
}

fn place(trains: &mut Trains, cart: &dyn Entity, world: &World) -> (Moved, Vec<Placed>) {
    let Some(&id) = trains.member.get(&cart.uuid()) else {
        return (Moved::Lone, Vec::new());
    };
    let Some(consist) = trains.consists.get_mut(&id) else {
        return (Moved::Lone, Vec::new());
    };
    if consist.carts.first() != Some(&cart.uuid()) {
        return (Moved::Follower, Vec::new());
    }
    let Some(mut carts) = entities(world, &consist.carts) else {
        trains.invalidate(&[cart.uuid()]);
        return (Moved::Lone, Vec::new());
    };
    let step = horizontal(cart.position() - consist.from);
    let mut result = Moved::Driver;
    if step.length_squared() > 1.0e-6 && step.dot(forward(&carts)) < 0.0 {
        let speed = cart.velocity();
        consist.reverse(cart.position());
        carts.reverse();
        if let Some(driver) = carts.first() {
            driver.set_velocity(speed);
            result = Moved::Turned(Arc::clone(driver));
        }
    }
    let Some(driver) = carts.first() else {
        return (Moved::Lone, Vec::new());
    };
    let driver_at = driver.position();
    consist.record(driver_at);
    let speed = horizontal(driver.velocity());
    consist.assigned = speed;
    let needed = consist.settle_gaps(step.length() > MOVING);
    let placed = carts
        .iter()
        .skip(1)
        .zip(consist.spots(driver_at))
        .map(|(follower, spot)| (Arc::clone(follower), spot, speed))
        .collect();
    consist.trim(driver_at, needed);
    (result, placed)
}

fn wrap_degrees(degrees: f32) -> f32 {
    let wrapped = degrees % 360.0;
    if wrapped >= 180.0 {
        wrapped - 360.0
    } else if wrapped < -180.0 {
        wrapped + 360.0
    } else {
        wrapped
    }
}

/// Puts a cart behind the driver, facing the way it went, with its riders.
fn put(cart: &dyn Entity, spot: DVec3, speed: DVec3) {
    let from = cart.position();
    if let Err(error) = cart.try_set_position(spot) {
        log::debug!("failed to put minecart {} in its train: {error}", cart.id());
        return;
    }
    cart.set_velocity(DVec3::new(speed.x, 0.0, speed.z));

    // Facing, as the server works it out from the step (it sees none: the cart did not
    // move itself).
    let x_diff = from.x - spot.x;
    let z_diff = from.z - spot.z;
    if x_diff * x_diff + z_diff * z_diff > 0.001
        && let Some(minecart) = cart.as_abstract_minecart()
    {
        let (old_yaw, pitch) = cart.rotation();
        let mut yaw = z_diff.atan2(x_diff).to_degrees() as f32;
        if minecart.is_flipped() {
            yaw += 180.0;
        }
        let turn = f64::from(wrap_degrees(yaw - old_yaw));
        if !(-170.0..170.0).contains(&turn) {
            yaw += 180.0;
            minecart.set_flipped(!minecart.is_flipped());
        }
        cart.set_rotation((yaw % 360.0, pitch));
    }
    for passenger in cart.passengers() {
        cart.position_rider(&*passenger);
    }
}

/// The other carts of the train of `cart`.
pub(super) fn others(cart: &dyn Entity) -> Vec<Arc<dyn Entity>> {
    let Some(world) = cart.level() else {
        return Vec::new();
    };
    with(|trains| {
        if !trains.couplings.is_linked(cart.uuid()) {
            return Vec::new();
        }
        trains
            .train(&world, cart.uuid())
            .into_iter()
            .skip(1)
            .filter_map(|id| resolve(&world, id))
            .collect()
    })
}

/// The cart in front of the train `cart` moves with: `cart` itself when it is alone.
pub(super) fn driver_of(cart: &dyn Entity) -> Option<Arc<dyn Entity>> {
    let world = cart.level()?;
    let driver = with(|trains| {
        let id = trains.member.get(&cart.uuid())?;
        trains.consists.get(id)?.carts.first().copied()
    })?;
    resolve(&world, driver)
}

/// Whether `cart` is put in place by the cart in front of its train.
pub(super) fn follows(cart: &dyn Entity) -> bool {
    with(|trains| {
        trains
            .member
            .get(&cart.uuid())
            .and_then(|id| trains.consists.get(id))
            .is_some_and(|consist| consist.carts.first() != Some(&cart.uuid()))
    })
}

/// Whether a player rides `cart` or any cart of its train.
pub(super) fn carries_player(cart: &dyn Entity) -> bool {
    if has_player(cart) {
        return true;
    }
    let Some(world) = cart.level() else {
        return false;
    };
    with(|trains| trains.carries_player(&world, cart))
}

/// A fast driver finished a part of its move: where it is goes on the trail, so the
/// carts behind it take the same way through curves.
pub(super) fn step(cart: &dyn Entity) {
    with(|trains| {
        let Some(consist) = trains
            .member
            .get(&cart.uuid())
            .and_then(|id| trains.consists.get_mut(id))
        else {
            return;
        };
        if consist.carts.first() == Some(&cart.uuid()) {
            consist.record(cart.position());
        }
    });
}

/// Whether `cart` is coupled: an empty coupled cart rolls as freely as a ridden one, or a
/// few empty wagons would stop a train right after every booster.
pub(super) fn is_linked(cart: &dyn Entity) -> bool {
    with(|trains| trains.couplings.is_linked(cart.uuid()))
}

/// Both carts on rails and moving against each other.
fn head_on(first: &dyn Entity, second: &dyn Entity) -> bool {
    let first_speed = horizontal(first.velocity());
    let second_speed = horizontal(second.velocity());
    first_speed.length_squared() > MOVING * MOVING
        && second_speed.length_squared() > MOVING * MOVING
        && first_speed.dot(second_speed) < 0.0
}

/// Whether `cart` and `other` go through each other. Carts of one train always do (the
/// trail keeps them apart), and so do carts on rails that drive at each other or of which
/// one carries a player: riders never bump into another train, so a cart left on the line
/// does not block the way. A cart or train with a player also goes through mobs and
/// players: it does not shove them, they do not slow it down, and its empty wagons do not
/// scoop mobs up.
pub(super) fn passes_through(cart: &dyn Entity, other: &dyn Entity) -> bool {
    let Some(world) = cart.level() else {
        return false;
    };
    with(|trains| {
        if other.as_abstract_minecart().is_none() {
            return trains.carries_player(&world, cart);
        }
        if trains.same_train(&world, cart.uuid(), other.uuid()) {
            return true;
        }
        if !cart.is_on_rails() || !other.is_on_rails() {
            return false;
        }
        head_on(cart, other)
            || trains.carries_player(&world, cart)
            || trains.carries_player(&world, other)
    })
}

/// A cart left the world for good (broken, killed, another dimension): its couplings
/// break and drop their chains.
pub(super) fn removed(cart: &dyn Entity, reason: RemovalReason) {
    if matches!(
        reason,
        RemovalReason::UnloadedToChunk | RemovalReason::StoredWithPlayer
    ) {
        return;
    }
    let chains = with(|trains| {
        if !trains.couplings.is_linked(cart.uuid()) {
            return Vec::new();
        }
        trains.invalidate(&[cart.uuid()]);
        trains.couplings.unlink_all(cart.uuid())
    });
    for chain in chains {
        drop_chain(cart, &chain);
    }
}

fn is_chain(stack: &ItemStack) -> bool {
    let item = stack.item();
    item.block.is_some() && item.key.to_string().ends_with("_chain")
}

fn sound(world: &World, cart: &dyn Entity, sound: SoundEventRef, pitch: f32) {
    let at = cart.position();
    world.play_sound(
        sound,
        SoundSource::Neutral,
        BlockPos::containing(at.x, at.y, at.z),
        1.0,
        pitch,
        None,
    );
}

/// Why these two carts cannot be coupled, or `None` if they can.
fn link_problem(
    trains: &Trains,
    world: &World,
    first: &dyn Entity,
    second: &dyn Entity,
) -> Option<String> {
    let (first_id, second_id) = (first.uuid(), second.uuid());
    if first.position().distance(second.position()) > LINK_REACH {
        return Some("Вагонетки должны стоять рядом (до 4 блоков)".to_owned());
    }
    if trains.couplings.are_linked(first_id, second_id) {
        return Some("Эти вагонетки уже сцеплены".to_owned());
    }
    if trains.couplings.is_full(first_id) || trains.couplings.is_full(second_id) {
        return Some("К вагонетке можно прицепить только две соседние".to_owned());
    }
    let train = trains.train(world, first_id);
    if train.contains(&second_id) {
        return Some("Нельзя замкнуть состав в кольцо".to_owned());
    }
    if train.len() + trains.train(world, second_id).len() > MAX_CARTS {
        return Some(format!(
            "Слишком длинный состав, максимум {MAX_CARTS} вагонов"
        ));
    }
    None
}

/// What a sneaking click with a chain on `cart` did: the message for the player, the sound
/// to play, and whether a chain was spent.
struct Coupled {
    message: String,
    sound: Option<(SoundEventRef, f32)>,
    spent: bool,
}

fn couple(
    trains: &mut Trains,
    world: &World,
    player: Uuid,
    cart: &dyn Entity,
    chain: Option<String>,
) -> Coupled {
    let quiet = |message: &str| Coupled {
        message: message.to_owned(),
        sound: None,
        spent: false,
    };
    let now = world.game_time();
    let first = trains
        .selected
        .remove(&player)
        .filter(|selection| selection.until >= now)
        .and_then(|selection| resolve(world, selection.cart));
    let Some(first) = first else {
        if trains.couplings.is_full(cart.uuid()) {
            return quiet("К этой вагонетке уже прицеплены две");
        }
        trains.selected.insert(
            player,
            Selection {
                cart: cart.uuid(),
                until: now + SELECT_TICKS,
            },
        );
        return Coupled {
            message: "Вагонетка выбрана. Шифт + ПКМ цепью по соседней".to_owned(),
            sound: Some((&sound_events::BLOCK_CHAIN_HIT, 1.2)),
            spent: false,
        };
    };
    if first.uuid() == cart.uuid() {
        return quiet("Выбор снят");
    }
    if let Some(problem) = link_problem(trains, world, &*first, cart) {
        return quiet(&problem);
    }
    let spent = chain.is_some();
    let both = [first.uuid(), cart.uuid()];
    let mut changed = trains.train(world, first.uuid());
    changed.extend(trains.train(world, cart.uuid()));
    trains.invalidate(&changed);
    trains.couplings.link(both[0], both[1], chain);
    Coupled {
        message: format!(
            "Сцеплено. Вагонов в составе: {}",
            trains.train(world, cart.uuid()).len()
        ),
        sound: Some((&sound_events::BLOCK_CHAIN_PLACE, 1.0)),
        spent,
    }
}

/// A player used `hand` on `cart`: sneaking with a chain couples, sneaking with shears
/// uncouples. `None` leaves the click to the cart.
pub(super) fn interact(
    cart: &dyn Entity,
    player: &Player,
    hand: InteractionHand,
) -> Option<InteractionResult> {
    if player.is_spectator() || !player.is_secondary_use_active() {
        return None;
    }
    let world = cart.level()?;
    let creative = player.has_infinite_materials();
    let held = player.inventory.lock().get_item_in_hand(hand).clone();
    if held.is(&vanilla_items::SHEARS) {
        let chains = with(|trains| {
            if !trains.couplings.is_linked(cart.uuid()) {
                return None;
            }
            let train = trains.train(&world, cart.uuid());
            trains.invalidate(&train);
            Some(trains.couplings.unlink_all(cart.uuid()))
        })?;
        if !creative {
            for chain in chains {
                drop_chain(cart, &chain);
            }
        }
        player
            .inventory
            .lock()
            .get_item_in_hand_mut(hand)
            .hurt_and_break(1, creative);
        sound(&world, cart, &sound_events::ITEM_SHEARS_SNIP, 1.0);
        metro::tell(player, "Вагонетка расцеплена".to_owned());
        return Some(InteractionResult::Success);
    }
    if !is_chain(&held) {
        return None;
    }
    let chain = (!creative).then(|| held.item().key.to_string());
    let coupled = with(|trains| couple(trains, &world, Entity::uuid(player), cart, chain));
    if coupled.spent {
        player.inventory.lock().get_item_in_hand_mut(hand).shrink(1);
    }
    if let Some((event, pitch)) = coupled.sound {
        sound(&world, cart, event, pitch);
    }
    metro::tell(player, coupled.message);
    Some(InteractionResult::Success)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn consist(gaps: &[f64], trail: &[DVec3]) -> Consist {
        Consist {
            carts: (0..=gaps.len())
                .map(|number| Uuid::from_u128(number as u128))
                .collect(),
            gaps: gaps.to_vec(),
            trail: trail.iter().copied().collect(),
            assigned: DVec3::ZERO,
            decided_at: i64::MIN,
            from: DVec3::ZERO,
        }
    }

    fn close(left: DVec3, right: DVec3) -> bool {
        left.distance(right) < 1.0e-9
    }

    #[test]
    fn followers_go_round_the_corner_the_driver_took() {
        // The driver went east along z = 0, then turned north at x = 3.
        let train = consist(
            &[1.5, 1.5],
            &[DVec3::new(3.0, 0.0, 0.0), DVec3::new(0.0, 0.0, 0.0)],
        );
        let spots = train.spots(DVec3::new(3.0, 0.0, -1.0));
        assert_eq!(spots.len(), 2);
        assert!(close(spots[0], DVec3::new(2.5, 0.0, 0.0)), "{spots:?}");
        assert!(close(spots[1], DVec3::new(1.0, 0.0, 0.0)), "{spots:?}");
    }

    #[test]
    fn a_short_trail_ends_where_it_ends() {
        let train = consist(&[1.5, 1.5], &[DVec3::new(1.0, 0.0, 0.0)]);
        let spots = train.spots(DVec3::new(2.0, 0.0, 0.0));
        assert!(close(spots[0], DVec3::new(1.0, 0.0, 0.0)), "{spots:?}");
        assert!(close(spots[1], DVec3::new(1.0, 0.0, 0.0)), "{spots:?}");
    }

    #[test]
    fn a_train_turned_around_keeps_every_cart_in_place() {
        let mut train = consist(
            &[1.5, 2.0],
            &[
                DVec3::new(8.5, 0.0, 0.0),
                DVec3::new(6.5, 0.0, 0.0),
                DVec3::new(2.0, 0.0, 0.0),
            ],
        );
        let driver_at = DVec3::new(10.0, 0.0, 0.0);
        let before = train.spots(driver_at);
        let first = train.carts.clone();
        train.reverse(driver_at);

        assert_eq!(train.gaps, [2.0, 1.5]);
        assert_eq!(train.carts.first(), first.last());
        // The rear cart drives now, from where it stood; the others stay where they were.
        let after = train.spots(before[1]);
        assert!(close(after[0], before[0]), "{after:?}");
        assert!(close(after[1], driver_at), "{after:?}");
    }

    #[test]
    fn gaps_settle_only_while_moving() {
        let mut train = consist(&[0.8, 3.0], &[]);
        assert!((train.settle_gaps(false) - 3.8).abs() < 1.0e-9);
        train.settle_gaps(true);
        assert!((train.gaps[0] - 0.85).abs() < 1.0e-9);
        assert!((train.gaps[1] - 2.95).abs() < 1.0e-9);
    }

    #[test]
    fn the_trail_is_forgotten_past_the_last_cart() {
        let mut train = consist(&[1.5], &[]);
        for step in (0..40).rev() {
            train.trail.push_back(DVec3::new(f64::from(step), 0.0, 0.0));
        }
        train.trim(DVec3::new(40.0, 0.0, 0.0), 1.5);
        assert_eq!(train.trail.len(), 4);
    }
}
