//! The big effects of a show: a meteor shower, a rift torn in the air, rockets flying
//! between rifts, a shockwave (the Fabric `Meteors`, `Rift`, `Rockets` and `Shockwave`).
//! All of it is display entities, particles, sounds and blocks sent to the viewers only:
//! the world is not touched.

use std::{
    f64::consts::{PI, TAU},
    str::FromStr as _,
    sync::Arc,
};

use glam::{DVec3, EulerRot, IVec3, Quat, Vec3};
use rustc_hash::FxHashMap;
use serde_json::Value;
use steel_core::{
    chunk::heightmap::HeightmapType,
    entity::Entity as _,
    player::Player,
    server::Server,
    world::{LevelReader, World},
};
use steel_protocol::packets::game::{
    CBlockUpdate, CLevelParticles, CSound, RandomizationType, SoundSource,
};
use steel_registry::{
    REGISTRY, RegistryExt as _, blocks::block_state_ext::BlockStateExt as _,
    particle_type::ParticleData, sound_event::SoundEventHolder, vanilla_blocks,
};
use steel_utils::{
    BlockPos, BlockStateId, Identifier,
    random::{Random as _, legacy_random::LegacyRandom},
};
use uuid::Uuid;

use super::{
    blocks::block_state,
    ease::Ease,
    json::{self, Object},
    prop::Prop,
    runner::loaded,
    script::{Stage, check_name},
};

/// How close a viewer must be for a meteor to fall there, and who sees it.
const METEOR_SEEN_FROM: f64 = 160.0;
const METEOR_HEARD_FROM: f64 = 96.0;
/// Who sees a rift and its rockets, from its middle.
const RIFT_SEEN_FROM: f64 = 220.0;
/// Who sees a shockwave, from its middle.
const WAVE_SEEN_FROM: f64 = 200.0;

/// What an effect works with on one tick.
pub struct Fx<'a> {
    pub tick: u32,
    /// Skipping ahead to a marker: nothing is shown, what already happened leaves nothing.
    pub muted: bool,
    pub server: &'a Arc<Server>,
    /// The world of the show.
    pub world: &'a Arc<World>,
    /// The viewers who are online, wherever they are.
    pub viewers: Vec<&'a Arc<Player>>,
    pub online: &'a [Arc<Player>],
}

impl<'a> Fx<'a> {
    /// The viewers in `world` within `reach` of `at`.
    pub fn near(&self, world: &Arc<World>, at: DVec3, reach: f64) -> Vec<&'a Arc<Player>> {
        self.viewers
            .iter()
            .filter(|player| {
                Arc::ptr_eq(&player.get_world(), world)
                    && player.position().distance_squared(at) < reach * reach
            })
            .copied()
            .collect()
    }
}

fn particles(
    to: &[&Arc<Player>],
    name: &'static str,
    at: DVec3,
    count: i32,
    spread: f64,
    speed: f32,
) {
    let Some(particle) = REGISTRY
        .particle_types
        .by_key(&Identifier::vanilla_static(name))
    else {
        return;
    };
    #[expect(clippy::cast_possible_truncation, reason = "a spread of a few blocks")]
    let spread = spread as f32;
    for player in to {
        player.send_packet(CLevelParticles {
            particle: ParticleData::simple(particle),
            override_limiter: true,
            always_show: true,
            x: at.x,
            y: at.y,
            z: at.z,
            x_dist: spread,
            y_dist: spread,
            z_dist: spread,
            x_max_speed: speed,
            y_max_speed: speed,
            z_max_speed: speed,
            count,
            randomization_type: RandomizationType::default(),
        });
    }
}

fn sound(to: &[&Arc<Player>], name: &'static str, at: DVec3, volume: f32, pitch: f32) {
    let Some(event) = REGISTRY
        .sound_events
        .by_key(&Identifier::vanilla_static(name))
    else {
        return;
    };
    let seed = rand::random();
    for player in to {
        player.send_packet(CSound::new_holder(
            SoundEventHolder::Registry(event),
            SoundSource::Master,
            at,
            volume,
            pitch,
            seed,
        ));
    }
}

/// The first free block above the ground of a column.
fn ground(world: &World, x: i32, z: i32) -> f64 {
    f64::from(LevelReader::height_at(
        world,
        HeightmapType::MotionBlockingNoLeaves,
        x,
        z,
    ))
}

#[expect(clippy::cast_possible_truncation, reason = "a place in the world")]
fn block_at(at: DVec3) -> BlockPos {
    let at = at.floor();
    BlockPos::new(at.x as i32, at.y as i32, at.z as i32)
}

fn state(object: &Object, key: &str, fallback: &str) -> Result<BlockStateId, String> {
    block_state(&json::string_or(object, key, fallback))
}

/// A whole number of things from a script, at least 1.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "checked to be within 1..=most"
)]
fn count(object: &Object, fallback: f64, most: u32) -> Result<u32, String> {
    let count = json::number(object, "count", fallback)?.floor();
    if count < 1.0 || count > f64::from(most) {
        return Err(format!("count: от 1 до {most}"));
    }
    Ok(count as u32)
}

/// A quaternion that turns "up" to `direction`.
fn up_to(direction: DVec3) -> Quat {
    let direction = direction.as_vec3().normalize_or(Vec3::Y);
    Quat::from_rotation_arc(Vec3::Y, direction)
}

/// A random number in -1..1.
fn signed() -> f64 {
    rand::random::<f64>() * 2.0 - 1.0
}

// --- Meteors ------------------------------------------------------------------------------

