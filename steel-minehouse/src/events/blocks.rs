//! Blocks for a show (the Fabric `BlockSet`): captured from the world with `/show capture`
//! or built from a shape (`sphere`, `box`, `disc`, `bowl`). Offsets are counted from an
//! origin; a captured set remembers where its origin sat from the stage (`anchor`).

use std::str::FromStr as _;

use glam::{DVec3, IVec3};
use serde_json::Value;
use steel_core::worldgen::rotate_block_state;
use steel_registry::{
    REGISTRY, RegistryExt as _, blocks::block_state_ext::BlockStateExt as _, vanilla_blocks,
};
use steel_utils::{
    BlockPos, BlockStateId, Identifier,
    random::{Random as _, legacy_random::LegacyRandom},
    rotation::Rotation,
};

use super::json::{self, Object};

/// Blocks at their places in the world, in the order they are put there.
pub type Placed = Vec<(BlockPos, BlockStateId)>;

/// `minecraft:oak_stairs[facing=north,half=bottom]`: a state as it is written in files.
#[must_use]
pub fn state_text(state: BlockStateId) -> String {
    let name = state.get_block().key.to_string();
    let properties: Vec<String> = REGISTRY
        .blocks
        .properties(state)
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    if properties.is_empty() {
        name
    } else {
        format!("{name}[{}]", properties.join(","))
    }
}

/// A block state written as `minecraft:oak_stairs[facing=north]`; properties left out take
/// their defaults.
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

/// A quarter turn for a stage yaw of 0, 90, 180 or 270.
#[must_use]
pub const fn rotation(yaw: i32) -> Rotation {
    match yaw.rem_euclid(360) {
        90 => Rotation::Clockwise90,
        180 => Rotation::Clockwise180,
        270 => Rotation::CounterClockwise90,
        _ => Rotation::None,
    }
}

/// Vanilla `BlockPos.rotate`.
const fn turn(offset: IVec3, rotation: Rotation) -> IVec3 {
    match rotation {
        Rotation::None => offset,
        Rotation::Clockwise90 => IVec3::new(-offset.z, offset.y, offset.x),
        Rotation::Clockwise180 => IVec3::new(-offset.x, offset.y, -offset.z),
        Rotation::CounterClockwise90 => IVec3::new(offset.z, offset.y, -offset.x),
    }
}

/// A set of blocks around an origin.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockSet {
    entries: Vec<(IVec3, BlockStateId)>,
    anchor: DVec3,
}

impl BlockSet {
    /// The blocks, as offsets from the origin.
    #[must_use]
    pub fn entries(&self) -> &[(IVec3, BlockStateId)] {
        &self.entries
    }

    /// Where the origin sat from the stage when the set was captured.
    #[must_use]
    pub const fn anchor(&self) -> DVec3 {
        self.anchor
    }

    pub fn from_json(object: &Object) -> Result<Self, String> {
        let Some(Value::Array(items)) = json::get(object, "blocks") else {
            return Err("нет поля \"blocks\"".to_owned());
        };
        let mut entries = Vec::with_capacity(items.len());
        for item in items {
            let Value::Object(entry) = item else {
                return Err("blocks: ожидаю объекты {p, s}".to_owned());
            };
            let at = json::vec_at(entry, "p")?;
            entries.push((
                at.round().as_ivec3(),
                block_state(&json::string(entry, "s")?)?,
            ));
        }
        Ok(Self {
            entries,
            anchor: json::vec_opt(object, "anchor")?.unwrap_or(DVec3::ZERO),
        })
    }

    #[must_use]
    pub fn to_json(&self) -> Value {
        let blocks: Vec<Value> = self
            .entries
            .iter()
            .map(|(offset, state)| {
                let mut entry = Object::new();
                entry.insert(
                    "p".to_owned(),
                    Value::from(vec![offset.x, offset.y, offset.z]),
                );
                entry.insert("s".to_owned(), Value::from(state_text(*state)));
                Value::Object(entry)
            })
            .collect();
        let mut object = Object::new();
        object.insert("anchor".to_owned(), json::array(self.anchor));
        object.insert("blocks".to_owned(), Value::from(blocks));
        Value::Object(object)
    }

    /// Copies the blocks between two corners; the origin is the bottom middle of the box.
    pub fn capture(
        block_at: impl Fn(BlockPos) -> BlockStateId,
        a: BlockPos,
        b: BlockPos,
        stage: DVec3,
        with_air: bool,
    ) -> Self {
        let min = a.0.min(b.0);
        let max = a.0.max(b.0);
        let origin = IVec3::new(
            (min.x + max.x).div_euclid(2),
            min.y,
            (min.z + max.z).div_euclid(2),
        );
        let mut entries = Vec::new();
        for z in min.z..=max.z {
            for y in min.y..=max.y {
                for x in min.x..=max.x {
                    let at = IVec3::new(x, y, z);
                    let block = block_at(BlockPos(at));
                    if with_air || !block.is_air() {
                        entries.push((at - origin, block));
                    }
                }
            }
        }
        Self {
            entries,
            anchor: origin.as_dvec3() - stage,
        }
    }

