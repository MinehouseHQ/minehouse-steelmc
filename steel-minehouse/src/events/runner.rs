//! One run of a show (the Fabric `ShowRunner`): plays its cues tick by tick, keeps the
//! viewers' camera and sky in step, and on the way out puts everything back. A rehearsal
//! plays for one admin only.

use std::{mem, str::FromStr as _, sync::Arc};

use glam::{DVec3, Quat, Vec3};
use rustc_hash::FxHashMap;
use steel_core::{
    chunk::heightmap::HeightmapType,
    entity::{Entity as _, next_entity_id},
    player::Player,
    server::Server,
    world::{LevelReader, World},
};
use steel_protocol::packets::game::{
    BlockChange, CAddEntity, CBlockUpdate, CClearTitles, CGameEvent, CLevelParticles,
    CRemoveEntities, CSectionBlocksUpdate, CSetActionBarText, CSetCamera, CSetSubtitleText,
    CSetTime, CSetTitleText, CSetTitlesAnimation, CSound, GameEventType, RandomizationType,
    SoundSource,
};
use steel_registry::{
    REGISTRY, RegistryEntry as _, RegistryExt as _,
    particle_type::{ParticleData, ParticleTypeRef},
    sound_event::SoundEventHolder,
    vanilla_entities,
};
use steel_utils::{
    BlockPos, BlockStateId, ChunkPos, Identifier, PackedSectionBlockPos, SectionPos,
    types::{GameType, UpdateFlags},
};
use text_components::{
    Modifier as _, TextComponent,
    interactivity::{ClickEvent, HoverEvent},
};
use uuid::Uuid;

use super::{
    actor::{Actor, Pose},
    blocks::{BlockSet, Placed, block_state, rotation, state_text},
    camera_path::{CameraPath, look, wrap},
    effects::{Fx, Rift, RocketEnd, Shower, Volley},
    journal::{Journal, ViewerState},
    permanent,
    script::{Action, BlockMode, BlockSource, Cue, Gather, PathSource, ShowScript, Stage},
};
use crate::{blocklog::BlockLog, moderation::format::render};

/// Every how many ticks the viewers are sent the ghost blocks again.
const GHOST_RESEND_TICKS: u32 = 40;
/// How long a bolt of lightning is left with the viewers: their game plays it out in less.
const BOLT_TICKS: u32 = 20;
/// Who the block log says made a show's permanent changes.
const LOG_USER: &str = "#show";

/// How far from the stage a viewer still gets the show.
const VIEW_DISTANCE: f64 = 256.0;
/// How far a viewer may be from the camera they look through before they are carried to it,
/// so the chunks around the camera are the ones they have.
const CAMERA_LEASH: f64 = 24.0;
/// How often the camera and the weather are told again, ticks.
const RESEND_TICKS: u32 = 20;

/// Who a show plays for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Everyone online.
    Live,
    /// One admin, otherwise the same.
    Rehearsal,
}

/// What a running cue remembers from its start.
enum Held {
    Nothing,
    Move { from: DVec3, yaw: f32 },
    Shape { rotation: Quat, scale: Vec3 },
    Time { from: f64, target: f64 },
    Weather { rain: f32, thunder: f32 },
    Meteors(Box<Shower>),
    Rift(Box<Rift>),
    Rockets(Box<Volley>),
}

impl Held {
    /// Takes away what an effect still shows.
    fn clear(&mut self, fx: &Fx<'_>) {
        match self {
            Self::Meteors(shower) => shower.clear(fx),
            Self::Rift(rift) => rift.clear(fx),
            Self::Rockets(volley) => volley.clear(fx),
            _ => {}
        }
    }

    /// A viewer's game was started again and has none of the effect's entities.
    fn forget(&mut self, id: Uuid) {
        match self {
            Self::Meteors(shower) => shower.forget(id),
            Self::Rift(rift) => rift.forget(id),
            Self::Rockets(volley) => volley.forget(id),
            _ => {}
        }
    }
}

struct Active {
    cue: usize,
    held: Held,
}

/// What a step of a show works with.
pub struct Cx<'a> {
    pub server: &'a Arc<Server>,
    /// Everyone online.
    pub online: &'a [Arc<Player>],
    pub journal: &'a mut Journal,
    /// Where permanent changes of a show are logged, if the block log is on.
    pub blocklog: Option<&'a Arc<BlockLog>>,
}

/// A show that is playing.
pub struct Runner {
    script: Arc<ShowScript>,
    /// The recorded camera paths of the script, read when the show started.
    paths: FxHashMap<String, CameraPath>,
    /// The captured block sets of the script, read when the show started.
    sets: FxHashMap<String, BlockSet>,
    /// Blocks only the viewers see, over what is really there.
    ghosts: FxHashMap<BlockPos, BlockStateId>,
    /// Ghosts were added while nothing was sent (skipping to a marker, a new viewer).
    ghosts_unsent: bool,
    /// Lightning the viewers were shown, and the tick it is taken away.
    bolts: Vec<(i32, u32)>,
    /// The show's places apart from its stage, where meteors fall.
    places: FxHashMap<String, Stage>,
    /// The rifts that are open, by name: where rockets fly.
    rifts: FxHashMap<String, DVec3>,
    world: Arc<World>,
    origin: DVec3,
    mode: Mode,
    next: usize,
    tick: u32,
    /// Skipping ahead to a marker: cues run, nothing is sent.
    muted: bool,
    finished: bool,
    active: Vec<Active>,
    actors: Vec<Option<Actor>>,
    viewers: Vec<Uuid>,
    spots: FxHashMap<Uuid, DVec3>,
    next_spot: u32,
    time: Option<f64>,
    weather: Option<(f32, f32)>,
    cinematic: Option<usize>,
    /// The entity each viewer's camera is bound to.
    bound: FxHashMap<Uuid, i32>,
    shake: f64,
}