/// `"do": "meteors"`: a shower of small meteors over `over`, `count` of them at each place.
/// Each falls at a slant with a fiery trail and hits the ground with a flash, a bang and a
/// small crater only the viewers see, which goes away when the shower ends.
///
/// Where they fall: the show's spots (`/show spot add`), all of them or the ones named in
/// `spots`, and with `aroundPlayers` also around each viewer. A meteor only falls where a
/// viewer is close enough to see it.
#[derive(Clone, Debug)]
pub struct MeteorsDef {
    count: u32,
    spots: Vec<String>,
    around_players: bool,
    radius: f64,
    min_size: f64,
    max_size: f64,
    block: BlockStateId,
    fall: u32,
    height: f64,
    craters: bool,
}

impl MeteorsDef {
    /// Reads the cue; the number is how long it runs.
    pub fn parse(object: &Object) -> Result<(u32, Self), String> {
        let over = json::ticks(object, "over", 600)?;
        if over == 0 {
            return Err("over: больше нуля".to_owned());
        }
        let mut spots = Vec::new();
        match json::get(object, "spots") {
            None => {}
            Some(Value::String(all)) if all == "all" => {}
            Some(Value::Array(names)) => {
                for name in names {
                    let Value::String(name) = name else {
                        return Err("spots: список имён мест или \"all\"".to_owned());
                    };
                    check_name(name)?;
                    spots.push(name.clone());
                }
            }
            Some(_) => return Err("spots: список имён мест или \"all\"".to_owned()),
        }
        let (min_size, max_size) = match json::get(object, "size") {
            Some(Value::Array(pair)) => match pair.as_slice() {
                [from, to] => (from.as_f64().unwrap_or(0.0), to.as_f64().unwrap_or(0.0)),
                _ => (0.0, 0.0),
            },
            Some(_) => {
                let size = json::number(object, "size", 1.0)?;
                (size, size)
            }
            None => (0.6, 1.6),
        };
        if min_size <= 0.0 || max_size < min_size || max_size > 16.0 {
            return Err("size: число или [от, до], от 0 до 16".to_owned());
        }
        let fall = json::ticks(object, "fall", 32)?;
        if !(4..=59).contains(&fall) {
            return Err("fall: от 0.2s до 2.95s (59t)".to_owned());
        }
        Ok((
            over + fall + 1,
            Self {
                count: count(object, 30.0, 2000)?,
                spots,
                around_players: json::boolean(object, "aroundPlayers", true)?,
                radius: json::number(object, "radius", 24.0)?,
                min_size,
                max_size,
                block: state(object, "block", "minecraft:magma_block")?,
                fall,
                height: json::number(object, "height", 70.0)?,
                craters: json::boolean(object, "craters", true)?,
            },
        ))
    }
}

struct Meteor {
    prop: Prop,
    world: Arc<World>,
    from: DVec3,
    to: DVec3,
    start: u32,
    size: f32,
    spin: f32,
}

impl Meteor {
    /// Centered on its position, tumbling as it falls.
    fn shape(&mut self, elapsed: u32, ticks: u32) {
        #[expect(clippy::cast_precision_loss, reason = "ticks of one fall")]
        let turned = elapsed as f32;
        let rotation = Quat::from_euler(
            EulerRot::XYZ,
            (self.spin + turned * 9.0).to_radians(),
            (self.spin * 0.7 + turned * 6.0).to_radians(),
            0.0,
        );
        let scale = Vec3::splat(self.size);
        self.prop.shape(
            rotation * (Vec3::splat(-0.5) * scale),
            rotation,
            scale,
            ticks,
        );
    }
}

/// A place meteors may fall now.
struct Target {
    key: String,
    world: Arc<World>,
    pos: DVec3,
}

/// A shower that is falling.
#[derive(Default)]
pub struct Shower {
    falling: Vec<Meteor>,
    /// Crater blocks per world, put back on the viewers' screens when the shower ends.
    craters: Vec<(Arc<World>, FxHashMap<BlockPos, BlockStateId>)>,
    /// Meteors sent so far to each place.
    launched: FxHashMap<String, u32>,
}

impl Shower {
    /// One tick of the shower, `elapsed` ticks into a cue that runs for `duration`.
    pub fn update(
        &mut self,
        def: &MeteorsDef,
        duration: u32,
        elapsed: u32,
        spots: &FxHashMap<String, Stage>,
        fx: &Fx<'_>,
    ) {
        let over = duration.saturating_sub(def.fall + 1).max(1);
        let due = if elapsed >= over {
            def.count
        } else {
            u32::try_from(u64::from(def.count) * u64::from(elapsed) / u64::from(over))
                .unwrap_or(def.count)
        };
        if !fx.muted {
            for target in targets(def, spots, fx) {
                // A place that only now has someone near starts from here, not with a
                // burst of the ones it missed.
                let done = self.launched.get(&target.key).copied().unwrap_or(due);
                for _ in done..due {
                    self.launch(def, &target, fx);
                }
                self.launched.insert(target.key, due.max(done));
            }
        }
        let mut index = 0;
        while index < self.falling.len() {
            if self.fly(def, index, fx) {
                let meteor = self.falling.swap_remove(index);
                self.land(def, meteor, fx);
            } else {
                index += 1;
            }
        }
    }

