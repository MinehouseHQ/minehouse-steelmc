//! A flight of the camera: points joined by a smooth curve and flown at an even speed.

use glam::DVec3;
use serde_json::Value;

use super::json::{self, Object};

/// A point of a path: where the camera is and where it looks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub pos: DVec3,
    pub yaw: f32,
    pub pitch: f32,
}

/// Where the camera is at a moment of the flight.
pub type Pose = Point;

/// A camera path, in stage coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraPath {
    points: Vec<Point>,
    /// The distance along the path at each point.
    distance: Vec<f64>,
    look_at: Option<DVec3>,
}

impl CameraPath {
    /// A path through `points`; with `look_at` the camera keeps looking at that place.
    pub fn new(points: Vec<Point>, look_at: Option<DVec3>) -> Result<Self, String> {
        if points.is_empty() {
            return Err("в пути камеры нет точек".to_owned());
        }
        let mut distance = vec![0.0; points.len()];
        for index in 1..points.len() {
            distance[index] =
                distance[index - 1] + points[index].pos.distance(points[index - 1].pos);
        }
        Ok(Self {
            points,
            distance,
            look_at,
        })
    }

    /// The same flight, looking at `target` all the way.
    #[must_use]
    pub const fn looking_at(mut self, target: DVec3) -> Self {
        self.look_at = Some(target);
        self
    }

    /// A circle (or part of one) around `center`, always looking at it. Degrees, 0 is south.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a number of steps of ten degrees"
    )]
    pub fn orbit(
        center: DVec3,
        radius: f64,
        height: f64,
        from_degrees: f64,
        to_degrees: f64,
    ) -> Result<Self, String> {
        let steps = (((to_degrees - from_degrees).abs() / 10.0).ceil() as u32).clamp(2, 3600);
        let points = (0..=steps)
            .map(|step| {
                let angle = (from_degrees
                    + (to_degrees - from_degrees) * f64::from(step) / f64::from(steps))
                .to_radians();
                Point {
                    pos: center + DVec3::new(-angle.sin() * radius, height, angle.cos() * radius),
                    yaw: 0.0,
                    pitch: 0.0,
                }
            })
            .collect();
        Self::new(points, Some(center))
    }

    /// Reads `{"points": [{"pos": [x, y, z], "yaw": 0, "pitch": 0}], "look": [x, y, z]}`.
    #[expect(clippy::cast_possible_truncation, reason = "angles in degrees")]
    pub fn from_json(object: &Object) -> Result<Self, String> {
        let Some(Value::Array(items)) = json::get(object, "points") else {
            return Err("нет поля \"points\"".to_owned());
        };
        let mut points = Vec::with_capacity(items.len());
        for item in items {
            let Value::Object(point) = item else {
                return Err("points: ожидаю объекты {pos, yaw, pitch}".to_owned());
            };
            points.push(Point {
                pos: json::vec_at(point, "pos")?,
                yaw: json::number(point, "yaw", 0.0)? as f32,
                pitch: json::number(point, "pitch", 0.0)? as f32,
            });
        }
        Self::new(points, json::vec_opt(object, "look")?)
    }

    /// Where the camera is at `t` in 0..1.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "an index into the points, clamped"
    )]
    pub fn sample(&self, t: f64) -> Pose {
        let [.., last_distance] = self.distance.as_slice() else {
            return self.pose(DVec3::ZERO, 0.0, 0.0);
        };
        let count = self.points.len();
        if count == 1 {
            let only = self.points[0];
            return self.pose(only.pos, only.yaw, only.pitch);
        }

        let total = *last_distance;
        let (index, local) = if total < 1e-6 {
            // All points in one place: only the view turns, spread evenly over the points.
            let along = t.clamp(0.0, 1.0) * (count - 1) as f64;
            let index = (along as usize).min(count - 2);
            (index, along - index as f64)
        } else {
            let along = t.clamp(0.0, 1.0) * total;
            let mut index = 0;
            while index < count - 2 && self.distance[index + 1] < along {
                index += 1;
            }
            let span = self.distance[index + 1] - self.distance[index];
            let local = if span < 1e-6 {
                0.0
            } else {
                (along - self.distance[index]) / span
            };
            (index, local)
        };

        let before = self.points[index.saturating_sub(1)];
        let from = self.points[index];
        let to = self.points[index + 1];
        let after = self.points[(index + 2).min(count - 1)];
        let pos = catmull_rom(before.pos, from.pos, to.pos, after.pos, local);

        // Yaw goes the short way round between neighbouring points.
        let blend = smooth(local) as f32;
        let yaw = from.yaw + wrap(to.yaw - from.yaw) * blend;
        let pitch = from.pitch + (to.pitch - from.pitch) * blend;
        self.pose(pos, yaw, pitch)
    }

    fn pose(&self, pos: DVec3, yaw: f32, pitch: f32) -> Pose {
        match self.look_at {
            None => Point { pos, yaw, pitch },
            Some(target) => {
                let (yaw, pitch) = look(pos, target);
                Point { pos, yaw, pitch }
            }
        }
    }
}

