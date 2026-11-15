//! Transfers (a port of the Fabric `Transfers`): forks in the track where the line color
//! changes. Tracks of different lines run together through a shared tunnel and split at a
//! fork; if the branches out of a fork are not all the same lines, a rider can change
//! lines there, and the maps show it like an interchange. Forks at a station are the
//! station's business and are not listed.
//!
//! Works on the drawn geometry only, so it finds forks with or without a junction block.

use std::collections::{BTreeMap, BTreeSet};

use steel_registry::dye_color::DyeColor;
use steel_utils::BlockPos;

/// Forks closer than this to a station rail belong to the station.
const STATION_RADIUS: i32 = 6;
/// Fork blocks closer than this are one transfer (the few rails of a switch).
const MERGE: i32 = 6;

/// One drawn line: its color (`None` is no line) and the rails where it turns.
#[derive(Debug, Clone)]
pub struct Track {
    /// The line it is drawn as.
    pub line: Option<DyeColor>,
    /// Its corner points.
    pub points: Vec<BlockPos>,
}

/// A transfer between lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    /// The rail it is marked at.
    pub pos: BlockPos,
    /// The lines that meet there, by their index among the dyes.
    pub lines: BTreeSet<usize>,
}

type Column = (i32, i32);

fn index(line: DyeColor) -> usize {
    DyeColor::VALUES
        .iter()
        .position(|&dye| dye == line)
        .unwrap_or_default()
}

/// What the tracks look like from above.
#[derive(Default)]
struct Plan {
    next: BTreeMap<Column, BTreeSet<Column>>,
    lines: BTreeMap<Column, BTreeSet<usize>>,
    height: BTreeMap<Column, i32>,
}

/// Java `Math.round(float)` of `span * step / steps`.
fn along(span: i32, step: i32, steps: i32) -> i32 {
    if steps == 0 {
        return 0;
    }
    (span as f32 * step as f32 / steps as f32 + 0.5).floor() as i32
}

impl Plan {
    fn draw(&mut self, track: &Track) {
        let mut prev: Option<Column> = None;
        let points = &track.points;
        let segments = points
            .len()
            .saturating_sub(1)
            .max(usize::from(points.len() == 1));
        for segment in 0..segments {
            let (Some(&a), Some(&b)) = (
                points.get(segment),
                points.get((segment + 1).min(points.len() - 1)),
            ) else {
                continue;
            };
            let steps = (b.x() - a.x()).abs().max((b.z() - a.z()).abs());
            for step in 0..=steps {
                let column = (
                    a.x() + along(b.x() - a.x(), step, steps),
                    a.z() + along(b.z() - a.z(), step, steps),
                );
                if prev == Some(column) {
                    continue;
                }
                self.height
                    .entry(column)
                    .or_insert_with(|| a.y() + along(b.y() - a.y(), step, steps));
                let here = self.lines.entry(column).or_default();
                if let Some(line) = track.line {
                    here.insert(index(line));
                }
                self.next.entry(column).or_default();
                if let Some(prev) = prev {
                    self.next.entry(column).or_default().insert(prev);
                    self.next.entry(prev).or_default().insert(column);
                }
                prev = Some(column);
            }
        }
    }

    fn lines_at(&self, column: Column) -> BTreeSet<usize> {
        self.lines.get(&column).cloned().unwrap_or_default()
    }

    /// The fork blocks whose branches carry different lines, away from the stations.
    fn forks(&self, station_rails: &[BlockPos]) -> Vec<Column> {
        self.next
            .iter()
            .filter(|(_, branches)| branches.len() >= 3)
            .filter(|(_, branches)| {
                let different: BTreeSet<BTreeSet<usize>> = branches
                    .iter()
                    .map(|&branch| self.lines_at(branch))
                    .filter(|lines| !lines.is_empty())
                    .collect();
                different.len() >= 2
            })
            .map(|(&column, _)| column)
            .filter(|&(x, z)| {
                !station_rails.iter().any(|rail| {
                    (rail.x() - x).abs() <= STATION_RADIUS && (rail.z() - z).abs() <= STATION_RADIUS
                })
            })
            .collect()
    }
}