    fn launch(&mut self, def: &MeteorsDef, target: &Target, fx: &Fx<'_>) {
        let angle = rand::random::<f64>() * TAU;
        let distance = rand::random::<f64>().sqrt() * def.radius;
        let column = block_at(target.pos + DVec3::new(angle.cos(), 0.0, angle.sin()) * distance);
        if !loaded(&target.world, column) {
            return;
        }
        let to = DVec3::new(
            f64::from(column.x()) + 0.5,
            ground(&target.world, column.x(), column.z()),
            f64::from(column.z()) + 0.5,
        );
        // Comes in at a slant from a random side, 30 to 50 degrees above the horizon.
        let azimuth = rand::random::<f64>() * TAU;
        let slope = (30.0 + rand::random::<f64>() * 20.0).to_radians();
        let run = def.height / slope.tan();
        let from = to + DVec3::new(azimuth.cos() * run, def.height, azimuth.sin() * run);

        #[expect(clippy::cast_possible_truncation, reason = "a size of at most 16")]
        let size = (def.min_size + rand::random::<f64>() * (def.max_size - def.min_size)) as f32;
        let mut meteor = Meteor {
            prop: Prop::new(def.block, from, 4.0),
            world: Arc::clone(&target.world),
            from,
            to,
            start: fx.tick,
            size,
            spin: rand::random::<f32>() * 360.0,
        };
        meteor.shape(0, 0);
        meteor
            .prop
            .sync(&fx.near(&target.world, to, METEOR_SEEN_FROM));
        self.falling.push(meteor);
        sound(
            &fx.near(&target.world, to, METEOR_HEARD_FROM * 1.5),
            "item.firecharge.use",
            from,
            6.0,
            0.5 + rand::random::<f32>() * 0.3,
        );
    }

    /// Moves one meteor; true once it is down.
    fn fly(&mut self, def: &MeteorsDef, index: usize, fx: &Fx<'_>) -> bool {
        let Some(meteor) = self.falling.get_mut(index) else {
            return false;
        };
        let elapsed = fx.tick.saturating_sub(meteor.start);
        if elapsed == 1 {
            // The client has seen it up in the sky by now: the whole fall is one smooth
            // move there, the server only counts the ticks until it lands.
            meteor.prop.glide(meteor.to, def.fall);
        }
        let t = (f64::from(elapsed.saturating_sub(1)) / f64::from(def.fall)).clamp(0.0, 1.0);
        let pos = meteor.from.lerp(meteor.to, t);
        let watching = fx.near(&meteor.world, pos, METEOR_SEEN_FROM);
        let size = f64::from(meteor.size);
        particles(&watching, "flame", pos, 6, size * 0.3, 0.02);
        particles(&watching, "large_smoke", pos, 3, size * 0.2, 0.01);
        if elapsed.is_multiple_of(4) && t < 1.0 {
            meteor.shape(elapsed + 4, 4);
        }
        meteor
            .prop
            .sync(&fx.near(&meteor.world, meteor.to, METEOR_SEEN_FROM));
        t >= 1.0
    }

    fn land(&mut self, def: &MeteorsDef, mut meteor: Meteor, fx: &Fx<'_>) {
        meteor.prop.remove(fx.online);
        let at = meteor.to;
        let big = meteor.size > 1.2;
        let watching = fx.near(&meteor.world, at, METEOR_SEEN_FROM);
        let flash = if big {
            "explosion_emitter"
        } else {
            "explosion"
        };
        particles(&watching, flash, at, 1, 0.0, 0.0);
        particles(&watching, "lava", at, 12, 0.5, 0.1);
        particles(&watching, "campfire_cosy_smoke", at, 4, 0.4, 0.02);
        sound(
            &fx.near(&meteor.world, at, METEOR_HEARD_FROM),
            "entity.generic.explode",
            at,
            2.0 + meteor.size,
            0.7 + rand::random::<f32>() * 0.5,
        );
        if def.craters {
            self.crater(&meteor.world, at, big, &watching);
        }
    }

    /// A small scorched dent, sent as blocks to whoever is near; the world is not touched.
    fn crater(&mut self, world: &Arc<World>, at: DVec3, big: bool, watching: &[&Arc<Player>]) {
        let center = block_at(at).0 - IVec3::Y;
        let reach = if big { 2 } else { 1 };
        let mut blocks = FxHashMap::default();
        for dx in -reach..=reach {
            for dz in -reach..=reach {
                if dx * dx + dz * dz > reach * reach + 1 {
                    continue;
                }
                let top = BlockPos(center + IVec3::new(dx, 0, dz));
                if !loaded(world, top) {
                    continue;
                }
                let there = world.get_block_state(top);
                if there.is_air() || there.has_block_entity() {
                    continue;
                }
                let middle = dx == 0 && dz == 0;
                let scorched = if middle {
                    &vanilla_blocks::MAGMA_BLOCK
                } else if rand::random_range(0..3) == 0 {
                    &vanilla_blocks::BLACKSTONE
                } else {
                    &vanilla_blocks::COAL_BLOCK
                };
                blocks.insert(top, scorched.default_state());
                let above = BlockPos(top.0 + IVec3::Y);
                if middle && world.get_block_state(above).is_replaceable() {
                    blocks.insert(above, vanilla_blocks::FIRE.default_state());
                }
            }
        }
        for player in watching {
            for (pos, state) in &blocks {
                player.send_packet(CBlockUpdate {
                    pos: *pos,
                    block_state: *state,
                });
            }
        }
        if let Some((_, known)) = self
            .craters
            .iter_mut()
            .find(|(known, _)| Arc::ptr_eq(known, world))
        {
            known.extend(blocks);
        } else {
            self.craters.push((Arc::clone(world), blocks));
        }
    }

