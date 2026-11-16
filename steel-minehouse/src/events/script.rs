//! A parsed show file (the Fabric `ShowScript` and `Cues.create`). Everything is checked
//! when the file loads, so a typo fails on reload, not mid-show.

use std::{collections::BTreeMap, f32::consts::PI, fmt, str::FromStr as _};

use glam::{DVec3, EulerRot, Quat, Vec3};
use serde::{
    Deserialize, Deserializer,
    de::{IgnoredAny, MapAccess, Visitor},
};
use serde_json::Value;
use steel_protocol::packets::game::SoundSource;
use steel_registry::{
    REGISTRY, RegistryExt as _, items::ItemRef, particle_type::ParticleTypeRef,
    sound_event::SoundEventHolder,
};
use steel_utils::{BlockStateId, Identifier};

use super::{
    camera_path::CameraPath,
    ease::Ease,
    json::{self, Object},
    time,
};

/// Actor types of the Fabric mod that have no Steel side yet.
const LATER_ACTORS: [&str; 5] = ["model", "train", "rig", "player", "mob"];
/// Cues of the Fabric mod that have no Steel side yet.
const LATER_CUES: [&str; 16] = [
    "blocks",
    "clear_blocks",
    "follow",
    "sparks",
    "lightning",
    "explosion",
    "meteors",
    "rift",
    "rockets",
    "shockwave",
    "animate",
    "pose",
    "swing",
    "ignite",
    "chest",
    "scatter",
];

/// Where a show plays. `yaw` is the way the admin faced at `/show stage`, snapped to a
/// quarter turn; only shows with `"stageFacing": true` turn with it.
#[derive(Clone, Debug, PartialEq)]
pub struct Stage {
    /// The key of the world, like `minecraft:overworld`.
    pub world: String,
    pub pos: DVec3,
    pub yaw: i32,
}

impl Stage {
    /// Reads `{"world": "...", "pos": [x, y, z], "yaw": 90}`.
    pub fn from_json(object: &Object) -> Result<Self, String> {
        Ok(Self {
            world: json::string_or(object, "world", "minecraft:overworld"),
            pos: json::vec_at(object, "pos")?,
            yaw: snap(json::number(object, "yaw", 0.0)?),
        })
    }

    /// The stage as it is kept in `stages.json`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut object = Object::new();
        object.insert("world".to_owned(), Value::from(self.world.clone()));
        object.insert("pos".to_owned(), json::array(self.pos));
        if self.yaw != 0 {
            object.insert("yaw".to_owned(), Value::from(self.yaw));
        }
        Value::Object(object)
    }
}

/// A Minecraft yaw snapped to 0, 90, 180 or 270.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "a number of quarter turns, reduced to 0..4 right away"
)]
pub fn snap(yaw: f64) -> i32 {
    ((yaw / 90.0).round().rem_euclid(4.0) as i32) * 90
}

/// What an actor is drawn as.
#[derive(Clone, Debug)]
pub enum ActorKind {
    /// Nothing to see: viewers look through it.
    Camera,
    Block(BlockStateId),
    Item {
        item: ItemRef,
        /// An item model of the resource pack.
        model: Option<Identifier>,
    },
    Text {
        text: String,
        line_width: i32,
        background: i32,
    },
}

/// A named thing on stage, from `actors`.
#[derive(Clone, Debug)]
pub struct ActorDef {
    pub name: String,
    pub kind: ActorKind,
    pub view_range: f32,
    pub bright: bool,
    pub scale: Vec3,
    /// The color of the outline seen through walls, RGB.
    pub glow: Option<i32>,
    /// Vanilla `Display.BillboardConstraints`, by id.
    pub billboard: Option<i8>,
}

impl ActorDef {
    /// Whether viewers look through this actor.
    #[must_use]
    pub const fn is_camera(&self) -> bool {
        matches!(self.kind, ActorKind::Camera)
    }
}

