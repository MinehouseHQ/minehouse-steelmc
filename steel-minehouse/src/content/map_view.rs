//! The part of the network the station map shows (a port of the Fabric `MapView`): a
//! window of the panel's size over the world's x and z, zoomed between "everything fits"
//! and [`CLOSEST`] blocks across. Panel coordinates are `u` to the right and `v` up (north
//! up), with (0, 0) in the middle of the panel.

/// At the closest zoom this many blocks span the panel's width.
const CLOSEST: f64 = 24.0;
/// One wheel notch zooms by this much.
const STEP: f64 = 1.5;
/// Room left between the outermost points of the network and the panel's edge.
const PAD: f32 = 0.2;

/// The window over the world.
#[derive(Debug, Clone)]
pub struct MapView {
    min_x: f64,
    max_x: f64,
    min_z: f64,
    max_z: f64,
    width: f32,
    height: f32,
    fit: f64,
    scale: f64,
    /// The world point in the middle of the panel.
    center_x: f64,
    center_z: f64,
}

/// A piece of a segment in panel coordinates: both of its ends.
pub type Segment = (f32, f32, f32, f32);

impl MapView {
    /// A view of the bounds given, on a panel `width` by `height`, with everything on it.
    #[must_use]
    pub fn new(
        (min_x, max_x): (f64, f64),
        (min_z, max_z): (f64, f64),
        width: f32,
        height: f32,
    ) -> Self {
        let across = f64::from(width - 2.0 * PAD) / (max_x - min_x).max(1.0);
        let down = f64::from(height - 2.0 * PAD) / (max_z - min_z).max(1.0);
        let fit = across.min(down);
        Self {
            min_x,
            max_x,
            min_z,
            max_z,
            width,
            height,
            fit,
            scale: fit,
            center_x: f64::midpoint(min_x, max_x),
            center_z: f64::midpoint(min_z, max_z),
        }
    }

    fn max_scale(&self) -> f64 {
        self.fit.max(f64::from(self.width) / CLOSEST)
    }

    /// Where world `x` is on the panel.
    #[must_use]
    pub fn u(&self, x: f64) -> f32 {
        ((x - self.center_x) * self.scale) as f32
    }

    /// Where world `z` is on the panel.
    #[must_use]
    pub fn v(&self, z: f64) -> f32 {
        (-(z - self.center_z) * self.scale) as f32
    }

    /// Zooms by `notches` wheel steps (positive is closer), keeping the world point under
    /// the look `at` where it is; without one, around the middle. Says whether anything
    /// changed.
    pub fn zoom(&mut self, notches: i32, at: Option<(f32, f32)>) -> bool {
        let next = (self.scale * STEP.powi(notches)).clamp(self.fit, self.max_scale());
        if (next - self.scale).abs() < 1.0e-9 {
            return false;
        }
        if let Some((u, v)) = at
            && self.inside(u, v, 0.0)
        {
            let (u, v) = (f64::from(u), f64::from(v));
            let world_x = self.center_x + u / self.scale;
            let world_z = self.center_z - v / self.scale;
            self.center_x = world_x - u / next;
            self.center_z = world_z + v / next;
        }
        self.scale = next;
        self.clamp();
        true
    }

    /// Drags the map by (`du`, `dv`) on the panel, as far as the edge of the network
    /// allows. Says whether it moved.
    pub fn pan(&mut self, du: f32, dv: f32) -> bool {
        let (x, z) = (self.center_x, self.center_z);
        self.center_x -= f64::from(du) / self.scale;
        self.center_z += f64::from(dv) / self.scale;
        self.clamp();
        (x - self.center_x).abs() > 1.0e-6 || (z - self.center_z).abs() > 1.0e-6
    }

    /// Keeps the window over the network: no empty space past its edge.
    fn clamp(&mut self) {
        let half_width = (f64::from(self.width) / 2.0 - f64::from(PAD)) / self.scale;
        let half_height = (f64::from(self.height) / 2.0 - f64::from(PAD)) / self.scale;
        let within = |value: f64, min: f64, max: f64, half: f64| {
            if min + half <= max - half {
                value.clamp(min + half, max - half)
            } else {
                f64::midpoint(min, max)
            }
        };
        self.center_x = within(self.center_x, self.min_x, self.max_x, half_width);
        self.center_z = within(self.center_z, self.min_z, self.max_z, half_height);
    }