    /// The shower is over (or the show stopped): meteors still in the air vanish, craters
    /// fill back in.
    pub fn clear(&mut self, fx: &Fx<'_>) {
        for mut meteor in self.falling.drain(..) {
            meteor.prop.remove(fx.online);
        }
        for (world, blocks) in self.craters.drain(..) {
            for pos in blocks.into_keys() {
                if !loaded(&world, pos) {
                    continue;
                }
                let real = world.get_block_state(pos);
                let middle = pos.0.as_dvec3() + DVec3::splat(0.5);
                for player in fx.near(&world, middle, METEOR_SEEN_FROM * 2.0) {
                    player.send_packet(CBlockUpdate {
                        pos,
                        block_state: real,
                    });
                }
            }
        }
        self.launched.clear();
    }

    pub fn forget(&mut self, id: Uuid) {
        for meteor in &mut self.falling {
            meteor.prop.forget(id);
        }
    }
}

/// Places a meteor may fall now: spots with a viewer nearby, and the viewers themselves.
fn targets(def: &MeteorsDef, spots: &FxHashMap<String, Stage>, fx: &Fx<'_>) -> Vec<Target> {
    let mut out = Vec::new();
    for (name, spot) in spots {
        if !def.spots.is_empty() && !def.spots.contains(name) {
            continue;
        }
        let Some(world) = Identifier::from_str(&spot.world)
            .ok()
            .and_then(|key| fx.server.worlds.get(&key))
        else {
            continue;
        };
        if !fx.near(world, spot.pos, METEOR_SEEN_FROM).is_empty() {
            out.push(Target {
                key: format!("spot:{name}"),
                world: Arc::clone(world),
                pos: spot.pos,
            });
        }
    }
    if def.around_players || spots.is_empty() && def.spots.is_empty() {
        for player in &fx.viewers {
            out.push(Target {
                key: format!("player:{}", player.gameprofile.id()),
                world: player.get_world(),
                pos: player.position(),
            });
        }
    }
    out
}

// --- Rift ---------------------------------------------------------------------------------

/// `"do": "rift"`: a jagged tear hanging in the air. It cracks open from the middle
/// outwards, crackles with portal sparks while it is open and seals itself over `close` at
/// the end of `over`. Rockets fly between open rifts by their `name`.
#[derive(Clone, Debug)]
pub struct RiftDef {
    pub name: String,
    pub pos: DVec3,
    length: f64,
    width: f64,
    yaw: f64,
    tilt: f64,
    open: u32,
    close: u32,
    core: BlockStateId,
    edge: BlockStateId,
    glow: i32,
    sound: bool,
}

impl RiftDef {
    /// Reads the cue; the number is how long it runs.
    pub fn parse(object: &Object) -> Result<(u32, Self), String> {
        let over = json::ticks(object, "over", 400)?;
        let open = json::ticks(object, "open", 40)?;
        let close = json::ticks(object, "close", 30)?;
        if over == 0 || over < open + close {
            return Err("over: не меньше open + close".to_owned());
        }
        let length = json::number(object, "length", 30.0)?;
        let width = json::number(object, "width", 3.0)?;
        if !(2.0..=200.0).contains(&length) || width <= 0.0 || width > 40.0 {
            return Err("length: от 2 до 200, width: до 40".to_owned());
        }
        let glow = i32::from_str_radix(&json::string_or(object, "glow", "c040ff"), 16)
            .map_err(|_| "glow: цвет RGB в hex, например c040ff".to_owned())?;
        Ok((
            over,
            Self {
                name: json::string_or(object, "name", "rift"),
                pos: json::vec_opt(object, "pos")?.unwrap_or(DVec3::new(0.0, 60.0, 0.0)),
                length,
                width,
                yaw: json::number(object, "yaw", 0.0)?,
                tilt: json::number(object, "tilt", 15.0)?,
                open,
                close,
                core: state(object, "core", "minecraft:white_concrete")?,
                edge: state(object, "edge", "minecraft:magenta_stained_glass")?,
                glow,
                sound: json::boolean(object, "sound", true)?,
            },
        ))
    }

    /// The line of the crack through `center`: along `yaw` (degrees, 0 = east), raised
    /// `tilt` degrees, zig-zagging in the upright plane. The same rift is the same line
    /// every time, and the same one the Fabric server drew.
    fn line(&self, center: DVec3, at: u32) -> Vec<DVec3> {
        let (yaw, tilt) = (self.yaw.to_radians(), self.tilt.to_radians());
        let axis = DVec3::new(yaw.cos() * tilt.cos(), tilt.sin(), yaw.sin() * tilt.cos());
        let side = DVec3::Y - axis * DVec3::Y.dot(axis);
        let side = if side.length_squared() < 1e-4 {
            DVec3::X
        } else {
            side.normalize()
        };
        let seed = i64::from(java_hash(&self.name)) * 31 + i64::from(at);
        #[expect(clippy::cast_sign_loss, reason = "a seed is bits, as in Java")]
        let mut random = LegacyRandom::from_seed(seed as u64);
        let pieces = pieces(self.length);
        (0..=pieces)
            .map(|index| {
                let along = f64::from(index) / f64::from(pieces);
                let taper = (PI * along).sin();
                let mut jag = (random.next_f64() * 2.0 - 1.0) * self.width * 0.9 * (0.3 + taper);
                if index == 0 || index == pieces {
                    jag = 0.0;
                }
                center + axis * ((along - 0.5) * self.length) + side * jag
            })
            .collect()
    }
}

/// How many straight pieces a rift of this length is made of.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a length of at most 200"
)]
fn pieces(length: f64) -> u32 {
    ((length / 2.5).round() as u32).max(4)
}

/// Java `String.hashCode`.
fn java_hash(text: &str) -> i32 {
    text.encode_utf16().fold(0_i32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(i32::from(unit))
    })
}