impl Runner {
    /// A run of `script` in `world`, at the script's stage.
    #[must_use]
    pub fn new(
        script: Arc<ShowScript>,
        paths: FxHashMap<String, CameraPath>,
        sets: FxHashMap<String, BlockSet>,
        world: Arc<World>,
        origin: DVec3,
        mode: Mode,
    ) -> Self {
        let actors = script.actors.iter().map(|_| None).collect();
        Self {
            script,
            paths,
            sets,
            ghosts: FxHashMap::default(),
            ghosts_unsent: false,
            bolts: Vec::new(),
            places: FxHashMap::default(),
            rifts: FxHashMap::default(),
            world,
            origin,
            mode,
            next: 0,
            tick: 0,
            muted: false,
            finished: false,
            active: Vec::new(),
            actors,
            viewers: Vec::new(),
            spots: FxHashMap::default(),
            next_spot: 0,
            time: None,
            weather: None,
            cinematic: None,
            bound: FxHashMap::default(),
            shake: 0.0,
        }
    }

    #[must_use]
    pub fn script(&self) -> &ShowScript {
        &self.script
    }

    #[must_use]
    pub const fn mode(&self) -> Mode {
        self.mode
    }

    #[must_use]
    pub const fn tick(&self) -> u32 {
        self.tick
    }

    #[must_use]
    pub const fn finished(&self) -> bool {
        self.finished
    }

    #[must_use]
    pub fn is_viewer(&self, id: Uuid) -> bool {
        self.viewers.contains(&id)
    }

    /// The show's spots (`/show spot`), as they are when it starts.
    pub fn set_places(&mut self, places: FxHashMap<String, Stage>) {
        self.places = places;
    }