/// What a cue does. Places are relative to the stage; actors are indexes into the script's
/// list.
#[derive(Clone, Debug)]
pub enum Action {
    Cinematic {
        on: bool,
        actor: usize,
    },
    Cut {
        actor: usize,
        pos: DVec3,
        look: Option<DVec3>,
        yaw: f32,
        pitch: f32,
    },
    Path {
        actor: usize,
        ease: Ease,
        path: PathSource,
    },
    Shake {
        strength: f64,
    },
    Spawn {
        actor: usize,
        pos: DVec3,
        yaw: f32,
        scale: Option<Vec3>,
    },
    Move {
        actor: usize,
        ease: Ease,
        to: DVec3,
        yaw: Option<f32>,
    },
    Transform {
        actor: usize,
        ease: Ease,
        rotation: Option<Quat>,
        scale: Option<Vec3>,
    },
    Remove {
        actor: usize,
    },
    Time {
        ease: Ease,
        to: f64,
        backward: bool,
    },
    Weather {
        rain: f32,
        thunder: f32,
    },
    Sound {
        sound: SoundEventHolder,
        source: SoundSource,
        volume: f32,
        pitch: f32,
        pos: Option<DVec3>,
        actor: Option<usize>,
    },
    Particles {
        particle: ParticleTypeRef,
        pos: Option<DVec3>,
        actor: Option<usize>,
        count: i32,
        spread: Vec3,
        speed: f32,
    },
    Title {
        text: String,
        sub: String,
        fade_in: u32,
        stay: u32,
        fade_out: u32,
    },
    Bar {
        text: String,
    },
    Chat {
        text: String,
        link: Option<String>,
    },
}

/// One thing a show does at a moment, or over a stretch of time.
#[derive(Clone, Debug)]
pub struct Cue {
    /// When it starts, ticks from the start of the show.
    pub at: u32,
    /// How long it lasts, ticks; 0 happens at once.
    pub duration: u32,
    pub action: Action,
}

/// `teleport`: viewers stand in a ring around the stage; `none`: they stay where they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gather {
    Teleport,
    None,
}

/// A show, ready to play.
#[derive(Clone, Debug)]
pub struct ShowScript {
    pub id: String,
    pub title: String,
    pub stage: Option<Stage>,
    pub gather: Gather,
    pub gather_radius: f64,
    pub gather_center: DVec3,
    /// Puts viewers back where they stood before the show; off: they stay where it left them.
    pub return_after: bool,
    pub actors: Vec<ActorDef>,
    /// In the order they start.
    pub cues: Vec<Cue>,
    pub markers: BTreeMap<String, u32>,
    pub length: u32,
    /// What the countdown says instead of the title.
    pub countdown: Option<String>,
}

/// The names of the tracks in the order the file has them: cues of the same tick start in
/// that order, and a JSON object read into a map forgets it.
#[derive(Default)]
struct Keys(Vec<String>);

impl<'de> Deserialize<'de> for Keys {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KeysVisitor;

        impl<'de> Visitor<'de> for KeysVisitor {
            type Value = Keys;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Keys, A::Error> {
                let mut keys = Vec::new();
                while let Some((key, _)) = map.next_entry::<String, IgnoredAny>()? {
                    keys.push(key);
                }
                Ok(Keys(keys))
            }
        }

        deserializer.deserialize_map(KeysVisitor)
    }
}

#[derive(Deserialize)]
struct TrackOrder {
    #[serde(default)]
    tracks: Keys,
}

impl ShowScript {
    /// The recorded camera paths the cues fly, by name.
    pub fn recorded_paths(&self) -> impl Iterator<Item = &str> {
        self.cues.iter().filter_map(|cue| match &cue.action {
            Action::Path {
                path: PathSource::Recorded { name, .. },
                ..
            } => Some(name.as_str()),
            _ => None,
        })
    }