struct Segment {
    core: Prop,
    edge: Prop,
    rotation: Quat,
    length: f32,
    core_thickness: f32,
    edge_thickness: f32,
    /// Ticks after the rift before this piece opens: the outer ones open later.
    delay: u32,
}

impl Segment {
    /// `grow` from 0 (shut) to 1 (open), blended by the client over `ticks`.
    fn reshape(&mut self, grow: f32, ticks: u32) {
        for (prop, thickness) in [
            (&mut self.core, self.core_thickness),
            (&mut self.edge, self.edge_thickness),
        ] {
            let thick = thickness * grow;
            let long = (self.length + thick * 0.5) * (0.15 + 0.85 * grow);
            let scale = Vec3::new(thick, long, thick);
            // The entity stands below its piece (see `Rift::open`): lift the piece back.
            let translation = self.rotation * (Vec3::splat(-0.5) * scale)
                + Vec3::new(0.0, self.length + 2.0, 0.0);
            prop.shape(translation, self.rotation, scale, ticks);
        }
    }
}

/// A rift that is open.
pub struct Rift {
    segments: Vec<Segment>,
    points: Vec<DVec3>,
    center: DVec3,
    /// It has begun to seal: rockets no longer fly to it.
    pub closing: bool,
}

impl Rift {
    /// Tears the rift of a cue that starts on tick `at`, shut, around `center`.
    pub fn open(def: &RiftDef, at: u32, center: DVec3, fx: &Fx<'_>) -> Self {
        let points = def.line(center, at);
        let count = points.len().saturating_sub(1).max(1);
        let segments = points
            .windows(2)
            .enumerate()
            .filter_map(|(index, ends)| {
                let [from, to] = ends else { return None };
                let along = to - from;
                #[expect(clippy::cast_precision_loss, reason = "a few dozen pieces")]
                let middle = (index as f64 + 0.5) / count as f64;
                let taper = (PI * middle).sin();
                #[expect(clippy::cast_possible_truncation, reason = "a few blocks")]
                let length = along.length() as f32;
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "ticks, at least zero"
                )]
                let delay = ((middle - 0.5).abs() * 2.0 * f64::from(def.open) * 0.6).round() as u32;
                // The client culls a display by a box that grows up from its feet, so the
                // entity stands below its piece and the shape lifts the piece into place.
                let mid = from + along * 0.5;
                let feet = DVec3::new(mid.x, mid.y - f64::from(length) - 2.0, mid.z);
                let prop = |state: BlockStateId| {
                    let mut prop = Prop::new(state, feet, 8.0);
                    prop.cull_box(length * 2.0 + 4.0, length * 2.0 + 4.0);
                    prop
                };
                let mut edge = prop(def.edge);
                edge.glow(def.glow);
                #[expect(clippy::cast_possible_truncation, reason = "a width of at most 40")]
                let mut segment = Segment {
                    core: prop(def.core),
                    edge,
                    rotation: up_to(along),
                    length,
                    core_thickness: (0.15 + def.width * 0.22 * taper) as f32,
                    edge_thickness: (0.4 + def.width * taper) as f32,
                    delay,
                };
                segment.reshape(0.0, 0);
                Some(segment)
            })
            .collect();
        if !fx.muted && def.sound {
            let hearing = fx.near(fx.world, center, RIFT_SEEN_FROM * 1.5);
            sound(&hearing, "block.end_portal.spawn", center, 10.0, 0.7);
            sound(&hearing, "entity.warden.sonic_charge", center, 8.0, 0.5);
        }
        Self {
            segments,
            points,
            center,
            closing: false,
        }
    }

    /// One tick of the rift, `elapsed` ticks into a cue that runs for `duration`.
    pub fn update(&mut self, def: &RiftDef, duration: u32, elapsed: u32, fx: &Fx<'_>) {
        let close_at = duration.saturating_sub(def.close);
        for segment in &mut self.segments {
            if elapsed == 1 + segment.delay {
                let ticks = if fx.muted {
                    0
                } else {
                    def.open.saturating_sub(segment.delay).max(2)
                };
                segment.reshape(1.0, ticks);
            }
            // Seals from the ends inwards.
            let close_delay = (def.close.div_ceil(2)).saturating_sub(segment.delay / 2);
            if elapsed == close_at + close_delay {
                let ticks = if fx.muted {
                    0
                } else {
                    def.close.saturating_sub(close_delay).max(2)
                };
                segment.reshape(0.0, ticks);
            }
        }
        let hearing = fx.near(fx.world, self.center, RIFT_SEEN_FROM * 1.5);
        if elapsed >= close_at && !self.closing {
            self.closing = true;
            if !fx.muted && def.sound {
                sound(&hearing, "block.beacon.deactivate", self.center, 8.0, 0.5);
                sound(&hearing, "entity.enderman.teleport", self.center, 8.0, 0.4);
            }
        }
        if !fx.muted {
            let watching = fx.near(fx.world, self.center, RIFT_SEEN_FROM);
            if elapsed > 0 && elapsed < close_at {
                self.sparks(def, elapsed, &watching);
                if def.sound && elapsed.is_multiple_of(50) {
                    let pitch = 0.5 + rand::random::<f32>() * 0.2;
                    sound(&hearing, "block.portal.ambient", self.center, 6.0, pitch);
                }
            }
            for segment in &mut self.segments {
                segment.core.sync(&watching);
                segment.edge.sync(&watching);
            }
        }
    }

    fn sparks(&self, def: &RiftDef, elapsed: u32, watching: &[&Arc<Player>]) {
        if watching.is_empty() || self.points.is_empty() {
            return;
        }
        let spread = def.width * 0.5 + 0.5;
        for _ in 0..3 {
            let Some(at) = self
                .points
                .get(rand::random_range(0..self.points.len()))
                .copied()
            else {
                continue;
            };
            particles(watching, "reverse_portal", at, 8, spread, 0.08);
            particles(watching, "portal", at, 6, spread, 0.6);
            if elapsed.is_multiple_of(4) {
                particles(watching, "end_rod", at, 2, spread * 0.5, 0.05);
            }
            if rand::random_range(0..6) == 0 {
                particles(watching, "electric_spark", at, 10, spread, 0.3);
            }
        }
    }

    pub fn clear(&mut self, fx: &Fx<'_>) {
        for mut segment in self.segments.drain(..) {
            segment.core.remove(fx.online);
            segment.edge.remove(fx.online);
        }
        self.points.clear();
    }

    pub fn forget(&mut self, id: Uuid) {
        for segment in &mut self.segments {
            segment.core.forget(id);
            segment.edge.forget(id);
        }
    }
}