    /// What the effects of this tick work with.
    fn fx<'a>(&'a self, cx: &'a Cx<'_>) -> Fx<'a> {
        Fx {
            tick: self.tick,
            muted: self.muted,
            server: cx.server,
            world: &self.world,
            viewers: self.viewers_online(cx.online),
            online: cx.online,
        }
    }

    /// A stage-relative position in world coordinates.
    fn abs(&self, relative: DVec3) -> DVec3 {
        self.origin + relative
    }

    /// The viewers who are online.
    #[must_use]
    pub fn viewers_online<'a>(&self, online: &'a [Arc<Player>]) -> Vec<&'a Arc<Player>> {
        online
            .iter()
            .filter(|player| self.viewers.contains(&player.gameprofile.id()))
            .collect()
    }

    fn in_world(&self, player: &Player) -> bool {
        Arc::ptr_eq(&player.get_world(), &self.world)
    }

    /// Viewers in the show's world close enough to see it, or looking through its camera.
    fn viewers_here<'a>(&self, online: &'a [Arc<Player>]) -> Vec<&'a Arc<Player>> {
        self.viewers_online(online)
            .into_iter()
            .filter(|player| {
                self.in_world(player)
                    && (self.bound.contains_key(&player.gameprofile.id())
                        || player.position().distance_squared(self.origin)
                            < VIEW_DISTANCE * VIEW_DISTANCE)
            })
            .collect()
    }

    // --- Actors -------------------------------------------------------------------------

    fn actor(&self, index: usize) -> Option<&Actor> {
        self.actors.get(index).and_then(Option::as_ref)
    }

    /// The actor, spawned at the stage center if no cue has spawned it yet.
    fn actor_or_spawn(&mut self, index: usize) -> Option<&mut Actor> {
        if self.actor(index).is_none() {
            let def = self.script.actors.get(index)?;
            let actor = Actor::spawn(def, &self.sets, self.origin, 0.0, None);
            *self.actors.get_mut(index)? = Some(actor);
        }
        self.actors.get_mut(index).and_then(Option::as_mut)
    }

    fn spawn_actor(
        &mut self,
        index: usize,
        pos: DVec3,
        yaw: f32,
        scale: Option<Vec3>,
        cx: &mut Cx<'_>,
    ) {
        self.remove_actor(index, cx);
        let Some(def) = self.script.actors.get(index) else {
            return;
        };
        let actor = Actor::spawn(def, &self.sets, pos, yaw, scale);
        if let Some(slot) = self.actors.get_mut(index) {
            *slot = Some(actor);
        }
    }

    fn remove_actor(&mut self, index: usize, cx: &mut Cx<'_>) {
        let Some(mut actor) = self.actors.get_mut(index).and_then(Option::take) else {
            return;
        };
        if self.cinematic == Some(index) {
            self.set_cinematic(None, cx);
        }
        actor.remove(cx.online);
    }

    // --- Camera and sky -----------------------------------------------------------------

    fn set_cinematic(&mut self, actor: Option<usize>, cx: &mut Cx<'_>) {
        if let Some(index) = actor {
            self.actor_or_spawn(index);
        }
        let old = mem::replace(&mut self.cinematic, actor);
        if old.is_some() && actor.is_none() && !self.muted {
            let online = cx.online;
            for player in self.viewers_online(online) {
                self.release_camera(player, cx.journal);
            }
        }
    }

    fn clock(&self) -> Option<(i32, f64)> {
        let clock = self.world.dimension_type.default_clock?;
        let id = REGISTRY.world_clocks.id_from_key(&clock.key)?;
        let now = self.world.clock_total_ticks(clock)?;
        Some((i32::try_from(id).ok()?, now as f64))
    }

    fn current_time(&self) -> f64 {
        self.time
            .or_else(|| self.clock().map(|(_, now)| now))
            .unwrap_or(6000.0)
    }

    fn current_weather(&self) -> (f32, f32) {
        self.weather.unwrap_or_else(|| {
            let weather = self.world.weather.lock();
            (weather.rain_level, weather.thunder_level)
        })
    }

    fn set_weather(&mut self, rain: f32, thunder: f32, cx: &Cx<'_>) {
        let was_dry = self.current_weather().0 <= 0.0;
        self.weather = Some((rain, thunder));
        if !self.muted {
            for player in self.viewers_here(cx.online) {
                send_weather(player, (rain, thunder), was_dry);
            }
        }
    }

    // --- Viewers ------------------------------------------------------------------------

    /// Takes a player into the show: their state goes to the journal, they can't be hurt
    /// or build, and (if the show gathers its viewers) they stand in the ring.
    pub fn add_viewer(&mut self, player: &Arc<Player>, journal: &mut Journal) {
        let id = player.gameprofile.id();
        if self.viewers.contains(&id) {
            return;
        }
        self.viewers.push(id);
        // Their spot before the show; kept if they were already saved (left and came back).
        journal.save_viewer(id, viewer_state(player, self.script.return_after));
        journal.flush();
        self.ghosts_unsent = true;

        if player.game_mode() == GameType::Survival {
            player.set_game_mode(GameType::Adventure); // watching, not building on stage
        }
        protect(player);

        if self.script.gather == Gather::Teleport {
            let spot = if let Some(spot) = self.spots.get(&id) {
                *spot
            } else {
                let spot = self.spot(self.next_spot);
                self.next_spot += 1;
                self.spots.insert(id, spot);
                spot
            };
            self.go_to(player, spot);
        }
        if let Some(weather) = self.weather {
            send_weather(player, weather, true);
        }
    }

    /// Where the n-th viewer stands: on a ring around the stage, on the ground.
    fn spot(&self, index: u32) -> DVec3 {
        let center = self.abs(self.script.gather_center);
        let (x, z) = ring(center, self.script.gather_radius, index);
        let ground = LevelReader::height_at(
            &*self.world,
            HeightmapType::MotionBlockingNoLeaves,
            x.floor() as i32,
            z.floor() as i32,
        );
        // A column that is not loaded has no ground: stand at the height of the stage.
        let y = if ground > LevelReader::min_y(&*self.world) {
            f64::from(ground)
        } else {
            self.origin.y
        };
        DVec3::new(x, y, z)
    }

    fn go_to(&self, player: &Arc<Player>, spot: DVec3) {
        let eyes = spot + DVec3::new(0.0, player.get_eye_height(), 0.0);
        let target = self.abs(self.script.gather_center) + DVec3::new(0.0, 4.0, 0.0);
        if !player.teleport_to_world(&self.world, spot, look(eyes, target)) {
            log::warn!(
                "MineHouse events: {} could not be taken to the show",
                player.gameprofile.name()
            );
        }
    }

    /// Takes a viewer out of the camera, back to their spot (or where they stood before).
    fn release_camera(&mut self, player: &Arc<Player>, journal: &Journal) {
        let id = player.gameprofile.id();
        self.bound.remove(&id);
        player.send_packet(CSetCamera {
            camera_id: player.id(),
        });
        let saved = journal.viewer(id);
        let mode = saved.map_or(GameType::Survival, |saved| GameType::from(saved.game_mode));
        player.set_game_mode(if mode == GameType::Survival {
            GameType::Adventure
        } else {
            mode
        });
        protect(player);
        if let Some(spot) = self.spots.get(&id) {
            self.go_to(player, *spot);
        } else if let Some(saved) = saved
            && saved.world == self.world.key.to_string()
        {
            let [x, y, z] = saved.pos;
            player.teleport_to_world(&self.world, DVec3::new(x, y, z), (saved.yaw, saved.pitch));
        }
    }

    /// The player's game was started again: it has none of the show's entities and looks
    /// through no camera.
    pub fn rejoined(&mut self, id: Uuid) {
        self.bound.remove(&id);
        for actor in self.actors.iter_mut().flatten() {
            actor.forget(id);
        }
        for running in &mut self.active {
            running.held.forget(id);
        }
    }

    /// Out of the show early (`/show optout`): back where and how they were.
    pub fn drop_viewer(&mut self, player: &Arc<Player>, cx: &mut Cx<'_>) {
        let id = player.gameprofile.id();
        let Some(index) = self.viewers.iter().position(|viewer| *viewer == id) else {
            return;
        };
        self.viewers.remove(index);
        let here = self.viewers_here(cx.online);
        for actor in self.actors.iter_mut().flatten() {
            actor.sync(&here, cx.online);
        }
        if self.in_world(player) && !self.ghosts.is_empty() {
            send_blocks(player, &self.real_under_ghosts());
        }
        self.leave(player, cx);
    }

    /// Out of the show for good: down from the camera onto their spot when they stay, then
    /// mode, sky and (if the show says so) place back.
    fn leave(&mut self, player: &Arc<Player>, cx: &mut Cx<'_>) {
        let stays = !self.script.return_after
            && player.game_mode() == GameType::Spectator
            && cx
                .journal
                .viewer(player.gameprofile.id())
                .is_some_and(|saved| GameType::from(saved.game_mode) != GameType::Spectator);
        if stays {
            self.release_camera(player, cx.journal);
        }
        self.bound.remove(&player.gameprofile.id());
        restore_viewer(cx.server, player, cx.journal);
    }

    // --- Running ------------------------------------------------------------------------

    /// Starts the show for `audience`; `from_tick` skips ahead to a marker.
    pub fn start(&mut self, audience: &[Arc<Player>], from_tick: u32, cx: &mut Cx<'_>) {
        cx.journal.set_active_show(Some(&self.script.id));
        cx.journal.flush();
        for player in audience {
            self.add_viewer(player, cx.journal);
        }
        if from_tick > 0 {
            // Play everything before the marker at once, silently, so the stage is set.
            self.muted = true;
            while self.tick < from_tick && !self.finished {
                self.step(cx);
            }
            self.muted = false;
            if let Some(weather) = self.weather {
                for player in self.viewers_here(cx.online) {
                    send_weather(player, weather, true);
                }
            }
        }
    }

    /// One server tick.
    pub fn step(&mut self, cx: &mut Cx<'_>) {
        if self.finished {
            return;
        }
        self.shake = 0.0;
        self.step_timeline(cx);
        if !self.muted {
            self.keep_viewers(cx);
        }
        self.tick += 1;
        if self.tick > self.script.length {
            self.finish(cx);
        }
    }

    fn step_timeline(&mut self, cx: &mut Cx<'_>) {
        let script = Arc::clone(&self.script);
        while let Some(cue) = script.cues.get(self.next)
            && cue.at <= self.tick
        {
            let index = self.next;
            self.next += 1;
            let mut held = self.start_cue(cue, cx);
            if cue.duration == 0 {
                self.end_cue(cue, &mut held, cx);
            } else {
                self.update_cue(cue, &mut held, 0.0, cx);
                self.active.push(Active { cue: index, held });
            }
        }

        for mut running in mem::take(&mut self.active) {
            let Some(cue) = script.cues.get(running.cue) else {
                continue;
            };
            let elapsed = self.tick.saturating_sub(cue.at);
            if elapsed == 0 {
                self.active.push(running);
                continue;
            }
            let progress = (f64::from(elapsed) / f64::from(cue.duration)).min(1.0);
            self.update_cue(cue, &mut running.held, progress, cx);
            if elapsed >= cue.duration {
                self.end_cue(cue, &mut running.held, cx);
            } else {
                self.active.push(running);
            }
        }
    }

    /// Called once when a cue's time comes.
    fn start_cue(&mut self, cue: &Cue, cx: &mut Cx<'_>) -> Held {
        match &cue.action {
            Action::Cinematic { on, actor } => self.set_cinematic(on.then_some(*actor), cx),
            Action::Cut {
                actor,
                pos,
                look: target,
                yaw,
                pitch,
            } => {
                let (yaw, pitch) = target.map_or((*yaw, *pitch), |target| look(*pos, target));
                let pos = self.abs(*pos);
                if let Some(actor) = self.actor_or_spawn(*actor) {
                    actor.move_to(pos, yaw, pitch);
                }
            }
            Action::Spawn {
                actor,
                pos,
                yaw,
                scale,
            } => self.spawn_actor(*actor, self.abs(*pos), *yaw, *scale, cx),
            Action::Move { actor, .. } => {
                if let Some(actor) = self.actor_or_spawn(*actor) {
                    return Held::Move {
                        from: actor.pos(),
                        yaw: actor.yaw(),
                    };
                }
            }
            Action::Transform {
                actor,
                ease,
                rotation,
                scale,
            } => {
                let Some(actor) = self.actor_or_spawn(*actor) else {
                    return Held::Nothing;
                };
                let held = Held::Shape {
                    rotation: actor.rotation(),
                    scale: actor.scale(),
                };
                if ease.is_linear() || cue.duration == 0 {
                    // The client blends a linear change by itself, from one update.
                    let turned = rotation.unwrap_or_else(|| actor.rotation());
                    let sized = scale.unwrap_or_else(|| actor.scale());
                    let ticks = i32::try_from(cue.duration).unwrap_or(i32::MAX);
                    actor.set_shape(turned, sized, ticks);
                }
                return held;
            }
            Action::Remove { actor } => self.remove_actor(*actor, cx),
            Action::Time { to, backward, .. } => {
                let from = self.current_time();
                let mut target = (from / 24000.0).floor() * 24000.0 + to;
                // The sky keeps turning forward unless the script asks otherwise.
                if !backward && target < from {
                    target += 24000.0;
                } else if *backward && target > from {
                    target -= 24000.0;
                }
                return Held::Time { from, target };
            }
            Action::Weather { .. } => {
                let (rain, thunder) = self.current_weather();
                return Held::Weather { rain, thunder };
            }
            Action::Particles { .. }
            | Action::Sound { .. }
            | Action::Title { .. }
            | Action::Bar { .. }
            | Action::Chat { .. }
            | Action::Explosion { .. } => self.emit(&cue.action, cx),
            Action::Blocks { mode, source, pos } => {
                let blocks = self.blocks_of(source, *pos);
                match mode {
                    BlockMode::Ghost => self.ghost(&blocks, cx.online),
                    BlockMode::Temporary => self.place_real(&blocks, true, cx),
                    // A rehearsal never changes the world for good.
                    BlockMode::Permanent => {
                        self.place_real(&blocks, self.mode == Mode::Rehearsal, cx);
                    }
                }
            }
            Action::ClearBlocks { ghost, temporary } => {
                if *ghost {
                    self.clear_ghosts(cx.online);
                }
                if *temporary {
                    restore_blocks(cx.server, cx.journal);
                    cx.journal.flush();
                }
            }
            Action::Lightning { pos } => self.strike(*pos, cx.online),
            Action::Meteors(_) | Action::Rockets(_) | Action::Rift(_) => {
                return self.start_effect(cue, cx);
            }
            Action::Path { .. } | Action::Shake { .. } | Action::Shockwave(_) => {}
        }
        Held::Nothing
    }

    /// Called every tick of a cue with a duration; `progress` goes from 0 to 1, before any
    /// easing.
    /// What a big effect keeps while it runs.
    fn start_effect(&mut self, cue: &Cue, cx: &Cx<'_>) -> Held {
        match &cue.action {
            Action::Meteors(_) => Held::Meteors(Box::default()),
            Action::Rockets(_) => Held::Rockets(Box::default()),
            Action::Rift(def) => {
                let center = self.abs(def.pos);
                let rift = Rift::open(def, cue.at, center, &self.fx(cx));
                self.rifts.insert(def.name.clone(), center);
                Held::Rift(Box::new(rift))
            }
            _ => Held::Nothing,
        }
    }

    /// One tick of a cue that is a big effect.
    fn update_effect(&mut self, cue: &Cue, held: &mut Held, progress: f64, cx: &Cx<'_>) {
        let elapsed = self.tick.saturating_sub(cue.at);
        match (&cue.action, held) {
            (Action::Meteors(def), Held::Meteors(shower)) => {
                shower.update(def, cue.duration, elapsed, &self.places, &self.fx(cx));
            }
            (Action::Rift(def), Held::Rift(rift)) => {
                rift.update(def, cue.duration, elapsed, &self.fx(cx));
                if rift.closing {
                    self.rifts.remove(&def.name);
                }
            }
            (Action::Rockets(def), Held::Rockets(volley)) => {
                let ends: Vec<DVec3> = def
                    .between
                    .iter()
                    .filter_map(|end| match end {
                        RocketEnd::Rift(name) => self.rifts.get(name).copied(),
                        RocketEnd::Point(pos) => Some(self.abs(*pos)),
                    })
                    .collect();
                volley.update(def, cue.duration, elapsed, &ends, &self.fx(cx));
            }
            (Action::Shockwave(def), _) => {
                def.update(self.abs(def.pos), progress, &self.fx(cx));
            }
            _ => {}
        }
    }

    fn update_cue(&mut self, cue: &Cue, held: &mut Held, progress: f64, cx: &mut Cx<'_>) {
        self.update_effect(cue, held, progress, cx);
        match (&cue.action, &*held) {
            (Action::Path { actor, ease, path }, _) => {
                let at = ease.apply(progress);
                let pose = match path {
                    PathSource::Inline(path) => Some(path.sample(at)),
                    PathSource::Recorded { name, look: target } => {
                        self.paths.get(name).map(|path| {
                            let mut pose = path.sample(at);
                            if let Some(target) = target {
                                (pose.yaw, pose.pitch) = look(pose.pos, *target);
                            }
                            pose
                        })
                    }
                };
                // A recorded path that is missing was refused when the show started.
                if let Some(pose) = pose {
                    let pos = self.abs(pose.pos);
                    if let Some(actor) = self.actor_or_spawn(*actor) {
                        actor.move_to(pos, pose.yaw, pose.pitch);
                    }
                }
            }
            (Action::Shake { strength }, _) => {
                self.shake = self.shake.max(strength * (1.0 - progress));
            }
            (
                Action::Move {
                    actor,
                    ease,
                    to,
                    yaw: to_yaw,
                },
                Held::Move { from, yaw },
            ) => {
                let eased = ease.apply(progress);
                let turned = to_yaw.map_or(*yaw, |to| yaw + wrap(to - yaw) * eased as f32);
                let pos = from.lerp(self.abs(*to), eased);
                if let Some(actor) = self.actor_or_spawn(*actor) {
                    let pitch = actor.pitch();
                    actor.move_to(pos, turned, pitch);
                }
            }
            (
                Action::Transform {
                    actor,
                    ease,
                    rotation: to_rotation,
                    scale: to_scale,
                },
                Held::Shape { rotation, scale },
            ) => {
                if ease.is_linear() || progress == 0.0 {
                    return;
                }
                let eased = ease.apply(progress) as f32;
                let turned = rotation.slerp(to_rotation.unwrap_or(*rotation), eased);
                let sized = scale.lerp(to_scale.unwrap_or(*scale), eased);
                if let Some(actor) = self.actor_or_spawn(*actor) {
                    actor.set_shape(turned, sized, 1);
                }
            }
            (Action::Time { ease, .. }, Held::Time { from, target }) => {
                self.time = Some(from + (target - from) * ease.apply(progress));
            }
            (
                Action::Weather {
                    rain: to_rain,
                    thunder: to_thunder,
                },
                Held::Weather { rain, thunder },
            ) => {
                let progress = progress as f32;
                self.set_weather(
                    rain + (to_rain - rain) * progress,
                    thunder + (to_thunder - thunder) * progress,
                    cx,
                );
            }
            (Action::Particles { .. }, _) if progress > 0.0 => self.emit(&cue.action, cx),
            _ => {}
        }
    }

    /// Called once after the last update, or right after the start of a cue without a
    /// duration: whatever the cue changes over time is at its end.
    fn end_cue(&mut self, cue: &Cue, held: &mut Held, cx: &mut Cx<'_>) {
        if let Action::Rift(def) = &cue.action {
            self.rifts.remove(&def.name);
        }
        held.clear(&self.fx(cx));
        match (&cue.action, &*held) {
            (Action::Move { .. }, _) => self.update_cue(cue, held, 1.0, cx),
            (Action::Time { .. }, Held::Time { target, .. }) => self.time = Some(*target),
            (Action::Weather { rain, thunder }, _) if cue.duration == 0 => {
                self.set_weather(*rain, *thunder, cx);
            }
            _ => {}
        }
    }

    /// A burst of one particle for the viewers in the show's world.
    fn burst(
        &self,
        particle: ParticleTypeRef,
        at: DVec3,
        spread: Vec3,
        speed: f32,
        count: i32,
        cx: &Cx<'_>,
    ) {
        for player in self.viewers_here(cx.online) {
            player.send_packet(CLevelParticles {
                particle: ParticleData::simple(particle),
                override_limiter: true,
                always_show: true,
                x: at.x,
                y: at.y,
                z: at.z,
                x_dist: spread.x,
                y_dist: spread.y,
                z_dist: spread.z,
                x_max_speed: speed,
                y_max_speed: speed,
                z_max_speed: speed,
                count,
                randomization_type: RandomizationType::default(),
            });
        }
    }

    /// The look and the sound of an explosion; nothing in the world breaks.
    fn explode(&self, at: DVec3, sound: bool, cx: &Cx<'_>) {
        if let Some(particle) = REGISTRY
            .particle_types
            .by_key(&Identifier::vanilla_static("explosion_emitter"))
        {
            self.burst(particle, at, Vec3::ZERO, 0.0, 1, cx);
        }
        let boom = REGISTRY
            .sound_events
            .by_key(&Identifier::vanilla_static("entity.generic.explode"))
            .filter(|_| sound);
        if let Some(boom) = boom {
            let seed = rand::random();
            for player in self.viewers_here(cx.online) {
                player.send_packet(CSound::new_holder(
                    SoundEventHolder::Registry(boom),
                    SoundSource::Master,
                    at,
                    4.0,
                    0.8,
                    seed,
                ));
            }
        }
    }

    /// What a cue sends to the viewers and nothing else: sounds, particles, texts.
    fn emit(&self, action: &Action, cx: &Cx<'_>) {
        if self.muted {
            return;
        }
        match action {
            Action::Sound {
                sound,
                source,
                volume,
                pitch,
                pos,
                actor,
            } => {
                let place = pos
                    .map(|pos| self.abs(pos))
                    .or_else(|| actor.and_then(|actor| self.actor(actor)).map(Actor::pos));
                let seed = rand::random();
                for player in self.viewers_here(cx.online) {
                    // Without a place the sound plays right at the viewer, like music.
                    let at = place.unwrap_or_else(|| {
                        let feet = player.position();
                        DVec3::new(feet.x, player.get_eye_y(), feet.z)
                    });
                    player.send_packet(CSound::new_holder(
                        sound.clone(),
                        *source,
                        at,
                        *volume,
                        *pitch,
                        seed,
                    ));
                }
            }
            Action::Particles {
                particle,
                pos,
                actor,
                count,
                spread,
                speed,
            } => {
                let offset = pos.unwrap_or(DVec3::ZERO);
                let at = actor
                    .and_then(|actor| self.actor(actor))
                    .map_or_else(|| self.abs(offset), |actor| actor.pos() + offset);
                self.burst(particle, at, *spread, *speed, *count, cx);
            }
            Action::Title {
                text,
                sub,
                fade_in,
                stay,
                fade_out,
            } => {
                let ticks = |ticks: &u32| i32::try_from(*ticks).unwrap_or(i32::MAX);
                for player in self.viewers_online(cx.online) {
                    player.send_packet(CSetTitlesAnimation::new(
                        ticks(fade_in),
                        ticks(stay),
                        ticks(fade_out),
                    ));
                    player.send_packet(CSetSubtitleText::new(render(sub, &[])));
                    player.send_packet(CSetTitleText::new(render(text, &[])));
                }
            }
            Action::Explosion { pos, sound } => self.explode(self.abs(*pos), *sound, cx),
            Action::Bar { text } => {
                for player in self.viewers_online(cx.online) {
                    player.send_packet(CSetActionBarText::new(render(text, &[])));
                }
            }
            Action::Chat { text, link } => {
                let mut message = render(text, &[]);
                if let Some(link) = link {
                    // The whole message opens the link on click.
                    message = TextComponent::new()
                        .add_child(message)
                        .click_event(ClickEvent::open_url(link.clone()))
                        .hover_event(HoverEvent::show_text(TextComponent::plain(link.clone())));
                }
                for player in self.viewers_online(cx.online) {
                    player.send_message(&message);
                }
            }
            _ => {}
        }
    }

    /// Every tick: actors, the camera and the sky of every viewer are what the show says.
    fn keep_viewers(&mut self, cx: &mut Cx<'_>) {
        let camera = self.cinematic.and_then(|index| {
            let shake = self.shake;
            let actor = self.actors.get_mut(index)?.as_mut()?;
            let nudged = (shake > 0.0).then(|| jitter(actor, shake));
            actor.nudge(nudged);
            Some((actor.id(), actor.pos(), actor.yaw(), actor.pitch()))
        });

        let online = cx.online;
        if let Some((_, pos, yaw, pitch)) = camera {
            for player in self.viewers_online(online) {
                if !self.in_world(player) {
                    continue;
                }
                let id = player.gameprofile.id();
                if player.game_mode() != GameType::Spectator {
                    player.set_game_mode(GameType::Spectator);
                }
                if player.position().distance_squared(pos) > CAMERA_LEASH * CAMERA_LEASH
                    && let Err(error) = player.teleport(pos, yaw, pitch)
                {
                    log::debug!("MineHouse events: camera teleport failed: {error:?}");
                }
                // Marked first: whoever looks through the camera gets the actors below.
                self.bound.entry(id).or_insert(0);
            }
        }

        let here = self.viewers_here(online);
        for actor in self.actors.iter_mut().flatten() {
            actor.sync(&here, online);
        }

        if let Some((entity, ..)) = camera {
            let again = self.tick.is_multiple_of(RESEND_TICKS);
            for player in &here {
                // Told again now and then: a respawn puts the view back into the player.
                if let Some(bound) = self.bound.get_mut(&player.gameprofile.id())
                    && (*bound != entity || again)
                {
                    *bound = entity;
                    player.send_packet(CSetCamera { camera_id: entity });
                }
            }
        }

        if let Some(time) = self.time
            && let Some((clock, _)) = self.clock()
        {
            let whole = time.floor();
            for player in self.viewers_online(online) {
                if self.in_world(player) {
                    // Rate 0: the sky holds still between our updates.
                    player.send_packet(CSetTime::new(
                        self.world.game_time(),
                        vec![(clock, whole as i64, (time - whole) as f32, 0.0)],
                    ));
                }
            }
        }
        if self.tick.is_multiple_of(RESEND_TICKS)
            && let Some(weather) = self.weather
        {
            // The real weather cycle may have broadcast over ours.
            for player in &here {
                send_weather(player, weather, false);
            }
        }
        // A chunk sent again (the viewer walked off and back) would show the real blocks.
        let resend = self.ghosts_unsent || self.tick.is_multiple_of(GHOST_RESEND_TICKS);
        if resend && !self.ghosts.is_empty() && !self.muted {
            for player in &here {
                send_blocks(player, &self.ghosts);
            }
            self.ghosts_unsent = false;
        }
        let tick = self.tick;
        if self.bolts.iter().any(|(_, until)| *until <= tick) {
            let gone: Vec<i32> = self
                .bolts
                .iter()
                .filter(|(_, until)| *until <= tick)
                .map(|(id, _)| *id)
                .collect();
            self.bolts.retain(|(_, until)| *until > tick);
            for player in self.viewers_online(online) {
                player.send_packet(CRemoveEntities {
                    entity_ids: gone.clone(),
                });
            }
        }
    }

    // --- Blocks -------------------------------------------------------------------------

    /// The blocks a `blocks` cue puts, at their places in the world.
    fn blocks_of(&self, source: &BlockSource, pos: Option<DVec3>) -> Placed {
        match source {
            BlockSource::Shape(shape) => {
                let center = block_pos(self.abs(pos.unwrap_or(DVec3::ZERO)));
                shape.blocks(center, |x, z| {
                    let height = LevelReader::height_at(
                        &*self.world,
                        HeightmapType::MotionBlockingNoLeaves,
                        x,
                        z,
                    );
                    height - 1
                })
            }
            BlockSource::Set(name) => self.sets.get(name).map_or_else(Placed::new, |set| {
                // A show that turns with its stage turns its sets too, blocks facing the
                // right way. Without a place the set goes back where it was captured.
                let origin = block_pos(self.abs(pos.unwrap_or_else(|| set.anchor())));
                set.at(origin, rotation(self.script.turned))
            }),
        }
    }

    /// Shows `blocks` to the viewers over what is really there.
    fn ghost(&mut self, blocks: &Placed, online: &[Arc<Player>]) {
        let mut added = FxHashMap::default();
        for (pos, state) in blocks {
            self.ghosts.insert(*pos, *state);
            added.insert(*pos, *state);
        }
        if self.muted {
            self.ghosts_unsent = true;
            return;
        }
        for player in self.viewers_here(online) {
            send_blocks(player, &added);
        }
    }

    /// Takes the ghosts away: the viewers see the real blocks again.
    fn clear_ghosts(&mut self, online: &[Arc<Player>]) {
        if self.ghosts.is_empty() {
            return;
        }
        let real = self.real_under_ghosts();
        for player in self.viewers_here(online) {
            send_blocks(player, &real);
        }
        self.ghosts.clear();
        self.ghosts_unsent = false;
    }

    fn real_under_ghosts(&self) -> FxHashMap<BlockPos, BlockStateId> {
        self.ghosts
            .keys()
            .map(|pos| (*pos, self.world.get_block_state(*pos)))
            .collect()
    }

    /// Real blocks. Temporary ones are journaled and put back at the end; permanent ones
    /// stay, with what they replaced written down for `/show undo` and in the block log.
    fn place_real(&self, blocks: &Placed, temporary: bool, cx: &mut Cx<'_>) {
        let world = self.world.key.to_string();
        let mut replaced = Vec::new();
        let mut skipped = 0;
        for (pos, state) in blocks {
            if !loaded(&self.world, *pos) {
                skipped += 1;
                continue;
            }
            let old = self.world.get_block_state(*pos);
            if temporary {
                cx.journal
                    .save_block(&world, [pos.x(), pos.y(), pos.z()], || state_text(old));
            } else {
                replaced.push((*pos, old));
                if let Some(blocklog) = cx.blocklog {
                    blocklog.record_change(LOG_USER, &self.world, *pos, old, *state);
                }
            }
            // Without neighbor updates: no sand falling or water flowing out of the scene.
            self.world
                .set_block(*pos, *state, UpdateFlags::UPDATE_CLIENTS);
        }
        if skipped > 0 {
            log::warn!(
                "MineHouse events: show {} skipped {skipped} block(s) in chunks that are not \
                 loaded",
                self.script.id
            );
        }
        if temporary {
            // On disk before anyone sees the change: a crash now still puts them back.
            cx.journal.flush();
        } else if let Err(error) = permanent::backup(&self.script.id, &world, &replaced) {
            log::error!(
                "MineHouse events: could not back up {} block(s) of show {} before a permanent \
                 change: {error}",
                replaced.len(),
                self.script.id
            );
        }
    }

    /// A bolt of lightning the viewers see and hear; nothing burns.
    fn strike(&mut self, pos: DVec3, online: &[Arc<Player>]) {
        if self.muted {
            return;
        }
        let id = next_entity_id();
        let at = self.abs(pos);
        for player in self.viewers_here(online) {
            player.send_packet(CAddEntity {
                id,
                uuid: Uuid::new_v4(),
                entity_type: vanilla_entities::LIGHTNING_BOLT.id() as i32,
                position: at,
                velocity: DVec3::ZERO,
                x_rot: 0,
                y_rot: 0,
                head_y_rot: 0,
                data: 0,
            });
        }
        self.bolts.push((id, self.tick + BOLT_TICKS));
    }

    /// Ends the show and puts everything back: actors, sky, camera, viewers.
    pub fn finish(&mut self, cx: &mut Cx<'_>) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.muted = false;
        self.cinematic = None;
        for mut running in mem::take(&mut self.active) {
            running.held.clear(&self.fx(cx));
        }
        self.rifts.clear();
        let online = cx.online;
        for slot in &mut self.actors {
            if let Some(mut actor) = slot.take() {
                actor.remove(online);
            }
        }
        self.clear_ghosts(online);
        restore_blocks(cx.server, cx.journal);
        if !self.bolts.is_empty() {
            let gone: Vec<i32> = self.bolts.drain(..).map(|(id, _)| id).collect();
            for player in self.viewers_online(online) {
                player.send_packet(CRemoveEntities {
                    entity_ids: gone.clone(),
                });
            }
        }
        for player in self.viewers_online(online) {
            player.send_packet(CClearTitles::new(true));
            self.leave(player, cx);
        }
        self.viewers.clear();
        self.bound.clear();
        cx.journal.set_active_show(None);
        cx.journal.flush();
    }
}

