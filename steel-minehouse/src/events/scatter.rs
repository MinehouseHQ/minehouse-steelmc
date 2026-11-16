//! The `scatter` cue (the Fabric `Puppets.Scatter`): items fly apart from a point, fall,
//! bounce and lie flat on a floor. They are item displays the show moves itself, so a ghost
//! floor holds them and nobody can pick them up; they stay until the show ends, or until
//! `vanish`, when they are gone at once. Blocks rest upright on the floor.

use std::{f32::consts::FRAC_PI_2, f64::consts::TAU};

use glam::{DVec3, Quat, Vec3};
use steel_registry::{
    REGISTRY, RegistryExt as _, data_components::vanilla_components::ITEM_MODEL,
    item_stack::ItemStack, items::ItemRef,
};
use steel_utils::{
    Identifier,
    random::{Random as _, legacy_random::LegacyRandom},
};
use uuid::Uuid;

use super::{
    effects::Fx,
    json::{self, Object},
    prop::Prop,
    script::{identifier, item},
    time,
};

/// How much speed a flying item loses downwards every tick.
const GRAVITY: f64 = 0.04;
/// Who sees the items, from where they start.
const SEEN_FROM: f64 = 128.0;
/// How far the viewers' game draws an item, in its own units.
const VIEW_RANGE: f32 = 2.0;
/// More items than this are not worth the packets.
const MOST: i32 = 256;

/// A `scatter` cue as the script has it.
#[derive(Clone, Debug)]
pub struct ScatterDef {
    pub item: ItemRef,
    /// An item model of the resource pack.
    pub model: Option<Identifier>,
    pub pos: DVec3,
    pub count: u32,
    pub speed: f64,
    pub up: f64,
    /// The height the items come to rest at, from the stage.
    pub floor: f64,
    pub size: f32,
    pub seed: i64,
    /// Walls: with a radius, nothing flies further than that from `center` (it bounces
    /// back).
    pub center: Option<DVec3>,
    pub radius: f64,
    /// How long the items fall before whatever is still in the air lands, ticks.
    pub fall: u32,
    /// When the items are gone, ticks after the cue starts; 0 keeps them until the show
    /// ends.
    pub vanish: u32,
    /// Whether the item is a block: those stand on the floor, flat items lie on it.
    pub block: bool,
}

impl ScatterDef {
    /// Reads the cue; also how long it runs.
    pub fn parse(object: &Object) -> Result<(u32, Self), String> {
        let item = item(&json::string(object, "item")?)?;
        let pos = json::vec_at(object, "pos")?;
        let over = json::ticks(object, "over", 3 * time::TICKS_PER_SECOND)?;
        let vanish = json::ticks(object, "vanish", 0)?;
        let model = json::has(object, "model")
            .then(|| identifier(&json::string_or(object, "model", "")))
            .transpose()?;
        let def = Self {
            item,
            model,
            pos,
            count: json::int(object, "count", 12)?
                .clamp(0, MOST)
                .unsigned_abs(),
            speed: json::number(object, "speed", 0.25)?,
            up: json::number(object, "up", 0.35)?,
            floor: json::number(object, "floor", pos.y)?,
            size: json::float(object, "size", 0.4)?,
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a seed: any whole number will do"
            )]
            seed: json::number(object, "seed", 7.0)? as i64,
            center: json::vec_opt(object, "center")?,
            radius: json::number(object, "radius", 0.0)?,
            fall: over.max(1),
            vanish,
            block: REGISTRY.blocks.by_key(&item.key).is_some(),
        };
        Ok((over.max(vanish).max(1), def))
    }
}

struct Flying {
    prop: Prop,
    velocity: DVec3,
    axis: Vec3,
    angle: f32,
    spin: f32,
    rest_yaw: f32,
    resting: bool,
}

/// The items of one `scatter` cue.
#[derive(Default)]
pub struct Scatter {
    flying: Vec<Flying>,
    /// The middle of the walls and the height of the floor, in the world.
    center: DVec3,
    floor: f64,
    landed: bool,
    gone: bool,
}