// --- Rockets ------------------------------------------------------------------------------

/// Where rockets fly from and to.
#[derive(Clone, Debug)]
pub enum RocketEnd {
    /// A rift by its name, while it is open.
    Rift(String),
    /// A place on the stage.
    Point(DVec3),
}

/// `"do": "rockets"`: `count` rockets over `over`, each bursting out of one rift and diving
/// into another along an arc, with a fiery trail. `between` lists rift names (only the ones
/// open at that moment are used) or stage positions; a rocket needs two different ends, so
/// with fewer than two open it is not launched.
#[derive(Clone, Debug)]
pub struct RocketsDef {
    pub between: Vec<RocketEnd>,
    count: u32,
    flight: u32,
    arc: f64,
    size: f32,
    body: BlockStateId,
    nose: BlockStateId,
    flame: BlockStateId,
    sound: bool,
}

impl RocketsDef {
    /// Reads the cue; the number is how long it runs.
    pub fn parse(object: &Object) -> Result<(u32, Self), String> {
        let ends = match json::get(object, "between") {
            Some(Value::Array(ends)) if ends.len() >= 2 => ends,
            _ => {
                return Err("between: хотя бы два разлома (имена) или точки [x, y, z]".to_owned());
            }
        };
        let mut between = Vec::with_capacity(ends.len());
        for end in ends {
            between.push(match end {
                Value::String(name) => RocketEnd::Rift(name.clone()),
                Value::Array(_) => RocketEnd::Point(
                    json::vec(end).map_err(|_| "between: имя разлома или [x, y, z]".to_owned())?,
                ),
                _ => return Err("between: имя разлома или [x, y, z]".to_owned()),
            });
        }
        let over = json::ticks(object, "over", 400)?;
        if over == 0 {
            return Err("over больше нуля, count от 1 до 500".to_owned());
        }
        let flight = json::ticks(object, "flight", 60)?;
        if !(10..=200).contains(&flight) {
            return Err("flight: от 0.5s до 10s".to_owned());
        }
        let size = json::float(object, "size", 1.0)?;
        if size <= 0.0 || size > 8.0 {
            return Err("size: до 8".to_owned());
        }
        Ok((
            over + flight + 1,
            Self {
                between,
                count: count(object, 30.0, 500)?,
                flight,
                arc: json::number(object, "arc", 14.0)?,
                size,
                body: state(object, "body", "minecraft:white_concrete")?,
                nose: state(object, "nose", "minecraft:red_concrete")?,
                flame: state(object, "flame", "minecraft:shroomlight")?,
                sound: json::boolean(object, "sound", true)?,
            },
        ))
    }
}

struct RocketPart {
    prop: Prop,
    /// Where the piece sits along the rocket, from its middle.
    center: Vec3,
    scale: Vec3,
}

struct Rocket {
    parts: Vec<RocketPart>,
    from: DVec3,
    control: DVec3,
    to: DVec3,
    start: u32,
}

impl Rocket {
    fn at(&self, t: f64) -> DVec3 {
        let u = 1.0 - t;
        self.from * (u * u) + self.control * (2.0 * u * t) + self.to * (t * t)
    }

    fn heading(&self, t: f64) -> DVec3 {
        let along =
            (self.control - self.from) * (2.0 * (1.0 - t)) + (self.to - self.control) * (2.0 * t);
        along.try_normalize().unwrap_or(DVec3::Y)
    }

    /// Points the rocket's nose along its path.
    fn orient(&mut self, t: f64, ticks: u32) {
        let rotation = up_to(self.heading(t));
        for part in &mut self.parts {
            let corner = Vec3::splat(-0.5) * part.scale + part.center;
            part.prop
                .shape(rotation * corner, rotation, part.scale, ticks);
        }
    }
}

/// Rockets in the air.
#[derive(Default)]
pub struct Volley {
    flying: Vec<Rocket>,
    launched: u32,
}

impl Volley {
    /// One tick of the rockets, `elapsed` ticks into a cue that runs for `duration`; `ends`
    /// are the places rockets may fly between right now.
    pub fn update(
        &mut self,
        def: &RocketsDef,
        duration: u32,
        elapsed: u32,
        ends: &[DVec3],
        fx: &Fx<'_>,
    ) {
        let over = duration.saturating_sub(def.flight + 1).max(1);
        let due = if elapsed >= over {
            def.count
        } else {
            u32::try_from(u64::from(def.count) * u64::from(elapsed) / u64::from(over))
                .unwrap_or(def.count)
        };
        while self.launched < due {
            self.launched += 1;
            // Skipping ahead: rockets already gone leave nothing behind.
            if !fx.muted {
                self.launch(def, ends, fx);
            }
        }
        let mut index = 0;
        while index < self.flying.len() {
            if self.fly(def, index, fx) {
                let rocket = self.flying.swap_remove(index);
                Self::arrive(def, rocket, fx);
            } else {
                index += 1;
            }
        }
    }