/// The block a place is in.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a place in the world, floored to its block"
)]
fn block_pos(at: DVec3) -> BlockPos {
    let at = at.floor();
    BlockPos::new(at.x as i32, at.y as i32, at.z as i32)
}

pub fn loaded(world: &World, pos: BlockPos) -> bool {
    world
        .chunk_map
        .with_full_chunk(ChunkPos::from_block_pos(pos), |_| ())
        .is_some()
}

/// Sends blocks to one player without touching the world, one packet per chunk section
/// rather than per block: a ghost station of ten thousand blocks is a few dozen packets.
fn send_blocks(player: &Player, blocks: &FxHashMap<BlockPos, BlockStateId>) {
    if blocks.len() < 8 {
        for (pos, state) in blocks {
            player.send_packet(CBlockUpdate {
                pos: *pos,
                block_state: *state,
            });
        }
        return;
    }
    let mut sections: FxHashMap<(i32, i32, i32), Vec<BlockChange>> = FxHashMap::default();
    for (pos, state) in blocks {
        sections
            .entry((pos.x() >> 4, pos.y() >> 4, pos.z() >> 4))
            .or_default()
            .push(BlockChange {
                pos: PackedSectionBlockPos::from_block_pos(*pos),
                block_state: *state,
            });
    }
    for ((x, y, z), changes) in sections {
        player.send_packet(CSectionBlocksUpdate {
            section_pos: SectionPos::new(x, y, z),
            changes,
        });
    }
}