impl Scatter {
    /// Throws the items from `from`; `origin` is the stage.
    #[must_use]
    pub fn throw(def: &ScatterDef, origin: DVec3) -> Self {
        let from = origin + def.pos;
        let mut random = LegacyRandom::from_seed(def.seed.cast_unsigned());
        let mut stack = ItemStack::new(def.item);
        if let Some(model) = &def.model {
            stack.set(ITEM_MODEL, model.clone());
        }
        let mut scatter = Self {
            flying: Vec::new(),
            center: origin + def.center.unwrap_or(def.pos),
            floor: origin.y + def.floor,
            landed: false,
            gone: false,
        };
        for _ in 0..def.count {
            // The order of the draws is the Fabric mod's: the same seed, the same throw.
            let direction = random.next_f64() * TAU;
            let speed = def.speed * 0.65f64.mul_add(random.next_f64(), 0.35);
            let velocity = DVec3::new(
                direction.cos() * speed,
                def.up * 0.7f64.mul_add(random.next_f64(), 0.6),
                direction.sin() * speed,
            );
            let axis = Vec3::new(
                random.next_f32() - 0.5,
                random.next_f32() - 0.5,
                random.next_f32() - 0.5,
            )
            .normalize_or(Vec3::Y);
            #[expect(clippy::cast_possible_truncation, reason = "angles")]
            let (spin, rest_yaw) = (
                0.5f64.mul_add(random.next_f64(), 0.3) as f32,
                (random.next_f64() * TAU) as f32,
            );
            let mut flying = Flying {
                prop: Prop::item(stack.clone(), from, VIEW_RANGE),
                velocity,
                axis,
                angle: 0.0,
                spin,
                rest_yaw,
                resting: false,
            };
            flying.shape(def, 0);
            scatter.flying.push(flying);
        }
        scatter
    }

    /// One tick, `elapsed` after the cue started.
    pub fn update(&mut self, def: &ScatterDef, elapsed: u32, fx: &Fx<'_>) {
        if elapsed == 0 || self.gone {
            self.sync(fx);
            return;
        }
        if def.vanish > 0 && elapsed >= def.vanish {
            self.gone = true;
            self.clear(fx);
            return;
        }
        if elapsed >= def.fall {
            self.land(def);
        } else {
            for flying in &mut self.flying {
                if !flying.resting {
                    flying.step(def, self.center, self.floor);
                }
            }
        }
        self.sync(fx);
    }

    /// Whatever is still in the air when the fall runs out lands where it is.
    pub fn land(&mut self, def: &ScatterDef) {
        if self.landed || self.gone {
            return;
        }
        self.landed = true;
        for flying in &mut self.flying {
            if !flying.resting {
                flying.resting = true;
                let at = flying.prop.pos();
                flying.prop.glide(DVec3::new(at.x, self.floor, at.z), 1);
                flying.shape(def, 2);
            }
        }
    }

    fn sync(&mut self, fx: &Fx<'_>) {
        if fx.muted {
            return;
        }
        let here = fx.near(fx.world, self.center, SEEN_FROM);
        for flying in &mut self.flying {
            flying.prop.sync(&here);
        }
    }

    /// The items, to lie where they are until the show ends.
    pub fn leave(&mut self) -> Vec<Prop> {
        self.flying.drain(..).map(|flying| flying.prop).collect()
    }

    /// Takes the items away.
    pub fn clear(&mut self, fx: &Fx<'_>) {
        for mut flying in self.flying.drain(..) {
            flying.prop.remove(fx.online);
        }
    }

    /// A viewer's game was started again and has none of the items.
    pub fn forget(&mut self, id: Uuid) {
        for flying in &mut self.flying {
            flying.prop.forget(id);
        }
    }
}