    /// Reads the text of a show file. `stage` is the place set with `/show stage`, which
    /// wins over the one in the file.
    pub fn parse(id: &str, text: &str, stage: Option<Stage>) -> Result<Self, String> {
        let value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
        let Value::Object(root) = value else {
            return Err("ожидаю объект".to_owned());
        };
        let order = serde_json::from_str::<TrackOrder>(text)
            .map_or_else(|_| Vec::new(), |order| order.tracks.0);
        Self::from_json(id, root, &order, stage)
    }

    fn from_json(
        id: &str,
        mut root: Object,
        order: &[String],
        stage: Option<Stage>,
    ) -> Result<Self, String> {
        let stage = match (stage, json::object(&root, "stage")?) {
            (Some(stage), _) => Some(stage),
            (None, Some(object)) => Some(Stage::from_json(object)?),
            (None, None) => None,
        };
        if json::boolean(&root, "stageFacing", false)?
            && let Some(stage) = &stage
            && stage.yaw != 0
        {
            rotate(&mut root, stage.yaw);
        }
        if json::get(&root, "logic").is_some_and(|logic| !is_empty(logic)) {
            return Err("ноды логики (logic) пока не перенесены в Steel".to_owned());
        }

        let title = json::string_or(&root, "title", id);
        let audience = Audience::read(&root)?;
        let actors = read_actors(&root)?;
        let (cues, end) = read_cues(&root, order, &actors)?;

        let mut markers = BTreeMap::new();
        if let Some(object) = json::object(&root, "markers")? {
            for (name, value) in object {
                markers.insert(name.clone(), time::from_json(value)?);
            }
        }
        let length = if json::has(&root, "length") {
            json::ticks(&root, "length", 0)?
        } else {
            end + 2 * time::TICKS_PER_SECOND
        };
        let countdown =
            json::has(&root, "countdown").then(|| json::string_or(&root, "countdown", &title));
        Ok(Self {
            id: id.to_owned(),
            title,
            stage,
            gather: audience.gather,
            gather_radius: audience.radius,
            gather_center: audience.center,
            return_after: audience.return_after,
            actors,
            cues,
            markers,
            length,
            countdown,
        })
    }

    /// What the countdown calls the show.
    #[must_use]
    pub fn countdown_name(&self) -> &str {
        self.countdown.as_deref().unwrap_or(&self.title)
    }
}

fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Object(object) => object.values().all(is_empty),
        Value::Array(items) => items.is_empty(),
        _ => false,
    }
}

struct Audience {
    gather: Gather,
    radius: f64,
    center: DVec3,
    return_after: bool,
}

impl Audience {
    fn read(root: &Object) -> Result<Self, String> {
        let mut audience = Self {
            gather: Gather::Teleport,
            radius: 24.0,
            center: DVec3::ZERO,
            return_after: false,
        };
        if let Some(object) = json::object(root, "audience")? {
            audience.gather = match json::string_or(object, "gather", "teleport").as_str() {
                "teleport" => Gather::Teleport,
                "none" => Gather::None,
                _ => return Err("audience.gather: teleport или none".to_owned()),
            };
            audience.radius = json::number(object, "radius", audience.radius)?;
            audience.center = json::vec_opt(object, "center")?.unwrap_or(DVec3::ZERO);
            audience.return_after = json::boolean(object, "returnAfter", false)?;
        }
        Ok(audience)
    }
}

fn read_actors(root: &Object) -> Result<Vec<ActorDef>, String> {
    let mut actors = Vec::new();
    if let Some(object) = json::object(root, "actors")? {
        for (name, value) in object {
            let Value::Object(actor) = value else {
                return Err(format!("актёр \"{name}\": ожидаю объект"));
            };
            actors.push(
                read_actor(name, actor).map_err(|error| format!("актёр \"{name}\": {error}"))?,
            );
        }
    }
    Ok(actors)
}

