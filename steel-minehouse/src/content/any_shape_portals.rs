//! Nether portals in a closed obsidian frame of any shape: a circle, a heart, an arch (the
//! Fabric `AnyShapePortals`).
//!
//! The frame stands upright like a vanilla one and its opening fits in vanilla's 21 x 21.
//! Fire inside such a frame fills the whole opening with portal. The portal then holds as
//! long as every portal block has frame or portal on its four sides in its plane; anything
//! else breaks that block and the rest follows block by block, as in vanilla. Rectangles
//! still go through vanilla, and so do linking and travel: the game finds a portal by any
//! of its blocks.

use std::{
    collections::{BTreeSet, VecDeque},
    sync::Arc,
};

use steel_core::{
    PortalShapeObserver,
    world::{LevelReader as _, World},
};
use steel_registry::{
    blocks::block_state_ext::BlockStateExt as _, vanilla_block_tags::BlockTag, vanilla_blocks,
};
use steel_utils::{BlockPos, BlockStateId, axis::Axis};

/// The longest side of an opening, as vanilla's `PortalShape.MAX_WIDTH`.
const MAX_SIDE: i32 = 21;

/// What a block is to a frame.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cell {
    /// Air or fire. An already lit portal is not an empty opening.
    Empty,
    /// Obsidian.
    Frame,
    /// Anything else, or outside the build height: the opening is not closed.
    Open,
}

/// The empty cells reachable from `start` through their four sides in the vertical plane
/// along `axis`, when frame closes them in and they fit in 21 x 21.
fn interior(
    cell_at: impl Fn(BlockPos) -> Cell,
    start: BlockPos,
    axis: Axis,
) -> Option<Vec<BlockPos>> {
    if cell_at(start) != Cell::Empty {
        return None;
    }
    let along = |pos: BlockPos| if axis == Axis::X { pos.x() } else { pos.z() };
    let step = |pos: BlockPos, by: i32| {
        if axis == Axis::X {
            BlockPos::new(pos.x() + by, pos.y(), pos.z())
        } else {
            BlockPos::new(pos.x(), pos.y(), pos.z() + by)
        }
    };

    let mut seen = BTreeSet::from([(start.x(), start.y(), start.z())]);
    let mut cells = vec![start];
    let mut queue = VecDeque::from([start]);
    let (mut min_a, mut max_a) = (along(start), along(start));
    let (mut min_y, mut max_y) = (start.y(), start.y());

    while let Some(cell) = queue.pop_front() {
        min_a = min_a.min(along(cell));
        max_a = max_a.max(along(cell));
        min_y = min_y.min(cell.y());
        max_y = max_y.max(cell.y());
        if max_a - min_a + 1 > MAX_SIDE || max_y - min_y + 1 > MAX_SIDE {
            return None;
        }

        for next in [step(cell, 1), step(cell, -1), cell.above(), cell.below()] {
            if seen.contains(&(next.x(), next.y(), next.z())) {
                continue;
            }
            match cell_at(next) {
                Cell::Frame => {}
                Cell::Open => return None,
                Cell::Empty => {
                    seen.insert((next.x(), next.y(), next.z()));
                    cells.push(next);
                    queue.push_back(next);
                }
            }
        }
    }
    Some(cells)
}

/// What the block at `pos` of `world` is to a frame.
fn cell_at(world: &Arc<World>, pos: BlockPos) -> Cell {
    if world.is_outside_build_height(pos.y()) {
        return Cell::Open;
    }
    let state = world.get_block_state(pos);
    let block = state.get_block();
    if block == &vanilla_blocks::OBSIDIAN {
        Cell::Frame
    } else if state.is_air() || block.has_tag(&BlockTag::FIRE) {
        Cell::Empty
    } else {
        Cell::Open
    }
}

/// The observer that lets portals of any shape light and stand.
pub struct AnyShapePortals;

/// The one instance, installed at start.
pub static ANY_SHAPE_PORTALS: AnyShapePortals = AnyShapePortals;

impl PortalShapeObserver for AnyShapePortals {
    fn find_opening(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        preferred_axis: Axis,
    ) -> Option<(Axis, Vec<BlockPos>)> {
        let other = if preferred_axis == Axis::X {
            Axis::Z
        } else {
            Axis::X
        };
        [preferred_axis, other].into_iter().find_map(|axis| {
            interior(|at| cell_at(world, at), pos, axis).map(|cells| (axis, cells))
        })
    }

    fn holds(&self, neighbor: BlockStateId) -> bool {
        let block = neighbor.get_block();
        block == &vanilla_blocks::OBSIDIAN || block == &vanilla_blocks::NETHER_PORTAL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture in the X-Y plane at z = 0: `#` is frame, `.` is empty, anything else open.
    /// The top row is the highest.
    fn picture(rows: &'static [&'static str]) -> impl Fn(BlockPos) -> Cell {
        move |pos| {
            if pos.z() != 0 {
                return Cell::Open;
            }
            let row = rows.len() as i32 - 1 - pos.y();
            let symbol = usize::try_from(row)
                .ok()
                .and_then(|row| rows.get(row))
                .and_then(|line| {
                    usize::try_from(pos.x())
                        .ok()
                        .and_then(|x| line.as_bytes().get(x))
                });
            match symbol {
                Some(b'#') => Cell::Frame,
                Some(b'.') => Cell::Empty,
                _ => Cell::Open,
            }
        }
    }

    #[test]
    fn a_closed_frame_of_any_shape_is_an_opening() {
        let frame = picture(&[" ### ", "#...#", "#.#.#", "#...#", " ### "]);
        let cells = interior(&frame, BlockPos::new(1, 1, 0), Axis::X);
        assert_eq!(cells.map(|cells| cells.len()), Some(8));
    }

    #[test]
    fn a_gap_in_the_frame_is_no_opening() {
        let frame = picture(&[" ### ", "#...#", "#....", "#...#", " ### "]);
        assert!(interior(&frame, BlockPos::new(1, 1, 0), Axis::X).is_none());
    }

    #[test]
    fn the_frame_must_stand_in_the_plane_of_the_axis() {
        let frame = picture(&[" # ", "#.#", " # "]);
        assert!(interior(&frame, BlockPos::new(1, 1, 0), Axis::X).is_some());
        assert!(interior(&frame, BlockPos::new(1, 1, 0), Axis::Z).is_none());
    }

    #[test]
    fn an_opening_wider_than_vanilla_is_refused() {
        let wide = |pos: BlockPos| {
            if pos.z() != 0 {
                Cell::Open
            } else if pos.y() == 1 && (0..22).contains(&pos.x()) {
                Cell::Empty
            } else {
                Cell::Frame
            }
        };
        assert!(interior(wide, BlockPos::new(0, 1, 0), Axis::X).is_none());
        let fits = |pos: BlockPos| {
            if pos.z() != 0 {
                Cell::Open
            } else if pos.y() == 1 && (0..21).contains(&pos.x()) {
                Cell::Empty
            } else {
                Cell::Frame
            }
        };
        assert_eq!(
            interior(fits, BlockPos::new(0, 1, 0), Axis::X).map(|cells| cells.len()),
            Some(21)
        );
    }

    #[test]
    fn fire_must_start_in_an_empty_cell() {
        let frame = picture(&[" # ", "#.#", " # "]);
        assert!(interior(&frame, BlockPos::new(0, 1, 0), Axis::X).is_none());
    }
}
