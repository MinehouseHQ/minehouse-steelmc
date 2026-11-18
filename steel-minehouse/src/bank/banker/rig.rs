//! The banker's Blockbench rig (the Fabric `Rig` of `MineHouse` Events): bones with keyframe
//! animations. The server samples the keyframes itself and shapes one item display per bone,
//! so viewers need nothing but the resource pack, which has an item model per bone.
//!
//! `rig.json` is made from `banker.bbmodel` by `scripts/gen_rig.py` of the Events mod. The maths
//! are Blockbench's: a bone sits at its pivot relative to the parent's pivot, turns in Z·Y·X
//! order, and keyframes add to the rest pose on every axis (format 5.0). Units are model pixels
//! (16 per block), the model looks at −Z.

use std::{collections::BTreeMap, sync::LazyLock};

use glam::{Mat4, Vec3};
use serde::Deserialize;

const RIG: &str = include_str!("rig.json");

/// Rotation xyz (degrees), position xyz (pixels), scale xyz, per bone.
const STRIDE: usize = 9;

#[derive(Deserialize)]
struct Bone {
    name: String,
    parent: Option<String>,
    pivot: [f32; 3],
    rest: [f32; 3],
    model: Option<String>,
}

#[derive(Deserialize)]
struct Key {
    t: f32,
    v: [f32; 3],
    #[serde(default)]
    i: Interpolation,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Interpolation {
    #[default]
    Linear,
    Catmullrom,
    Step,
}

#[derive(Default, Deserialize)]
struct Channels {
    #[serde(default)]
    rotation: Vec<Key>,
    #[serde(default)]
    position: Vec<Key>,
    #[serde(default)]
    scale: Vec<Key>,
}

#[derive(Deserialize)]
pub struct Anim {
    length: f32,
    #[serde(rename = "loop")]
    looping: String,
    bones: BTreeMap<String, Channels>,
}

#[derive(Deserialize)]
struct File {
    bones: Vec<Bone>,
    #[serde(default)]
    animations: BTreeMap<String, Anim>,
}

/// The rig, with each bone's parent found.
pub struct Rig {
    bones: Vec<Bone>,
    parents: Vec<Option<usize>>,
    anims: BTreeMap<String, Anim>,
}

/// A pose: [`STRIDE`] numbers per bone.
pub type Pose = Vec<f32>;

static BANKER: LazyLock<Option<Rig>> = LazyLock::new(|| match Rig::parse(RIG) {
    Ok(rig) => Some(rig),
    Err(error) => {
        log::error!("Bank: the banker's rig can't be read: {error}");
        None
    }
});

impl Rig {
    /// The banker's rig, unless `rig.json` is broken.
    pub fn banker() -> Option<&'static Self> {
        BANKER.as_ref()
    }

    fn parse(text: &str) -> serde_json::Result<Self> {
        let file: File = serde_json::from_str(text)?;
        let parents = file
            .bones
            .iter()
            .map(|bone| {
                bone.parent
                    .as_ref()
                    .and_then(|parent| file.bones.iter().position(|other| &other.name == parent))
            })
            .collect();
        Ok(Self {
            bones: file.bones,
            parents,
            anims: file.animations,
        })
    }

    /// The bones that are drawn: their index and item model.
    pub fn models(&self) -> impl Iterator<Item = (usize, &str)> {
        self.bones
            .iter()
            .enumerate()
            .filter_map(|(index, bone)| bone.model.as_deref().map(|model| (index, model)))
    }

    /// The animation called `name`, else `idle`.
    pub fn anim(&self, name: &str) -> Option<&Anim> {
        self.anims.get(name).or_else(|| self.anims.get("idle"))
    }

    /// The pose with no animation: everything zero, scale one.
    pub fn rest_pose(&self) -> Pose {
        let mut pose = vec![0.0; self.bones.len() * STRIDE];
        for bone in pose.as_chunks_mut::<STRIDE>().0 {
            bone[6..].fill(1.0);
        }
        pose
    }

    /// The animation at `seconds`. A looping one wraps round its length, the others stop at
    /// their last frame.
    pub fn sample(&self, anim: &Anim, seconds: f32) -> Pose {
        let time = if anim.looping == "loop" && anim.length > 0.0 {
            seconds % anim.length
        } else {
            seconds.min(anim.length)
        };
        let mut pose = self.rest_pose();
        for (index, bone) in self.bones.iter().enumerate() {
            let Some(channels) = anim.bones.get(&bone.name) else {
                continue;
            };
            for (channel, keys) in [&channels.rotation, &channels.position, &channels.scale]
                .into_iter()
                .enumerate()
            {
                if let Some(value) = value(keys, time) {
                    let at = index * STRIDE + channel * 3;
                    pose[at..at + 3].copy_from_slice(&value);
                }
            }
        }
        pose
    }