fn read_actor(name: &str, object: &Object) -> Result<ActorDef, String> {
    let kind = match json::string(object, "type")?.as_str() {
        "camera" => ActorKind::Camera,
        "block" => ActorKind::Block(block_state(&json::string(object, "block")?)?),
        "item" => {
            let id = json::string(object, "item")?;
            let item = identifier(&id)
                .ok()
                .and_then(|key| REGISTRY.items.by_key(&key))
                .ok_or_else(|| format!("нет предмета \"{id}\""))?;
            let model = json::get(object, "model")
                .map(|_| identifier(&json::string_or(object, "model", "")))
                .transpose()?;
            ActorKind::Item { item, model }
        }
        "text" => ActorKind::Text {
            text: json::string(object, "text")?,
            line_width: json::int(object, "lineWidth", 200)?,
            background: argb(&json::string_or(object, "background", "40000000"))?,
        },
        later if LATER_ACTORS.contains(&later) => {
            return Err(format!("тип \"{later}\" пока не перенесён в Steel"));
        }
        other => {
            return Err(format!(
                "нет типа \"{other}\" (есть camera, block, item, text)"
            ));
        }
    };
    let glow = json::get(object, "glow")
        .map(|_| {
            i32::from_str_radix(&json::string_or(object, "glow", ""), 16)
                .map_err(|_| "glow: цвет RGB в hex, например c040ff".to_owned())
        })
        .transpose()?;
    let billboard = json::get(object, "billboard")
        .map(
            |_| match json::string_or(object, "billboard", "").as_str() {
                "fixed" => Ok(0),
                "vertical" => Ok(1),
                "horizontal" => Ok(2),
                "center" => Ok(3),
                _ => Err("billboard: center, vertical, horizontal или fixed".to_owned()),
            },
        )
        .transpose()?;
    Ok(ActorDef {
        name: name.to_owned(),
        kind,
        view_range: json::float(object, "viewRange", 4.0)?,
        bright: json::boolean(object, "bright", false)?,
        scale: json::scale(object, "scale")?.map_or(Vec3::ONE, json::narrow),
        glow,
        billboard,
    })
}

fn identifier(text: &str) -> Result<Identifier, String> {
    Identifier::from_str(text).map_err(|_| format!("не понимаю имя \"{text}\""))
}

/// `40000000` as ARGB: the bits of an unsigned hex number.
fn argb(text: &str) -> Result<i32, String> {
    u32::from_str_radix(text, 16)
        .map(u32::cast_signed)
        .map_err(|_| "background: цвет ARGB в hex, например 40000000".to_owned())
}

/// `minecraft:oak_log[axis=x]` as a block state; properties left out take their defaults.
pub fn block_state(text: &str) -> Result<BlockStateId, String> {
    let wrong = || format!("не понимаю блок \"{text}\"");
    let (name, properties) = match text.split_once('[') {
        Some((name, rest)) => (name, rest.strip_suffix(']').ok_or_else(wrong)?),
        None => (text, ""),
    };
    let properties: Vec<(&str, &str)> = properties
        .split(',')
        .filter(|property| !property.trim().is_empty())
        .map(|property| {
            property
                .split_once('=')
                .map(|(key, value)| (key.trim(), value.trim()))
                .ok_or_else(wrong)
        })
        .collect::<Result<_, _>>()?;
    let key = Identifier::from_str(name.trim()).map_err(|_| wrong())?;
    let block = REGISTRY.blocks.by_key(&key).ok_or_else(wrong)?;
    REGISTRY
        .blocks
        .state_id_from_block_defaulted_properties(block, properties)
        .ok_or_else(wrong)
}

/// Reads `tracks` (in the order of the file) and `cues`. Returns the cues sorted by their
/// start and the tick the last of them ends on.
fn read_cues(
    root: &Object,
    order: &[String],
    actors: &[ActorDef],
) -> Result<(Vec<Cue>, u32), String> {
    let mut cues = Vec::new();
    if let Some(tracks) = json::object(root, "tracks")? {
        let named = order.iter().filter(|name| tracks.contains_key(*name));
        let rest = tracks.keys().filter(|name| !order.contains(name));
        for name in named.chain(rest) {
            if let Some(track) = tracks.get(name) {
                read_track(track, &format!("tracks.{name}"), actors, &mut cues)?;
            }
        }
    }
    if let Some(list) = json::get(root, "cues") {
        read_track(list, "cues", actors, &mut cues)?;
    }
    // Stable: cues of the same tick keep the order they were written in.
    cues.sort_by_key(|cue| cue.at);
    let end = cues
        .iter()
        .map(|cue| cue.at + cue.duration)
        .max()
        .unwrap_or(0);
    Ok((cues, end))
}