    fn launch(&mut self, def: &RocketsDef, ends: &[DVec3], fx: &Fx<'_>) {
        if ends.len() < 2 {
            return;
        }
        let first = rand::random_range(0..ends.len());
        let mut second = rand::random_range(0..ends.len() - 1);
        if second >= first {
            second += 1;
        }
        let (Some(from), Some(to)) = (ends.get(first), ends.get(second)) else {
            return;
        };
        let jitter = |by: f64| DVec3::new(signed() * by, signed() * by * 0.4, signed() * by);
        let (from, to) = (*from + jitter(3.0), *to + jitter(3.0));
        let lift = def.arc * (0.6 + rand::random::<f64>() * 0.8);
        let control = (from + to) * 0.5 + jitter(10.0) + DVec3::new(0.0, lift, 0.0);

        let size = def.size;
        let part = |state: BlockStateId, along: f32, scale: Vec3| RocketPart {
            prop: {
                let mut prop = Prop::new(state, from, 6.0);
                prop.glide(from, 2);
                prop
            },
            center: Vec3::new(0.0, along * size, 0.0),
            scale: scale * size,
        };
        let mut rocket = Rocket {
            parts: vec![
                part(def.body, 0.0, Vec3::new(0.5, 2.0, 0.5)),
                part(def.nose, 1.25, Vec3::new(0.34, 0.5, 0.34)),
                // Fins.
                part(def.nose, -0.85, Vec3::new(1.0, 0.3, 0.12)),
                part(def.nose, -0.85, Vec3::new(0.12, 0.3, 1.0)),
                part(def.flame, -1.15, Vec3::new(0.36, 0.3, 0.36)),
            ],
            from,
            control,
            to,
            start: fx.tick,
        };
        rocket.orient(0.0, 0);
        let watching = fx.near(fx.world, from, RIFT_SEEN_FROM);
        for part in &mut rocket.parts {
            part.prop.sync(&watching);
        }
        self.flying.push(rocket);
        if def.sound {
            let pitch = 0.5 + rand::random::<f32>() * 0.2;
            sound(&watching, "entity.firework_rocket.launch", from, 8.0, pitch);
        }
        particles(&watching, "reverse_portal", from, 30, 1.0, 0.3);
    }

    /// Moves one rocket; true once it is there.
    fn fly(&mut self, def: &RocketsDef, index: usize, fx: &Fx<'_>) -> bool {
        let Some(rocket) = self.flying.get_mut(index) else {
            return false;
        };
        let elapsed = fx.tick.saturating_sub(rocket.start);
        let t = (f64::from(elapsed) / f64::from(def.flight)).clamp(0.0, 1.0);
        // Leaves the rift at half speed and picks up from there.
        let eased = 0.5 * t * t + 0.5 * t;
        let pos = rocket.at(eased);
        if elapsed.is_multiple_of(2) {
            rocket.orient((eased + 0.05).min(1.0), 2);
        }
        let watching = fx.near(fx.world, pos, RIFT_SEEN_FROM);
        for part in &mut rocket.parts {
            part.prop.glide(pos, 2);
            part.prop.sync(&watching);
        }
        let tail = pos - rocket.heading(eased) * (1.4 * f64::from(def.size));
        particles(&watching, "flame", tail, 4, 0.15, 0.02);
        particles(&watching, "large_smoke", tail, 2, 0.2, 0.01);
        if elapsed.is_multiple_of(3) {
            particles(&watching, "firework", tail, 1, 0.1, 0.02);
        }
        t >= 1.0
    }

    fn arrive(def: &RocketsDef, mut rocket: Rocket, fx: &Fx<'_>) {
        for part in &mut rocket.parts {
            part.prop.remove(fx.online);
        }
        let at = rocket.to;
        let watching = fx.near(fx.world, at, RIFT_SEEN_FROM);
        particles(&watching, "reverse_portal", at, 40, 1.2, 0.4);
        particles(&watching, "end_rod", at, 8, 0.6, 0.15);
        if def.sound {
            sound(&watching, "entity.enderman.teleport", at, 6.0, 0.5);
        }
    }

    pub fn clear(&mut self, fx: &Fx<'_>) {
        for mut rocket in self.flying.drain(..) {
            for part in &mut rocket.parts {
                part.prop.remove(fx.online);
            }
        }
    }

    pub fn forget(&mut self, id: Uuid) {
        for rocket in &mut self.flying {
            for part in &mut rocket.parts {
                part.prop.forget(id);
            }
        }
    }
}

// --- Shockwave ----------------------------------------------------------------------------

/// `"do": "shockwave"`: a ring of particles racing outwards along the ground (or flat, with
/// `ground: false`).
#[derive(Clone, Debug)]
pub struct ShockwaveDef {
    ease: Ease,
    pub pos: DVec3,
    radius: f64,
    particle: &'static str,
    ground: bool,
}

/// The particles a shockwave may be made of: the ones that need no parameters.
fn simple_particle(id: &str) -> Option<&'static str> {
    let key = Identifier::from_str(id).ok()?;
    let particle = REGISTRY.particle_types.by_key(&key)?;
    (particle.is_simple() && key.namespace == "minecraft").then_some(particle.key.path.as_ref())
}