    /// Each bone's frame in model space, in blocks: where its pivot is and how it is turned,
    /// so the bone's item model (cubes around its pivot) drawn with it lands in place.
    pub fn matrices(&self, pose: &[f32]) -> Vec<Mat4> {
        let mut out: Vec<Mat4> = Vec::with_capacity(self.bones.len());
        for (index, bone) in self.bones.iter().enumerate() {
            let values = &pose[index * STRIDE..(index + 1) * STRIDE];
            let parent = self.parents[index];
            let parent_pivot = parent.map_or(Vec3::ZERO, |parent| {
                Vec3::from_array(self.bones[parent].pivot)
            });
            let base = parent.map_or(Mat4::IDENTITY, |parent| out[parent]);
            let offset = (Vec3::from_array(bone.pivot) - parent_pivot
                + Vec3::new(values[3], values[4], values[5]))
                / 16.0;
            let turn = |axis: usize| (bone.rest[axis] + values[axis]).to_radians();
            out.push(
                base * Mat4::from_translation(offset)
                    * Mat4::from_rotation_z(turn(2))
                    * Mat4::from_rotation_y(turn(1))
                    * Mat4::from_rotation_x(turn(0))
                    * Mat4::from_scale(Vec3::new(values[6], values[7], values[8])),
            );
        }
        out
    }
}

/// Blends two poses: `f` 0 is `a`, 1 is `b`.
pub fn mix(a: &[f32], b: &[f32], f: f32) -> Pose {
    a.iter().zip(b).map(|(a, b)| a + (b - a) * f).collect()
}

#[expect(
    clippy::many_single_char_names,
    reason = "the names of the spline's maths"
)]
fn value(keys: &[Key], t: f32) -> Option<[f32; 3]> {
    let first = keys.first()?;
    let last = keys.last()?;
    if t <= first.t {
        return Some(first.v);
    }
    if t >= last.t {
        return Some(last.v);
    }
    let i = keys.windows(2).position(|pair| pair[1].t >= t)?;
    let (a, b) = (&keys[i], &keys[i + 1]);
    if a.i == Interpolation::Step {
        return Some(a.v);
    }
    let span = b.t - a.t;
    let f = if span <= 0.0 { 1.0 } else { (t - a.t) / span };
    let mut out = [0.0; 3];
    if a.i == Interpolation::Catmullrom || b.i == Interpolation::Catmullrom {
        let p0 = keys[i.saturating_sub(1)].v;
        let p3 = keys[(i + 2).min(keys.len() - 1)].v;
        for k in 0..3 {
            out[k] = catmull_rom(p0[k], a.v[k], b.v[k], p3[k], f);
        }
    } else {
        for k in 0..3 {
            out[k] = a.v[k] + (b.v[k] - a.v[k]) * f;
        }
    }
    Some(out)
}

fn catmull_rom(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_banker_has_his_bones_and_animations() {
        let rig = Rig::banker().expect("rig.json");
        assert_eq!(rig.models().count(), 6);
        for name in ["idle", "look_up", "talk"] {
            assert!(rig.anims.contains_key(name), "{name}");
        }
    }

    #[test]
    fn at_rest_the_head_sits_on_the_body() {
        let rig = Rig::banker().expect("rig.json");
        let frames = rig.matrices(&rig.rest_pose());
        let head = rig
            .bones
            .iter()
            .position(|bone| bone.name == "head")
            .expect("a head");
        // The head's pivot is 24 pixels up: a block and a half.
        assert!((frames[head].w_axis.y - 1.5).abs() < 1e-5);
    }

    #[test]
    fn a_loop_wraps_and_a_hold_stays_at_its_end() {
        let rig = Rig::banker().expect("rig.json");
        let idle = rig.anim("idle").expect("idle");
        let start = rig.sample(idle, 0.0);
        let again = rig.sample(idle, idle.length);
        assert!(start.iter().zip(&again).all(|(a, b)| (a - b).abs() < 1e-3));
        let look_up = rig.anim("look_up").expect("look_up");
        assert_eq!(
            rig.sample(look_up, look_up.length),
            rig.sample(look_up, look_up.length + 5.0)
        );
    }
}