fn read_track(
    track: &Value,
    place: &str,
    actors: &[ActorDef],
    cues: &mut Vec<Cue>,
) -> Result<(), String> {
    let Value::Array(items) = track else {
        return Err(format!("{place}: ожидаю список"));
    };
    for (index, item) in items.iter().enumerate() {
        let place = format!("{place}[{index}]");
        let Value::Object(object) = item else {
            return Err(format!("{place}: ожидаю объект"));
        };
        let at = json::ticks(object, "at", 0).map_err(|error| format!("{place}: {error}"))?;
        let kind = json::string(object, "do").map_err(|error| format!("{place}: {error}"))?;
        let (duration, action) = read_action(&kind, object, actors)
            .map_err(|error| format!("{place} (\"{kind}\" на {}): {error}", time::format(at)))?;
        cues.push(Cue {
            at,
            duration,
            action,
        });
    }
    Ok(())
}

fn actor_index(actors: &[ActorDef], name: &str) -> Result<usize, String> {
    actors
        .iter()
        .position(|actor| actor.name == name)
        .ok_or_else(|| format!("нет актёра \"{name}\" в actors"))
}

fn actor(actors: &[ActorDef], object: &Object) -> Result<usize, String> {
    actor_index(actors, &json::string(object, "actor")?)
}

fn actor_opt(actors: &[ActorDef], object: &Object) -> Result<Option<usize>, String> {
    json::has(object, "actor")
        .then(|| actor(actors, object))
        .transpose()
}

fn camera(actors: &[ActorDef], object: &Object) -> Result<usize, String> {
    let name = json::string_or(object, "actor", "cam");
    let index = actor_index(actors, &name)?;
    if actors[index].is_camera() {
        Ok(index)
    } else {
        Err(format!("актёр \"{name}\" не камера (type: camera)"))
    }
}

const fn seconds(seconds: u32) -> u32 {
    seconds * time::TICKS_PER_SECOND
}

/// The action of a cue and how long it lasts.
fn read_action(kind: &str, object: &Object, actors: &[ActorDef]) -> Result<(u32, Action), String> {
    let over = |fallback: u32| json::ticks(object, "over", fallback);
    Ok(match kind {
        "cinematic" => (
            0,
            Action::Cinematic {
                on: json::boolean(object, "on", true)?,
                actor: camera(actors, object)?,
            },
        ),
        "cut" => (
            0,
            Action::Cut {
                actor: camera(actors, object)?,
                pos: json::vec_at(object, "pos")?,
                look: json::vec_opt(object, "look")?,
                yaw: json::float(object, "yaw", 0.0)?,
                pitch: json::float(object, "pitch", 0.0)?,
            },
        ),
        "path" => (
            over(seconds(5))?,
            Action::Path {
                actor: camera(actors, object)?,
                ease: json::ease(object, Ease::InOutSine)?,
                path: camera_path(object)?,
            },
        ),
        "shake" => (
            over(seconds(1))?,
            Action::Shake {
                strength: json::number(object, "strength", 0.5)?,
            },
        ),
        "spawn" | "move" | "transform" | "remove" => read_actor_action(kind, object, actors)?,
        "time" => (
            over(0)?,
            Action::Time {
                ease: json::ease(object, Ease::InOutSine)?,
                to: json::number(object, "to", 18000.0)?,
                backward: json::boolean(object, "backward", false)?,
            },
        ),
        "weather" => {
            let (rain, thunder) = match json::string(object, "to")?.as_str() {
                "clear" => (0.0, 0.0),
                "rain" => (1.0, 0.0),
                "thunder" => (1.0, 1.0),
                _ => return Err("погода: clear, rain или thunder".to_owned()),
            };
            (over(0)?, Action::Weather { rain, thunder })
        }
        "sound" | "particles" => read_effect(kind, object, actors)?,
        "title" => (
            0,
            Action::Title {
                text: json::string_or(object, "text", ""),
                sub: json::string_or(object, "sub", ""),
                fade_in: json::ticks(object, "fadeIn", 10)?,
                stay: json::ticks(object, "stay", seconds(3))?,
                fade_out: json::ticks(object, "fadeOut", 20)?,
            },
        ),
        "actionbar" => (
            0,
            Action::Bar {
                text: json::string(object, "text")?,
            },
        ),
        "chat" => {
            let link = json::string_or(object, "link", "");
            if !link.is_empty() && !link.starts_with("https://") {
                return Err("link: адрес должен начинаться с https://".to_owned());
            }
            (
                0,
                Action::Chat {
                    text: json::string(object, "text")?,
                    link: (!link.is_empty()).then_some(link),
                },
            )
        }
        later if LATER_CUES.contains(&later) => {
            return Err("это действие пока не перенесено в Steel".to_owned());
        }
        other => return Err(format!("нет такого действия \"{other}\"")),
    })
}