impl ShockwaveDef {
    /// Reads the cue; the number is how long it runs.
    pub fn parse(object: &Object) -> Result<(u32, Self), String> {
        let radius = json::number(object, "radius", 30.0)?;
        if radius <= 0.0 || radius > 200.0 {
            return Err("radius: от 0 до 200".to_owned());
        }
        let id = json::string_or(object, "id", "minecraft:cloud");
        let particle = simple_particle(&id)
            .ok_or_else(|| format!("частицы \"{id}\" не найдены или требуют параметров"))?;
        Ok((
            json::ticks(object, "over", 30)?.max(1),
            Self {
                ease: json::ease(object, Ease::OutCubic)?,
                pos: json::vec_opt(object, "pos")?.unwrap_or(DVec3::ZERO),
                radius,
                particle,
                ground: json::boolean(object, "ground", true)?,
            },
        ))
    }

    /// The ring `progress` (0..1) of the way out from `center`.
    pub fn update(&self, center: DVec3, progress: f64, fx: &Fx<'_>) {
        if fx.muted {
            return;
        }
        let watching = fx.near(fx.world, center, WAVE_SEEN_FROM);
        if watching.is_empty() {
            return;
        }
        let ring = (self.radius * self.ease.apply(progress)).max(0.5);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "at most 160 points"
        )]
        let points = ((ring * TAU / 1.8).ceil() as u32).min(160);
        for index in 0..points {
            let angle = TAU * f64::from(index) / f64::from(points);
            let (x, z) = (center.x + angle.cos() * ring, center.z + angle.sin() * ring);
            let mut y = center.y;
            if self.ground {
                let column = block_at(DVec3::new(x, 0.0, z));
                if loaded(fx.world, column) {
                    y = ground(fx.world, column.x(), column.z()) + 0.2;
                }
            }
            particles(&watching, self.particle, DVec3::new(x, y, z), 1, 0.1, 0.02);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use steel_registry::{init_vanilla_registry, particle_type::ParticleType};

    use super::*;

    fn object(value: Value) -> Object {
        let Value::Object(object) = value else {
            unreachable!("an object literal")
        };
        object
    }

    #[test]
    fn every_particle_and_sound_is_in_the_registry() {
        init_vanilla_registry();
        for name in [
            "flame",
            "large_smoke",
            "explosion_emitter",
            "explosion",
            "lava",
            "campfire_cosy_smoke",
            "reverse_portal",
            "portal",
            "end_rod",
            "electric_spark",
            "firework",
            "cloud",
        ] {
            let key = Identifier::vanilla_static(name);
            let particle = REGISTRY.particle_types.by_key(&key);
            assert!(particle.is_some_and(ParticleType::is_simple), "{name}");
        }
        for name in [
            "item.firecharge.use",
            "entity.generic.explode",
            "block.end_portal.spawn",
            "entity.warden.sonic_charge",
            "block.beacon.deactivate",
            "entity.enderman.teleport",
            "block.portal.ambient",
            "entity.firework_rocket.launch",
        ] {
            let key = Identifier::vanilla_static(name);
            assert!(REGISTRY.sound_events.by_key(&key).is_some(), "{name}");
        }
    }

    #[test]
    fn a_rift_is_the_same_line_every_time() {
        init_vanilla_registry();
        let (over, def) =
            RiftDef::parse(&object(json!({"name": "east", "length": 30}))).expect("a rift");
        assert_eq!(over, 400);
        let center = DVec3::new(0.0, 80.0, 0.0);
        let line = def.line(center, 40);
        assert_eq!(line.len(), 13);
        assert_eq!(line, def.line(center, 40));
        assert_ne!(line, def.line(center, 41));
        // The ends are on the axis, 30 blocks apart; the middle zig-zags off it.
        let (first, last) = (line[0], line[12]);
        assert!((first.distance(last) - 30.0).abs() < 1e-9);
        assert!(((first + last) * 0.5).distance(center) < 1e-9);
        assert!(line[6].distance(center) > 1e-3);
        assert_eq!(java_hash("rift"), 3_500_357);

        assert!(RiftDef::parse(&object(json!({"over": "1s"}))).is_err());
        assert!(RiftDef::parse(&object(json!({"glow": "pink"}))).is_err());
    }

    #[test]
    fn cues_check_what_they_are_given() {
        init_vanilla_registry();
        let (over, _) =
            MeteorsDef::parse(&object(json!({"over": "10s", "fall": "1s"}))).expect("meteors");
        assert_eq!(over, 221);
        assert!(MeteorsDef::parse(&object(json!({"count": 0}))).is_err());
        assert!(MeteorsDef::parse(&object(json!({"size": [2, 1]}))).is_err());
        assert!(MeteorsDef::parse(&object(json!({"spots": ["Bad Name"]}))).is_err());
        assert!(MeteorsDef::parse(&object(json!({"spots": "all", "size": 2}))).is_ok());

        let (over, rockets) = RocketsDef::parse(&object(
            json!({"between": ["east", [0, 80, 0]], "over": "5s", "flight": "2s"}),
        ))
        .expect("rockets");
        assert_eq!(over, 141);
        assert_eq!(rockets.between.len(), 2);
        assert!(RocketsDef::parse(&object(json!({"between": ["east"]}))).is_err());
        assert!(RocketsDef::parse(&object(json!({"between": ["a", 5]}))).is_err());

        assert!(ShockwaveDef::parse(&object(json!({"radius": 500}))).is_err());
        assert!(ShockwaveDef::parse(&object(json!({"id": "minecraft:dust"}))).is_err());
        let (over, _) = ShockwaveDef::parse(&object(json!({}))).expect("a shockwave");
        assert_eq!(over, 30);
    }
}