    /// Whether a panel point is on the panel, or within `margin` of it.
    #[must_use]
    pub fn inside(&self, u: f32, v: f32, margin: f32) -> bool {
        u.abs() <= self.width / 2.0 + margin && v.abs() <= self.height / 2.0 + margin
    }

    /// The part of a segment that is on the panel (Liang–Barsky), `None` when none is.
    #[must_use]
    pub fn clip(&self, (u1, v1, u2, v2): Segment) -> Option<Segment> {
        let (du, dv) = (u2 - u1, v2 - v1);
        let (mut enter, mut leave) = (0.0_f32, 1.0_f32);
        let edges = [
            (-du, u1 + self.width / 2.0),
            (du, self.width / 2.0 - u1),
            (-dv, v1 + self.height / 2.0),
            (dv, self.height / 2.0 - v1),
        ];
        for (direction, room) in edges {
            if direction == 0.0 {
                if room < 0.0 {
                    return None;
                }
                continue;
            }
            let at = room / direction;
            if direction < 0.0 {
                enter = enter.max(at);
            } else {
                leave = leave.min(at);
            }
            if enter > leave {
                return None;
            }
        }
        Some((
            u1 + enter * du,
            v1 + enter * dv,
            u1 + leave * du,
            v1 + leave * dv,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> MapView {
        MapView::new((0.0, 200.0), (-50.0, 50.0), 2.6, 1.6)
    }

    #[test]
    fn at_first_the_whole_network_is_on_the_panel() {
        let view = view();
        for (x, z) in [(0.0, -50.0), (200.0, 50.0), (100.0, 0.0)] {
            assert!(view.inside(view.u(x), view.v(z), 0.0), "{x} {z}");
        }
        // North is up, east is to the right.
        assert!(view.v(-50.0) > view.v(50.0));
        assert!(view.u(200.0) > view.u(0.0));
    }

    #[test]
    fn zooming_keeps_the_point_looked_at_and_stops_at_both_ends() {
        let mut view = view();
        assert!(!view.zoom(-1, None), "it is as far out as it goes");
        let at = (view.u(150.0), view.v(10.0));
        assert!(view.zoom(1, Some(at)));
        assert!((view.u(150.0) - at.0).abs() < 1.0e-4);
        assert!((view.v(10.0) - at.1).abs() < 1.0e-4);
        while view.zoom(1, None) {}
        // Closest: 24 blocks across the panel.
        assert!((view.u(124.0) - view.u(100.0) - 2.6).abs() < 1.0e-3);
    }

    #[test]
    fn the_map_cannot_be_dragged_off_the_network() {
        let mut view = view();
        assert!(!view.pan(1.0, 0.0), "everything is on the panel already");
        view.zoom(3, None);
        assert!(view.pan(100.0, 0.0));
        assert!(!view.pan(100.0, 0.0));
        // The west end is at the padding now.
        assert!((view.u(0.0) + 1.3 - 0.2).abs() < 1.0e-3);
    }

    #[test]
    fn a_line_is_cut_at_the_edge_of_the_panel() {
        let view = view();
        let close = |cut: Option<Segment>, want: Segment| {
            cut.is_some_and(|cut| {
                [
                    cut.0 - want.0,
                    cut.1 - want.1,
                    cut.2 - want.2,
                    cut.3 - want.3,
                ]
                .iter()
                .all(|off| off.abs() < 1.0e-4)
            })
        };
        assert!(close(
            view.clip((-5.0, 0.0, 5.0, 0.0)),
            (-1.3, 0.0, 1.3, 0.0)
        ));
        assert!(close(
            view.clip((-1.0, 0.5, 1.0, 0.5)),
            (-1.0, 0.5, 1.0, 0.5)
        ));
        assert_eq!(view.clip((-5.0, 2.0, 5.0, 2.0)), None);
        assert_eq!(view.clip((2.0, -1.0, 2.0, 1.0)), None);
    }
}