fn read_actor_action(
    kind: &str,
    object: &Object,
    actors: &[ActorDef],
) -> Result<(u32, Action), String> {
    let actor = actor(actors, object)?;
    let over = json::ticks(object, "over", 0)?;
    let scale = json::scale(object, "scale")?.map(json::narrow);
    Ok(match kind {
        "spawn" => (
            0,
            Action::Spawn {
                actor,
                pos: json::vec_opt(object, "pos")?.unwrap_or(DVec3::ZERO),
                yaw: json::float(object, "yaw", 0.0)?,
                scale,
            },
        ),
        "move" => (
            over,
            Action::Move {
                actor,
                ease: json::ease(object, Ease::InOutSine)?,
                to: json::vec_at(object, "to")?,
                yaw: json::has(object, "yaw")
                    .then(|| json::float(object, "yaw", 0.0))
                    .transpose()?,
            },
        ),
        "transform" => (
            over,
            Action::Transform {
                actor,
                ease: json::ease(object, Ease::Linear)?,
                // Degrees around X, Y and Z, turned in the order of vanilla `rotateYXZ`.
                rotation: json::vec_opt(object, "rotate")?.map(|degrees| {
                    let radians = json::narrow(degrees) * (PI / 180.0);
                    Quat::from_euler(EulerRot::YXZ, radians.y, radians.x, radians.z)
                }),
                scale,
            },
        ),
        _ => (0, Action::Remove { actor }),
    })
}

fn read_effect(kind: &str, object: &Object, actors: &[ActorDef]) -> Result<(u32, Action), String> {
    let pos = json::vec_opt(object, "pos")?;
    let actor = actor_opt(actors, object)?;
    if kind == "sound" {
        let id = identifier(&json::string(object, "id")?)?;
        // Sounds of a resource pack aren't in the registry; the client finds them by name.
        let sound = REGISTRY.sound_events.by_key(&id).map_or_else(
            || SoundEventHolder::Direct {
                sound_id: id.clone(),
                fixed_range: None,
            },
            SoundEventHolder::Registry,
        );
        let name = json::string_or(object, "source", "master");
        let source = SoundSource::VALUES
            .into_iter()
            .find(|source| source.name() == name)
            .ok_or_else(|| {
                format!("нет громкости \"{name}\" (master, music, ambient, weather, ...)")
            })?;
        return Ok((
            0,
            Action::Sound {
                sound,
                source,
                volume: json::float(object, "volume", 1.0)?,
                pitch: json::float(object, "pitch", 1.0)?,
                pos,
                actor,
            },
        ));
    }
    let id = json::string(object, "id")?;
    let particle = identifier(&id)
        .ok()
        .and_then(|key| REGISTRY.particle_types.by_key(&key))
        .filter(|particle| particle.is_simple())
        .ok_or_else(|| format!("частицы \"{id}\" не найдены или требуют параметров"))?;
    Ok((
        json::ticks(object, "over", 0)?,
        Action::Particles {
            particle,
            pos,
            actor,
            count: json::int(object, "count", 20)?,
            spread: json::scale(object, "spread")?.map_or(Vec3::ONE, json::narrow),
            speed: json::float(object, "speed", 0.05)?,
        },
    ))
}

