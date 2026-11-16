//! Easing curves for anything that changes over time, named like on easings.net.

use std::f64::consts::PI;

/// A curve from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ease {
    Linear,
    InQuad,
    OutQuad,
    InOutQuad,
    InCubic,
    OutCubic,
    InOutCubic,
    InSine,
    OutSine,
    InOutSine,
    InExpo,
    OutExpo,
    OutBack,
    OutBounce,
}

const NAMES: [(&str, Ease); 14] = [
    ("linear", Ease::Linear),
    ("inquad", Ease::InQuad),
    ("outquad", Ease::OutQuad),
    ("inoutquad", Ease::InOutQuad),
    ("incubic", Ease::InCubic),
    ("outcubic", Ease::OutCubic),
    ("inoutcubic", Ease::InOutCubic),
    ("insine", Ease::InSine),
    ("outsine", Ease::OutSine),
    ("inoutsine", Ease::InOutSine),
    ("inexpo", Ease::InExpo),
    ("outexpo", Ease::OutExpo),
    ("outback", Ease::OutBack),
    ("outbounce", Ease::OutBounce),
];

impl Ease {
    /// The curve at `t`, which is clamped to 0..1.
    #[must_use]
    pub fn apply(self, t: f64) -> f64 {
        let x = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => x,
            Self::InQuad => x * x,
            Self::OutQuad => 1.0 - (1.0 - x) * (1.0 - x),
            Self::InOutQuad => {
                if x < 0.5 {
                    2.0 * x * x
                } else {
                    1.0 - (-2.0 * x + 2.0).powi(2) / 2.0
                }
            }
            Self::InCubic => x * x * x,
            Self::OutCubic => 1.0 - (1.0 - x).powi(3),
            Self::InOutCubic => {
                if x < 0.5 {
                    4.0 * x * x * x
                } else {
                    1.0 - (-2.0 * x + 2.0).powi(3) / 2.0
                }
            }
            Self::InSine => 1.0 - (x * PI / 2.0).cos(),
            Self::OutSine => (x * PI / 2.0).sin(),
            Self::InOutSine => -((PI * x).cos() - 1.0) / 2.0,
            Self::InExpo => {
                if x == 0.0 {
                    0.0
                } else {
                    2.0_f64.powf(10.0 * x - 10.0)
                }
            }
            Self::OutExpo => {
                if (x - 1.0).abs() < f64::EPSILON {
                    1.0
                } else {
                    1.0 - 2.0_f64.powf(-10.0 * x)
                }
            }
            Self::OutBack => {
                let c1 = 1.70158;
                let c3 = c1 + 1.0;
                1.0 + c3 * (x - 1.0).powi(3) + c1 * (x - 1.0).powi(2)
            }
            Self::OutBounce => bounce(x),
        }
    }

    /// Whether the curve is a straight line: the client can blend such a change itself.
    #[must_use]
    pub const fn is_linear(self) -> bool {
        matches!(self, Self::Linear)
    }

    /// `"inOutSine"`, `"in_out_sine"` and `"IN-OUT-SINE"` all work.
    pub fn parse(name: &str) -> Result<Self, String> {
        let key: String = name
            .trim()
            .chars()
            .filter(|character| *character != '_' && *character != '-')
            .flat_map(char::to_lowercase)
            .collect();
        NAMES
            .iter()
            .find(|(known, _)| *known == key)
            .map(|(_, ease)| *ease)
            .ok_or_else(|| format!("нет такой кривой \"{name}\""))
    }
}

fn bounce(x: f64) -> f64 {
    let n1 = 7.5625;
    let d1 = 2.75;
    if x < 1.0 / d1 {
        n1 * x * x
    } else if x < 2.0 / d1 {
        let x = x - 1.5 / d1;
        n1 * x * x + 0.75
    } else if x < 2.5 / d1 {
        let x = x - 2.25 / d1;
        n1 * x * x + 0.9375
    } else {
        let x = x - 2.625 / d1;
        n1 * x * x + 0.984_375
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_curve_runs_from_zero_to_one() {
        for (name, ease) in NAMES {
            assert!(ease.apply(0.0).abs() < 1e-9, "{name} at 0");
            assert!((ease.apply(1.0) - 1.0).abs() < 1e-9, "{name} at 1");
            assert!((ease.apply(7.0) - 1.0).abs() < 1e-9, "{name} past 1");
        }
    }

    #[test]
    fn names_are_read_in_any_spelling() {
        assert_eq!(Ease::parse("inOutSine"), Ok(Ease::InOutSine));
        assert_eq!(Ease::parse("in_out_sine"), Ok(Ease::InOutSine));
        assert_eq!(Ease::parse("IN-OUT-SINE"), Ok(Ease::InOutSine));
        assert_eq!(
            Ease::parse("wobble"),
            Err("нет такой кривой \"wobble\"".to_owned())
        );
    }

    #[test]
    fn quad_curves_bend_the_right_way() {
        assert!((Ease::InQuad.apply(0.5) - 0.25).abs() < 1e-9);
        assert!((Ease::OutQuad.apply(0.5) - 0.75).abs() < 1e-9);
    }
}