impl Flying {
    fn step(&mut self, def: &ScatterDef, center: DVec3, floor: f64) {
        self.velocity = DVec3::new(
            self.velocity.x * 0.98,
            self.velocity.y - GRAVITY,
            self.velocity.z * 0.98,
        );
        let mut next = self.prop.pos() + self.velocity;
        if next.y <= floor {
            next.y = floor;
            if self.velocity.y.abs() < 0.12 {
                self.resting = true;
            } else {
                self.velocity = DVec3::new(
                    self.velocity.x * 0.55,
                    -self.velocity.y * 0.35,
                    self.velocity.z * 0.55,
                );
            }
        }
        if def.radius > 0.0 {
            let (dx, dz) = (next.x - center.x, next.z - center.z);
            let distance = dx.hypot(dz);
            if distance > def.radius {
                // Off the wall: back inside, the outward part of the speed turned round.
                let (nx, nz) = (dx / distance, dz / distance);
                let out = self.velocity.x.mul_add(nx, self.velocity.z * nz);
                self.velocity = DVec3::new(
                    (2.0 * out).mul_add(-nx, self.velocity.x) * 0.5,
                    self.velocity.y,
                    (2.0 * out).mul_add(-nz, self.velocity.z) * 0.5,
                );
                next.x = nx.mul_add(def.radius, center.x);
                next.z = nz.mul_add(def.radius, center.z);
            }
        }
        self.angle += self.spin;
        self.prop.glide(next, 1);
        self.shape(def, 1);
    }

    fn shape(&mut self, def: &ScatterDef, ticks: u32) {
        let rotation = if !self.resting {
            Quat::from_axis_angle(self.axis, self.angle)
        } else if def.block {
            // Blocks stand on the floor.
            Quat::from_rotation_y(self.rest_yaw)
        } else {
            // Flat items lie on it, a little above so it doesn't swallow them.
            Quat::from_rotation_y(self.rest_yaw) * Quat::from_rotation_x(FRAC_PI_2)
        };
        let lift = if self.resting && !def.block {
            0.02
        } else {
            def.size / 2.0
        };
        self.prop.shape(
            Vec3::new(0.0, lift, 0.0),
            rotation,
            Vec3::splat(def.size),
            ticks,
        );
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use steel_registry::init_vanilla_registry;

    use super::*;

    fn def(value: serde_json::Value) -> (u32, ScatterDef) {
        init_vanilla_registry();
        let serde_json::Value::Object(object) = value else {
            unreachable!("an object literal")
        };
        ScatterDef::parse(&object).expect("a scatter")
    }

    #[test]
    fn a_scatter_reads_its_defaults() {
        let (over, coins) = def(json!({"item": "minecraft:gold_nugget", "pos": [1, 5, 2]}));
        assert_eq!(over, 60);
        assert_eq!((coins.count, coins.fall, coins.vanish), (12, 60, 0));
        assert!((coins.floor - 5.0).abs() < 1e-9);
        assert!(!coins.block);
        let (over, stones) = def(json!({
            "item": "minecraft:stone", "pos": [0, 0, 0], "over": "1s", "vanish": "4s", "count": 9999
        }));
        // The cue runs until the items are gone.
        assert_eq!((over, stones.fall, stones.vanish), (80, 20, 80));
        assert_eq!(stones.count, 256);
        assert!(stones.block);
    }

    #[test]
    fn items_fall_to_the_floor_and_stay_inside_the_walls() {
        let (over, def) = def(json!({
            "item": "minecraft:gold_nugget", "pos": [0, 3, 0], "floor": 1, "radius": 2,
            "speed": 0.6, "count": 20
        }));
        let origin = DVec3::new(100.0, 64.0, -50.0);
        let mut scatter = Scatter::throw(&def, origin);
        assert_eq!(scatter.flying.len(), 20);
        for _ in 1..over {
            for flying in &mut scatter.flying {
                if !flying.resting {
                    flying.step(&def, scatter.center, scatter.floor);
                }
            }
        }
        scatter.land(&def);
        for flying in &scatter.flying {
            let at = flying.prop.pos();
            assert!(flying.resting);
            assert!((at.y - 65.0).abs() < 1e-9, "on the floor: {at}");
            let out = (at.x - 100.0).hypot(at.z + 50.0);
            assert!(out <= 2.0 + 1e-9, "inside the walls: {out}");
        }
        // The same seed throws the same way.
        let again = Scatter::throw(&def, origin);
        assert_eq!(
            again.flying[3].velocity,
            Scatter::throw(&def, origin).flying[3].velocity
        );
        assert_eq!(scatter.leave().len(), 20);
    }
}