/// The flight of a `path` cue: an orbit, points written in the cue, or a recorded path.
fn camera_path(object: &Object) -> Result<PathSource, String> {
    let look = json::vec_opt(object, "look")?;
    let path = if let Some(orbit) = json::object(object, "orbit")? {
        CameraPath::orbit(
            json::vec_opt(orbit, "center")?.unwrap_or(DVec3::ZERO),
            json::number(orbit, "radius", 20.0)?,
            json::number(orbit, "height", 10.0)?,
            json::number(orbit, "from", 0.0)?,
            json::number(orbit, "to", 360.0)?,
        )?
    } else if json::has(object, "points") {
        CameraPath::from_json(object)?
    } else {
        // A recorded path may not exist yet while the show is being made: it is read when the
        // show starts.
        let name = json::string(object, "path")?;
        check_name(&name)?;
        return Ok(PathSource::Recorded { name, look });
    };
    Ok(PathSource::Inline(match look {
        Some(target) => path.looking_at(target),
        None => path,
    }))
}

/// Where a `path` cue gets its flight.
#[derive(Clone, Debug, PartialEq)]
pub enum PathSource {
    /// Written in the cue: points or an orbit.
    Inline(CameraPath),
    /// Recorded with `/show cam` and kept beside the show; with `look` the camera keeps
    /// looking at that place instead of where the admin looked.
    Recorded { name: String, look: Option<DVec3> },
}

/// The name of something kept in a file of its own (a camera path): safe as a file name.
pub fn check_name(name: &str) -> Result<(), String> {
    let fine = !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        });
    if fine {
        Ok(())
    } else {
        Err(format!("имя \"{name}\": только a-z, 0-9, _ и -"))
    }
}

/// Fields whose three numbers are a size or angles, not a place: they don't turn with the
/// stage.
const NOT_PLACES: [&str; 4] = ["scale", "rotate", "size", "pivot"];

/// `"stageFacing": true`: the script is written for an admin facing south (+Z ahead, +X to
/// the left, yaw 0) and turns with the way they faced at `/show stage`. Every place in the
/// cues and the audience turns around the stage, every `yaw` and orbit angle gets the
/// stage's yaw added.
fn rotate(root: &mut Object, yaw: i32) {
    for key in ["tracks", "cues", "audience"] {
        if let Some(value) = root.get_mut(key) {
            rotate_value(value, None, yaw);
        }
    }
}

fn rotate_value(value: &mut Value, key: Option<&str>, yaw: i32) {
    match value {
        Value::Object(object) => {
            for (name, inner) in object.iter_mut() {
                let angle =
                    name == "yaw" || (key == Some("orbit") && (name == "from" || name == "to"));
                if angle && let Some(number) = inner.as_f64().filter(|_| inner.is_number()) {
                    *inner = Value::from(number + f64::from(yaw));
                } else if !NOT_PLACES.contains(&name.as_str()) {
                    rotate_value(inner, Some(name), yaw);
                }
            }
        }
        Value::Array(items) => {
            if let [x, _, z] = items.as_mut_slice()
                && let (Some(old_x), Some(old_z)) = (number(x), number(z))
            {
                let radians = f64::from(yaw).to_radians();
                let (sin, cos) = (radians.sin().round(), radians.cos().round());
                *x = Value::from(old_x * cos - old_z * sin);
                *z = Value::from(old_x * sin + old_z * cos);
                return;
            }
            for item in items {
                rotate_value(item, key, yaw);
            }
        }
        _ => {}
    }
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64().filter(|_| value.is_number())
}