/// Puts back the blocks shows replaced for a while. A block in a chunk that is not loaded
/// stays in the journal for the next try; returns how many are left.
pub fn restore_blocks(server: &Server, journal: &mut Journal) -> usize {
    if !journal.has_blocks() {
        return 0;
    }
    journal.restore_blocks(|block| {
        let world = Identifier::from_str(&block.world)
            .ok()
            .and_then(|key| server.worlds.get(&key));
        // A world or a block that no longer exists can never be put back.
        let (Some(world), Ok(state)) = (world, block_state(&block.s)) else {
            return true;
        };
        let pos = BlockPos::new(block.p[0], block.p[1], block.p[2]);
        if !loaded(world, pos) {
            return false;
        }
        if world.get_block_state(pos) != state {
            world.set_block(pos, state, UpdateFlags::UPDATE_CLIENTS);
        }
        true
    })
}

/// The place of the n-th viewer on the ring around `center`: the golden angle spreads the
/// spots evenly however many come, and every 24 a wider ring starts.
fn ring(center: DVec3, radius: f64, index: u32) -> (f64, f64) {
    let angle = f64::from(index) * 2.399_963;
    let ring = radius + f64::from(index / 24) * 3.0;
    (center.x - angle.sin() * ring, center.z + angle.cos() * ring)
}