/// The transfers among `tracks`; `station_rails` are the rails of the stations drawn.
#[must_use]
pub fn find(tracks: &[Track], station_rails: &[BlockPos]) -> Vec<Transfer> {
    let mut plan = Plan::default();
    for track in tracks {
        plan.draw(track);
    }

    // The rails of one switch are one transfer.
    let mut groups: Vec<Vec<Column>> = Vec::new();
    for fork in plan.forks(station_rails) {
        let group = groups.iter_mut().find(|group| {
            group
                .first()
                .is_some_and(|&(x, z)| (x - fork.0).abs() <= MERGE && (z - fork.1).abs() <= MERGE)
        });
        match group {
            Some(group) => group.push(fork),
            None => groups.push(vec![fork]),
        }
    }

    let mut transfers: Vec<Transfer> = groups
        .iter()
        .filter_map(|group| {
            let count = group.len() as f64;
            let middle_x = group.iter().map(|&(x, _)| f64::from(x)).sum::<f64>() / count;
            let middle_z = group.iter().map(|&(_, z)| f64::from(z)).sum::<f64>() / count;
            let away = |&(x, z): &Column| (f64::from(x) - middle_x).hypot(f64::from(z) - middle_z);
            // The fork block nearest the middle, so the mark sits on the track.
            let &(x, z) = group.iter().min_by(|a, b| away(a).total_cmp(&away(b)))?;
            let lines = group
                .iter()
                .filter_map(|fork| plan.next.get(fork))
                .flatten()
                .flat_map(|&branch| plan.lines_at(branch))
                .collect();
            let y = plan.height.get(&(x, z)).copied().unwrap_or_default();
            Some(Transfer {
                pos: BlockPos::new(x, y, z),
                lines,
            })
        })
        .collect();
    transfers.sort_by_key(|transfer| (transfer.pos.x(), transfer.pos.y(), transfer.pos.z()));
    transfers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(line: Option<DyeColor>, points: &[(i32, i32)]) -> Track {
        Track {
            line,
            points: points
                .iter()
                .map(|&(x, z)| BlockPos::new(x, 64, z))
                .collect(),
        }
    }

    #[test]
    fn a_fork_where_the_color_changes_is_a_transfer() {
        // Red runs west to east; blue shares its tunnel up to x = 20 and turns south.
        let tracks = [
            track(Some(DyeColor::Red), &[(0, 0), (40, 0)]),
            track(Some(DyeColor::Blue), &[(0, 0), (20, 0), (20, 30)]),
        ];
        let transfers = find(
            &tracks,
            &[BlockPos::new(0, 64, 0), BlockPos::new(40, 64, 0)],
        );
        assert_eq!(transfers.len(), 1, "{transfers:?}");
        let transfer = &transfers[0];
        assert_eq!(transfer.pos, BlockPos::new(20, 64, 0));
        assert_eq!(
            transfer.lines,
            BTreeSet::from([index(DyeColor::Red), index(DyeColor::Blue)])
        );
    }

    #[test]
    fn a_fork_of_one_line_or_at_a_station_is_not() {
        let same = [
            track(Some(DyeColor::Red), &[(0, 0), (40, 0)]),
            track(Some(DyeColor::Red), &[(0, 0), (20, 0), (20, 30)]),
        ];
        assert_eq!(find(&same, &[]), []);

        let tracks = [
            track(Some(DyeColor::Red), &[(0, 0), (40, 0)]),
            track(Some(DyeColor::Blue), &[(0, 0), (20, 0), (20, 30)]),
        ];
        assert_eq!(find(&tracks, &[BlockPos::new(22, 64, 3)]), []);
    }
}
