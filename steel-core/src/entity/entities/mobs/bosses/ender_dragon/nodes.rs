//! The Ender Dragon's flight graph: vanilla `EnderDragon.findClosestNode` and
//! `findPath`, an A* search over 24 fixed nodes around the End island.

use std::f64::consts::PI;
use steel_math::trig;
use steel_utils::BlockPos;

use crate::entity::ai::node::{Node, NodeHeap, NodeStore};
use crate::entity::ai::path::Path;

/// The number of nodes in the graph: 12 outer, 8 middle and 4 inner.
pub(super) const NODE_COUNT: usize = 24;
/// Index of the first middle-ring node; the outer ring circles the towers.
pub(super) const INNER_START: usize = 12;
/// The lowest height of any node.
const MIN_NODE_Y: i32 = 73;

/// Vanilla `nodeAdjacency`: bit `j` of entry `i` links node `i` to node `j`.
const NODE_ADJACENCY: [u32; NODE_COUNT] = [
    6146, 8197, 8202, 16404, 32808, 32848, 65696, 131_392, 131_712, 263_424, 526_848, 525_313,
    1_581_057, 3_166_214, 2_138_120, 6_373_424, 4_358_208, 12_910_976, 9_044_480, 9_706_496,
    15_216_640, 13_688_832, 11_763_712, 8_257_536,
];

/// The x and z of node `index` and how far above the ground it flies.
#[expect(
    clippy::cast_precision_loss,
    reason = "vanilla multiplies the node index as a float"
)]
#[must_use]
pub(super) fn node_layout(index: usize) -> (i32, i32, i32) {
    let (radius, step, ring_index, y_adjustment) = if index < 12 {
        (60.0_f32, PI / 12.0, index, 5)
    } else if index < 20 {
        (40.0, PI / 8.0, index - 12, 15)
    } else {
        (20.0, PI / 4.0, index - 20, 5)
    };
    // Vanilla `2.0F * ((float)-Math.PI + (float)(Math.PI / n) * i)`.
    let angle = 2.0 * ((-PI) as f32 + step as f32 * ring_index as f32);
    let x = (radius * trig::cos(angle)).floor() as i32;
    let z = (radius * trig::sin(angle)).floor() as i32;
    (x, z, y_adjustment)
}

/// The dragon's 24 flight nodes, built on first use from the terrain height.
pub(super) struct DragonNodes {
    nodes: Vec<Node>,
}

impl DragonNodes {
    /// Vanilla `findClosestNode`'s lazy node setup; `surface_y(x, z)` is the
    /// motion-blocking-no-leaves heightmap.
    pub(super) fn build(mut surface_y: impl FnMut(i32, i32) -> i32) -> Self {
        let nodes = (0..NODE_COUNT)
            .map(|index| {
                let (x, z, y_adjustment) = node_layout(index);
                Node::new(x, MIN_NODE_Y.max(surface_y(x, z) + y_adjustment), z)
            })
            .collect();
        Self { nodes }
    }

    /// Vanilla `findClosestNode(x, y, z)`: the outer ring only counts while
    /// crystals remain.
    #[must_use]
    pub(super) fn closest(&self, pos: BlockPos, alive_crystals: bool) -> usize {
        let current = Node::new(pos.x(), pos.y(), pos.z());
        let start = if alive_crystals { 0 } else { INNER_START };
        let mut closest_dist = 10_000.0_f32;
        let mut closest_index = 0;
        for (index, node) in self.nodes.iter().enumerate().skip(start) {
            let dist = node.distance_to_sqr(&current);
            if dist < closest_dist {
                closest_dist = dist;
                closest_index = index;
            }
        }
        closest_index
    }

    /// Vanilla `findPath`: A* from node `start` to node `end`, optionally
    /// finishing at `final_node`. Without crystals the outer ring is skipped.
    #[must_use]
    pub(super) fn find_path(
        &self,
        start: usize,
        end: usize,
        final_node: Option<Node>,
        alive_crystals: bool,
    ) -> Option<Path> {
        let mut store = NodeStore::new();
        let hashes: Vec<i32> = self
            .nodes
            .iter()
            .map(|node| {
                let stored = store.get_node(node.x, node.y, node.z);
                stored.hash()
            })
            .collect();
        let from = hashes[start];
        let to = hashes[end];
        let to_node = self.nodes[end].clone();
        if let Some(node) = store.get_mut(from) {
            node.g = 0.0;
            node.h = node.distance_to(&to_node);
            node.f = node.h;
        }
        let mut open_set = NodeHeap::new();
        open_set.insert(&mut store, from);
        let mut closest = from;
        let minimum = if alive_crystals { 0 } else { INNER_START };

        while let Some(open) = open_set.pop(&mut store) {
            if open == to {
                return Some(Self::reconstruct(&store, open, final_node));
            }
            let open_node = store.get(open)?.clone();
            if open_node.distance_to(&to_node) < store.get(closest)?.distance_to(&to_node) {
                closest = open;
            }
            if let Some(node) = store.get_mut(open) {
                node.closed = true;
            }
            let open_index = hashes.iter().position(|&hash| hash == open).unwrap_or(0);

            for (index, &adjacent) in hashes.iter().enumerate().skip(minimum) {
                if NODE_ADJACENCY[open_index] & (1 << index) == 0 {
                    continue;
                }
                let adjacent_node = store.get(adjacent)?.clone();
                if adjacent_node.closed {
                    continue;
                }
                let tentative_g = open_node.g + open_node.distance_to(&adjacent_node);
                if adjacent_node.in_open_set() && tentative_g >= adjacent_node.g {
                    continue;
                }
                let h = adjacent_node.distance_to(&to_node);
                if let Some(node) = store.get_mut(adjacent) {
                    node.came_from = Some(open);
                    node.g = tentative_g;
                    node.h = h;
                }
                if adjacent_node.in_open_set() {
                    open_set.change_cost(&mut store, adjacent, tentative_g + h);
                } else {
                    if let Some(node) = store.get_mut(adjacent) {
                        node.f = tentative_g + h;
                    }
                    open_set.insert(&mut store, adjacent);
                }
            }
        }

        if closest == from {
            return None;
        }
        log::debug!("Failed to find path from {start} to {end}");
        Some(Self::reconstruct(&store, closest, final_node))
    }

    /// Vanilla `reconstructPath`, with `final_node` appended after `last`.
    fn reconstruct(store: &NodeStore, last: i32, final_node: Option<Node>) -> Path {
        let mut nodes = Vec::new();
        let mut current = Some(last);
        while let Some(hash) = current {
            let Some(node) = store.get(hash) else {
                break;
            };
            nodes.push(node.clone());
            current = node.came_from;
        }
        nodes.reverse();
        if let Some(final_node) = final_node {
            nodes.push(final_node);
        }
        let target = nodes.last().map_or(BlockPos::ZERO, Node::as_block_pos);
        Path::new(nodes, target, true)
    }
}