/// Where a shaking camera is shown this tick.
fn jitter(camera: &Actor, strength: f64) -> Pose {
    let size = strength * 0.35;
    let nudge = || (rand::random::<f64>() - 0.5) * size;
    Pose {
        pos: camera.pos() + DVec3::new(nudge(), nudge(), nudge()),
        yaw: camera.yaw() + (nudge() * 6.0) as f32,
        pitch: camera.pitch() + (nudge() * 6.0) as f32,
    }
}

fn viewer_state(player: &Player, return_back: bool) -> ViewerState {
    let pos = player.position();
    let (yaw, pitch) = player.rotation();
    ViewerState {
        world: player.get_world().key.to_string(),
        pos: [pos.x, pos.y, pos.z],
        yaw,
        pitch,
        game_mode: i32::from(player.game_mode()),
        invulnerable: player.abilities.lock().invulnerable,
        return_back,
    }
}

/// No damage during the show. A game mode change resets abilities, so this comes after it.
fn protect(player: &Player) {
    player.abilities.lock().invulnerable = true;
    player.send_abilities();
}

fn send_weather(player: &Player, (rain, thunder): (f32, f32), announce_start: bool) {
    if announce_start && rain > 0.0 {
        player.send_packet(CGameEvent {
            event: GameEventType::StartRaining,
            data: 0.0,
        });
    }
    player.send_packet(CGameEvent {
        event: GameEventType::RainLevelChange,
        data: rain,
    });
    player.send_packet(CGameEvent {
        event: GameEventType::ThunderLevelChange,
        data: thunder,
    });
}