/// Minecraft yaw and pitch to look from `from` at `to`.
#[must_use]
#[expect(clippy::cast_possible_truncation, reason = "angles in degrees")]
pub fn look(from: DVec3, to: DVec3) -> (f32, f32) {
    let delta = to - from;
    let horizontal = delta.x.hypot(delta.z);
    let yaw = (-delta.x).atan2(delta.z).to_degrees() as f32;
    let pitch = -delta.y.atan2(horizontal).to_degrees() as f32;
    (yaw, pitch)
}

/// An angle brought into -180..180.
#[must_use]
pub fn wrap(degrees: f32) -> f32 {
    let wrapped = degrees % 360.0;
    if wrapped >= 180.0 {
        wrapped - 360.0
    } else if wrapped < -180.0 {
        wrapped + 360.0
    } else {
        wrapped
    }
}

fn smooth(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

fn catmull_rom(p0: DVec3, p1: DVec3, p2: DVec3, p3: DVec3, t: f64) -> DVec3 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * (2.0 * p1
        + (p2 - p0) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: f64, z: f64) -> Point {
        Point {
            pos: DVec3::new(x, 0.0, z),
            yaw: 0.0,
            pitch: 0.0,
        }
    }

    #[test]
    fn a_path_is_flown_at_an_even_speed() {
        // A short leg and a long one: halfway in time is halfway in distance.
        let path = CameraPath::new(
            vec![point(0.0, 0.0), point(2.0, 0.0), point(10.0, 0.0)],
            None,
        )
        .expect("a path");
        assert!((path.sample(0.0).pos.x).abs() < 1e-9);
        assert!((path.sample(1.0).pos.x - 10.0).abs() < 1e-9);
        let middle = path.sample(0.5).pos.x;
        assert!((4.0..6.0).contains(&middle), "{middle}");
    }

    #[test]
    fn looking_south_is_yaw_zero() {
        let (yaw, pitch) = look(DVec3::ZERO, DVec3::new(0.0, 0.0, 5.0));
        assert!(yaw.abs() < 1e-4 && pitch.abs() < 1e-4);
        let (yaw, _) = look(DVec3::ZERO, DVec3::new(-5.0, 0.0, 0.0));
        assert!((yaw - 90.0).abs() < 1e-4);
        let (_, pitch) = look(DVec3::ZERO, DVec3::new(0.0, 5.0, 5.0));
        assert!((pitch + 45.0).abs() < 1e-4);
    }

    #[test]
    fn an_orbit_looks_at_its_center() {
        let center = DVec3::new(0.0, 10.0, 0.0);
        let path = CameraPath::orbit(center, 40.0, 25.0, 0.0, 90.0).expect("an orbit");
        let start = path.sample(0.0);
        assert!((start.pos - DVec3::new(0.0, 35.0, 40.0)).length() < 1e-6);
        // South of the center, so it looks north and down.
        assert!((wrap(start.yaw - 180.0)).abs() < 1e-3);
        assert!(start.pitch > 0.0);
        let end = path.sample(1.0);
        assert!((end.pos - DVec3::new(-40.0, 35.0, 0.0)).length() < 1e-6);
    }

    #[test]
    fn yaw_turns_the_short_way() {
        assert!((wrap(350.0) + 10.0).abs() < 1e-4);
        assert!((wrap(-190.0) - 170.0).abs() < 1e-4);
        let path = CameraPath::new(
            vec![
                Point {
                    pos: DVec3::ZERO,
                    yaw: 170.0,
                    pitch: 0.0,
                },
                Point {
                    pos: DVec3::ZERO,
                    yaw: -170.0,
                    pitch: 0.0,
                },
            ],
            None,
        )
        .expect("a path");
        assert!((path.sample(0.5).yaw - 180.0).abs() < 1e-3);
    }

    #[test]
    fn a_path_needs_points() {
        assert!(CameraPath::new(Vec::new(), None).is_err());
    }
}