    /// The blocks in the world when the origin sits at `origin`, the set turned by
    /// `rotation` around it the way `stageFacing` turns a script's places; the blocks turn
    /// with it.
    #[must_use]
    pub fn at(&self, origin: BlockPos, rotation: Rotation) -> Placed {
        self.entries
            .iter()
            .map(|(offset, state)| {
                (
                    BlockPos(origin.0 + turn(*offset, rotation)),
                    rotate_block_state(*state, rotation),
                )
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
enum ShapeKind {
    Sphere,
    Box {
        size: IVec3,
    },
    /// Replaces the top block of the terrain in a circle.
    Disc,
    /// A crater dug into the terrain: air down to a curved floor lined with the palette.
    Bowl {
        depth: f64,
    },
}

/// A shape made of blocks picked from a palette. The picks are random but always the same
/// for the same shape (and the same as on Fabric), so a rehearsal and the show look alike.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    kind: ShapeKind,
    radius: f64,
    palette: Vec<BlockStateId>,
    seed: i64,
}

impl Shape {
    pub fn parse(object: &Object) -> Result<Self, String> {
        let radius = json::number(object, "radius", 5.0)?;
        let kind = match json::string(object, "type")?.as_str() {
            "sphere" => ShapeKind::Sphere,
            "box" => ShapeKind::Box {
                size: json::vec_at(object, "size")?.as_ivec3(),
            },
            "disc" => ShapeKind::Disc,
            "bowl" => ShapeKind::Bowl {
                depth: json::number(object, "depth", radius / 2.0)?,
            },
            other => {
                return Err(format!(
                    "нет такой формы \"{other}\" (есть sphere, box, disc, bowl)"
                ));
            }
        };
        let mut palette = Vec::new();
        match json::get(object, "palette") {
            None => {}
            Some(Value::Array(names)) => {
                for name in names {
                    let Value::String(name) = name else {
                        return Err("palette: ожидаю названия блоков".to_owned());
                    };
                    palette.push(block_state(name)?);
                }
            }
            Some(Value::String(name)) => palette.push(block_state(name)?),
            Some(_) => return Err("palette: ожидаю список блоков".to_owned()),
        }
        if palette.is_empty() {
            palette.push(vanilla_blocks::STONE.default_state());
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a seed written as a whole number"
        )]
        let seed = json::number(object, "seed", 42.0)? as i64;
        Ok(Self {
            kind,
            radius,
            palette,
            seed,
        })
    }

    /// The blocks of the shape around `center`. `surface` is the y of the highest solid
    /// block of a column, leaves aside.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "block distances of a shape a show draws"
    )]
    pub fn blocks(&self, center: BlockPos, surface: impl Fn(i32, i32) -> i32) -> Placed {
        #[expect(clippy::cast_sign_loss, reason = "the seed's bits, as Java takes them")]
        let mut random = LegacyRandom::from_seed(self.seed as u64);
        let count = i32::try_from(self.palette.len()).unwrap_or(i32::MAX);
        let mut pick = || {
            let index = random.next_i32_bounded(count);
            self.palette[usize::try_from(index).unwrap_or(0) % self.palette.len()]
        };
        let center = center.0;
        let radius = self.radius;
        let reach = radius.ceil() as i32;
        let mut out = Placed::new();
        match &self.kind {
            ShapeKind::Sphere => {
                for z in -reach..=reach {
                    for y in -reach..=reach {
                        for x in -reach..=reach {
                            if f64::from(x * x + y * y + z * z) <= radius * radius {
                                out.push((BlockPos(center + IVec3::new(x, y, z)), pick()));
                            }
                        }
                    }
                }
            }
            ShapeKind::Box { size } => {
                let end = (*size - IVec3::ONE).max(IVec3::ZERO);
                for z in 0..=end.z {
                    for y in 0..=end.y {
                        for x in 0..=end.x {
                            out.push((BlockPos(center + IVec3::new(x, y, z)), pick()));
                        }
                    }
                }
            }
            ShapeKind::Disc => {
                for dx in -reach..=reach {
                    for dz in -reach..=reach {
                        if f64::from(dx * dx + dz * dz) <= radius * radius {
                            let (x, z) = (center.x + dx, center.z + dz);
                            out.push((BlockPos::new(x, surface(x, z), z), pick()));
                        }
                    }
                }
            }
            ShapeKind::Bowl { depth } => {
                let air = vanilla_blocks::AIR.default_state();
                for dx in -reach..=reach {
                    for dz in -reach..=reach {
                        let part = f64::from(dx * dx + dz * dz) / (radius * radius);
                        if part > 1.0 {
                            continue;
                        }
                        let (x, z) = (center.x + dx, center.z + dz);
                        let top = surface(x, z);
                        // Java's `Math.round`: halves go up.
                        let dig = (depth * (1.0 - part) + 0.5).floor() as i32;
                        for y in ((top - dig + 1)..=top).rev() {
                            out.push((BlockPos::new(x, y, z), air));
                        }
                        out.push((BlockPos::new(x, top - dig, z), pick()));
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::json;
    use steel_registry::init_vanilla_registry;

    use super::*;

    fn object(value: Value) -> Object {
        match value {
            Value::Object(object) => object,
            _ => Object::new(),
        }
    }

    #[test]
    fn states_are_written_and_read_back() {
        init_vanilla_registry();
        assert_eq!(
            state_text(vanilla_blocks::STONE.default_state()),
            "minecraft:stone"
        );
        let stairs = block_state("minecraft:oak_stairs[facing=east,half=top]").expect("stairs");
        let text = state_text(stairs);
        assert!(text.starts_with("minecraft:oak_stairs["), "{text}");
        assert_eq!(block_state(&text), Ok(stairs));
        assert!(block_state("minecraft:no_such_block").is_err());
    }

    #[test]
    fn a_captured_set_sits_on_the_bottom_middle() {
        init_vanilla_registry();
        let stone = vanilla_blocks::STONE.default_state();
        let air = vanilla_blocks::AIR.default_state();
        // A floor of stone at y 64, air above.
        let world = |pos: BlockPos| if pos.0.y == 64 { stone } else { air };
        let stage = DVec3::new(0.5, 64.0, 0.5);
        let set = BlockSet::capture(
            world,
            BlockPos::new(12, 65, 9),
            BlockPos::new(10, 64, 7),
            stage,
            false,
        );
        assert_eq!(set.entries().len(), 9);
        assert!(set.entries().iter().all(|(offset, _)| offset.y == 0));
        assert_eq!(set.anchor(), DVec3::new(10.5, 0.0, 7.5));
        let with_air = BlockSet::capture(
            world,
            BlockPos::new(10, 64, 7),
            BlockPos::new(12, 65, 9),
            stage,
            true,
        );
        assert_eq!(with_air.entries().len(), 18);

        let read = BlockSet::from_json(&object(set.to_json())).expect("read back");
        assert_eq!(read, set);
    }

    #[test]
    fn a_set_turns_with_the_stage() {
        init_vanilla_registry();
        let stairs = block_state("minecraft:oak_stairs[facing=north]").expect("stairs");
        let set = BlockSet {
            entries: vec![(IVec3::new(2, 0, 1), stairs)],
            anchor: DVec3::ZERO,
        };
        let origin = BlockPos::new(100, 64, 100);
        assert_eq!(
            set.at(origin, Rotation::None),
            [(BlockPos::new(102, 64, 101), stairs)]
        );
        let turned = set.at(origin, rotation(90));
        assert_eq!(turned[0].0, BlockPos::new(99, 64, 102));
        assert_eq!(
            turned[0].1,
            block_state("minecraft:oak_stairs[facing=east]").expect("stairs")
        );
        assert_eq!(rotation(-90), Rotation::CounterClockwise90);
    }

    #[test]
    fn shapes_are_the_same_every_time() {
        init_vanilla_registry();
        let shape = Shape::parse(&object(json!({
            "type": "sphere", "radius": 2,
            "palette": ["minecraft:stone", "minecraft:magma_block", "minecraft:obsidian"]
        })))
        .expect("a sphere");
        let center = BlockPos::new(0, 70, 0);
        let first = shape.blocks(center, |_, _| 64);
        // 33 blocks are within 2 of the middle.
        assert_eq!(first.len(), 33);
        assert_eq!(first, shape.blocks(center, |_, _| 64));
        let kinds: BTreeSet<u16> = first.iter().map(|(_, state)| state.0).collect();
        assert_eq!(kinds.len(), 3);

        // java.util.Random(42).nextInt(3) starts 2, 0, 0, 2.
        let palette = &shape.palette;
        let picks: Vec<BlockStateId> = first.iter().take(4).map(|(_, state)| *state).collect();
        assert_eq!(picks, [palette[2], palette[0], palette[0], palette[2]]);
    }

    #[test]
    fn a_bowl_is_dug_from_the_surface() {
        init_vanilla_registry();
        let shape = Shape::parse(&object(json!({"type": "bowl", "radius": 2, "depth": 2})))
            .expect("a bowl");
        let blocks = shape.blocks(BlockPos::new(0, 0, 0), |_, _| 64);
        let air = vanilla_blocks::AIR.default_state();
        let stone = vanilla_blocks::STONE.default_state();
        // The middle: two blocks of air, the floor two below the surface.
        assert!(blocks.contains(&(BlockPos::new(0, 64, 0), air)));
        assert!(blocks.contains(&(BlockPos::new(0, 63, 0), air)));
        assert!(blocks.contains(&(BlockPos::new(0, 62, 0), stone)));
        // The rim is only relined.
        assert!(blocks.contains(&(BlockPos::new(2, 64, 0), stone)));
        assert!(Shape::parse(&object(json!({"type": "cone"}))).is_err());
        assert!(Shape::parse(&object(json!({"type": "box"}))).is_err());
    }
}