/// The real sky and weather again, after a show sent its own.
fn resync_sky(player: &Player) {
    let world = player.get_world();
    player.send_packet(world.time_sync_packet());
    let (rain, thunder) = {
        let weather = world.weather.lock();
        (weather.rain_level, weather.thunder_level)
    };
    player.send_packet(CGameEvent {
        event: if world.is_raining() {
            GameEventType::StartRaining
        } else {
            GameEventType::StopRaining
        },
        data: 0.0,
    });
    send_weather(player, (rain, thunder), false);
}

/// Back to where and how the player was before the show.
pub fn restore_viewer(server: &Server, player: &Arc<Player>, journal: &mut Journal) {
    let state = journal.take_viewer(player.gameprofile.id());
    journal.flush();
    let was_watching = player.game_mode() == GameType::Spectator;
    player.send_packet(CSetCamera {
        camera_id: player.id(),
    });
    let Some(state) = state else {
        return;
    };
    let mode = GameType::from(state.game_mode);
    if player.game_mode() != mode {
        player.set_game_mode(mode);
    }
    if mode != GameType::Creative && mode != GameType::Spectator {
        player.abilities.lock().invulnerable = state.invulnerable;
        player.send_abilities();
    }
    if state.return_back {
        let world = Identifier::from_str(&state.world)
            .ok()
            .and_then(|key| server.worlds.get(&key).cloned());
        if let Some(world) = world {
            let [x, y, z] = state.pos;
            player.teleport_to_world(&world, DVec3::new(x, y, z), (state.yaw, state.pitch));
        }
    } else if was_watching && mode != GameType::Spectator {
        // Left while looking through the show camera: they'd hang where it was. Down to
        // the ground.
        let world = player.get_world();
        let pos = player.position();
        let ground = LevelReader::height_at(
            &*world,
            HeightmapType::MotionBlockingNoLeaves,
            pos.x.floor() as i32,
            pos.z.floor() as i32,
        );
        if ground > LevelReader::min_y(&*world) {
            let (yaw, pitch) = player.rotation();
            player.teleport_to_world(
                &world,
                DVec3::new(pos.x, f64::from(ground), pos.z),
                (yaw, pitch),
            );
        }
    }
    resync_sky(player);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewers_spread_over_the_ring() {
        let center = DVec3::new(100.0, 64.0, -50.0);
        let (x, z) = ring(center, 24.0, 0);
        assert!((x - 100.0).abs() < 1e-9 && (z + 26.0).abs() < 1e-9);
        for index in 0..24 {
            let (x, z) = ring(center, 24.0, index);
            let distance = (x - center.x).hypot(z - center.z);
            assert!((distance - 24.0).abs() < 1e-9);
        }
        let (x, z) = ring(center, 24.0, 24);
        assert!(((x - center.x).hypot(z - center.z) - 27.0).abs() < 1e-9);
    }
}
