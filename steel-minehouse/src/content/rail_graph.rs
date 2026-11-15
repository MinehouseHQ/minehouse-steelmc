//! The metro network as carts drove it. A port of the learning half of the Fabric
//! `RailGraph`. Nodes are station rails (the rail on a station block) and junctions (a
//! plain rail on a junction block with track coming in from three or four sides); an edge
//! is the run of rails from leaving one node through a side to entering the next one. Any
//! cart learns the edges it drives, both ways, so a line is known once something has gone
//! along it.
//!
//! The graph is kept in `metro-rail-graph.json` of the data directory, by world.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BinaryHeap},
    fs, io,
    path::PathBuf,
    ptr,
    sync::Arc,
};

use glam::I64Vec3;
use serde::{Deserialize, Serialize};
use steel_core::world::World;
use steel_registry::{
    blocks::{
        block_state_ext::BlockStateExt as _,
        properties::{BlockStateProperties, RailShape},
    },
    vanilla_block_tags::BlockTag,
    vanilla_blocks,
};
use steel_utils::{BlockPos, Direction, Identifier, locks::SyncMutex};

use super::metro::{JUNCTION_BLOCK, STATION_BLOCK};
use crate::config;

/// Most edges kept per world.
const MAX_EDGES: usize = 8192;
/// A run longer than this (in rails) between two nodes is not kept.
const MAX_LENGTH: u32 = 50_000;
/// Most turns of a run that are kept: a straight run is one point.
const MAX_POINTS: usize = 4096;
/// A changed graph is written at most this often, in ticks.
const SAVE_EVERY: i64 = 100;

const HORIZONTAL: [Direction; 4] = [
    Direction::North,
    Direction::East,
    Direction::South,
    Direction::West,
];

/// Leaving `from` through side `exit`, the track enters `to` through side `entry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    /// The rail of the node the run leaves.
    pub from: BlockPos,
    /// The side of `from` it leaves by.
    pub exit: Direction,
    /// The rail of the node the run comes to.
    pub to: BlockPos,
    /// The side of `to` it comes in by.
    pub entry: Direction,
    /// How many rails long the run is.
    pub length: u32,
    /// The rails where the run turns, from `from` to `to`.
    pub points: Vec<BlockPos>,
}

impl Edge {
    fn reversed(&self) -> Self {
        let mut points = self.points.clone();
        points.reverse();
        Self {
            from: self.to,
            exit: self.entry,
            to: self.from,
            entry: self.exit,
            length: self.length,
            points,
        }
    }
}

type Key = (i32, i32, i32);

const fn key(pos: BlockPos) -> Key {
    (pos.x(), pos.y(), pos.z())
}

const fn side_index(side: Direction) -> u8 {
    match side {
        Direction::North => 0,
        Direction::East => 1,
        Direction::South => 2,
        Direction::West => 3,
        Direction::Down | Direction::Up => 4,
    }
}

const fn side_name(side: Direction) -> &'static str {
    match side {
        Direction::North => "north",
        Direction::East => "east",
        Direction::South => "south",
        Direction::West => "west",
        Direction::Down => "down",
        Direction::Up => "up",
    }
}

fn side_named(name: &str) -> Option<Direction> {
    HORIZONTAL.into_iter().find(|&side| side_name(side) == name)
}

/// The graph of one world: its edges by the node and the side they leave.
#[derive(Debug, Default)]
pub struct RailGraph {
    edges: BTreeMap<(Key, u8), Edge>,
}

impl RailGraph {
    /// Learns `edge` and the same run driven the other way. Says whether anything changed.
    pub fn put(&mut self, edge: &Edge) -> bool {
        if edge.from == edge.to && edge.exit == edge.entry {
            return false;
        }
        let forward = self.put_one(edge.clone());
        let back = self.put_one(edge.reversed());
        forward || back
    }

    fn put_one(&mut self, edge: Edge) -> bool {
        let key = (key(edge.from), side_index(edge.exit));
        match self.edges.get(&key) {
            Some(known) if *known == edge => return false,
            None if self.edges.len() >= MAX_EDGES => return false,
            _ => {}
        }
        self.edges.insert(key, edge);
        true
    }

    /// The edge that leaves `node` through `side`.
    #[must_use]
    pub fn edge(&self, node: BlockPos, side: Direction) -> Option<&Edge> {
        self.edges.get(&(key(node), side_index(side)))
    }

    /// How many edges there are, each run counted both ways.
    #[must_use]
    pub fn len(&self) -> usize {
        self.edges.len()
    }

    /// Whether nothing is known.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// The block under the rail `node` went away: so do the edges through it. Says whether
    /// there were any.
    pub fn remove_node(&mut self, node: BlockPos) -> bool {
        let before = self.edges.len();
        self.edges
            .retain(|_, edge| edge.from != node && edge.to != node);
        self.edges.len() != before
    }
}

/// What a cart remembers of its way since the last node.
#[derive(Debug, Default)]
pub struct Nav {
    prev: Option<BlockPos>,
    from: Option<BlockPos>,
    exit: Option<Direction>,
    length: u32,
    points: Vec<BlockPos>,
}

impl Nav {
    /// Forgets the way: the cart left the track or was put down somewhere else.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// What a cart getting onto a rail came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Learned {
    /// Whether the graph changed.
    pub changed: bool,
    /// Whether the rail is a node.
    pub node: bool,
    /// The side the cart came onto a node through, `None` when it was put there.
    pub entry: Option<Direction>,
}

/// The cart is on rail `pos` now. `is_node` tells whether a rail is a station rail or a
/// junction.
pub fn learn(
    graph: &mut RailGraph,
    nav: &mut Nav,
    pos: BlockPos,
    is_node: impl FnOnce(BlockPos) -> bool,
) -> Learned {
    let mut prev = nav.prev;
    if prev == Some(pos) {
        return Learned::default();
    }
    if prev.is_some_and(|prev| manhattan(prev, pos) > 3) {
        // Teleported or put down somewhere else: start over.
        nav.reset();
        prev = None;
    }
    if let (Some(from), None, Some(_)) = (nav.from, nav.exit, prev) {
        nav.exit = Some(side(from, pos));
    }
    nav.length += 1;
    add_point(&mut nav.points, pos);
    if nav.length > MAX_LENGTH {
        nav.reset();
    }

    let mut learned = Learned::default();
    if is_node(pos) {
        if let (Some(from), Some(exit), Some(prev)) = (nav.from, nav.exit, prev)
            && pos != from
        {
            learned.changed = graph.put(&Edge {
                from,
                exit,
                to: pos,
                entry: side(pos, prev),
                length: nav.length,
                points: nav.points.clone(),
            });
        }
        nav.from = Some(pos);
        nav.exit = None;
        nav.length = 0;
        nav.points.clear();
        nav.points.push(pos);
        learned.node = true;
        learned.entry = prev.map(|prev| side(pos, prev));
    }
    nav.prev = Some(pos);
    learned
}

const fn manhattan(a: BlockPos, b: BlockPos) -> u32 {
    (a.x() - b.x()).unsigned_abs() + (a.y() - b.y()).unsigned_abs() + (a.z() - b.z()).unsigned_abs()
}

fn add_point(points: &mut Vec<BlockPos>, pos: BlockPos) {
    let size = points.len();
    if size >= 2 && straight(points[size - 2], points[size - 1], pos) {
        points[size - 1] = pos;
    } else if size < MAX_POINTS {
        points.push(pos);
    }
}

/// Whether the three rails lie on one straight run, in that order.
#[must_use]
pub fn straight(a: BlockPos, b: BlockPos, c: BlockPos) -> bool {
    let first = (b.0 - a.0).as_i64vec3();
    let second = (c.0 - b.0).as_i64vec3();
    first.cross(second) == I64Vec3::ZERO && first.dot(second) > 0
}

/// The horizontal side of `from` that `to` lies on.
#[must_use]
pub const fn side(from: BlockPos, to: BlockPos) -> Direction {
    let dx = to.x() - from.x();
    let dz = to.z() - from.z();
    if dx.abs() >= dz.abs() {
        if dx >= 0 {
            Direction::East
        } else {
            Direction::West
        }
    } else if dz >= 0 {
        Direction::South
    } else {
        Direction::North
    }
}

/// Whether `rail` is a rail on a station block.
#[must_use]
pub fn is_station(world: &World, rail: BlockPos) -> bool {
    ptr::eq(
        world.get_block_state(rail.below()).get_block(),
        &raw const STATION_BLOCK,
    )
}

/// Whether `rail` is a plain rail (the only kind that can turn) on a junction block, with
/// track leading into it from three or four sides.
#[must_use]
pub fn is_junction(world: &World, rail: BlockPos) -> bool {
    let on_junction = ptr::eq(
        world.get_block_state(rail.below()).get_block(),
        &raw const JUNCTION_BLOCK,
    );
    if !on_junction || world.get_block_state(rail).get_block() != &vanilla_blocks::RAIL {
        return false;
    }
    let sides = HORIZONTAL
        .into_iter()
        .filter(|&side| leads_in(world, rail, side))
        .count();
    sides >= 3
}

/// Whether `rail` is a node of the graph.
#[must_use]
pub fn is_node(world: &World, rail: BlockPos) -> bool {
    is_station(world, rail) || is_junction(world, rail)
}

/// Whether the rail next to `rail` on that side points into it (a track, not a parallel
/// line).
fn leads_in(world: &World, rail: BlockPos, side: Direction) -> bool {
    let next = rail.relative(side);
    let (step_x, step_z) = side.offset_xz();
    for pos in [next, next.below()] {
        let state = world.get_block_state(pos);
        if state.get_block().has_tag(&BlockTag::RAILS) {
            let shape = state.get_value(&BlockStateProperties::RAIL_SHAPE);
            return exits(shape)
                .into_iter()
                .any(|(x, z)| x == -step_x && z == -step_z);
        }
    }
    false
}

/// The two horizontal offsets a rail shape connects (vanilla `AbstractMinecart.EXITS`
/// without the heights).
pub(super) const fn exits(shape: RailShape) -> [(i32, i32); 2] {
    const NORTH: (i32, i32) = (0, -1);
    const SOUTH: (i32, i32) = (0, 1);
    const WEST: (i32, i32) = (-1, 0);
    const EAST: (i32, i32) = (1, 0);
    match shape {
        RailShape::NorthSouth | RailShape::AscendingNorth | RailShape::AscendingSouth => {
            [NORTH, SOUTH]
        }
        RailShape::EastWest | RailShape::AscendingEast | RailShape::AscendingWest => [WEST, EAST],
        RailShape::SouthEast => [SOUTH, EAST],
        RailShape::SouthWest => [SOUTH, WEST],
        RailShape::NorthWest => [NORTH, WEST],
        RailShape::NorthEast => [NORTH, EAST],
    }
}

/// An edge in the file.
#[derive(Serialize, Deserialize)]
struct Saved {
    from: [i32; 3],
    exit: String,
    to: [i32; 3],
    entry: String,
    length: u32,
    points: Vec<[i32; 3]>,
}

const fn array(pos: BlockPos) -> [i32; 3] {
    [pos.x(), pos.y(), pos.z()]
}

const fn block([x, y, z]: [i32; 3]) -> BlockPos {
    BlockPos::new(x, y, z)
}

/// The graphs of every world, kept in memory and written when they change.
pub struct RailGraphs {
    file: PathBuf,
    worlds: BTreeMap<String, RailGraph>,
    dirty: bool,
    saved_at: i64,
}

impl RailGraphs {
    /// Reads `file`; a missing or broken file is no track.
    #[must_use]
    pub fn load(file: PathBuf) -> Self {
        let saved: BTreeMap<String, Vec<Saved>> = match fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
                log::error!("Could not read {}, starting empty: {error}", file.display());
                BTreeMap::new()
            }),
            Err(error) => {
                if error.kind() != io::ErrorKind::NotFound {
                    log::error!("Could not read {}, starting empty: {error}", file.display());
                }
                BTreeMap::new()
            }
        };
        let worlds = saved
            .into_iter()
            .map(|(world, edges)| {
                let mut graph = RailGraph::default();
                for edge in edges {
                    let (Some(exit), Some(entry)) =
                        (side_named(&edge.exit), side_named(&edge.entry))
                    else {
                        continue;
                    };
                    let edge = Edge {
                        from: block(edge.from),
                        exit,
                        to: block(edge.to),
                        entry,
                        length: edge.length,
                        points: edge.points.into_iter().map(block).collect(),
                    };
                    graph.put_one(edge);
                }
                (world, graph)
            })
            .collect();
        Self {
            file,
            worlds,
            dirty: false,
            saved_at: 0,
        }
    }

    /// The graph of `world`.
    pub fn of(&mut self, world: &Identifier) -> &mut RailGraph {
        self.worlds.entry(world.to_string()).or_default()
    }

    /// How many edges there are, in every world.
    #[must_use]
    pub fn count(&self) -> usize {
        self.worlds.values().map(RailGraph::len).sum()
    }

    /// Writes the file now.
    pub fn save(&mut self) {
        self.dirty = false;
        let saved: BTreeMap<&str, Vec<Saved>> = self
            .worlds
            .iter()
            .filter(|(_, graph)| !graph.is_empty())
            .map(|(world, graph)| {
                let edges = graph
                    .edges
                    .values()
                    .map(|edge| Saved {
                        from: array(edge.from),
                        exit: side_name(edge.exit).to_owned(),
                        to: array(edge.to),
                        entry: side_name(edge.entry).to_owned(),
                        length: edge.length,
                        points: edge.points.iter().copied().map(array).collect(),
                    })
                    .collect();
                (world.as_str(), edges)
            })
            .collect();
        let result = (|| {
            if let Some(parent) = self.file.parent() {
                fs::create_dir_all(parent)?;
            }
            let text = serde_json::to_string(&saved).map_err(io::Error::other)?;
            let temporary = self.file.with_extension("json.tmp");
            fs::write(&temporary, text)?;
            fs::rename(&temporary, &self.file)
        })();
        if let Err(error) = result {
            log::error!("Could not save {}: {error}", self.file.display());
        }
    }
}

static GRAPHS: SyncMutex<Option<RailGraphs>> = SyncMutex::new(None);

/// Runs `action` on the graphs of the server, reading the file on the first use.
pub fn with<T>(action: impl FnOnce(&mut RailGraphs) -> T) -> T {
    let mut graphs = GRAPHS.lock();
    let graphs =
        graphs.get_or_insert_with(|| RailGraphs::load(config::data_path("metro-rail-graph.json")));
    action(graphs)
}

/// The block under the rail `node` went away: forgets the edges through it, at once.
pub fn remove_node(world: &Identifier, node: BlockPos) {
    with(|graphs| {
        if graphs.of(world).remove_node(node) {
            graphs.save();
        }
    });
}

/// The cart with `nav` is on rail `pos` of `world` now: learns the graph from it, and
/// writes a changed graph now and then.
pub fn follow(world: &Arc<World>, nav: &mut Nav, pos: BlockPos) -> Learned {
    let now = world.game_time();
    with(|graphs| {
        let learned = learn(graphs.of(&world.key), nav, pos, |pos| is_node(world, pos));
        graphs.dirty |= learned.changed;
        if graphs.dirty && (now - graphs.saved_at).abs() >= SAVE_EVERY {
            graphs.saved_at = now;
            graphs.save();
        }
        learned
    })
}

/// What a rail is to the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A rail on a station block.
    Station,
    /// A switch.
    Junction,
    /// Any other rail.
    Plain,
}

/// What `rail` of `world` is to the graph.
#[must_use]
pub fn kind(world: &World, rail: BlockPos) -> Kind {
    if is_station(world, rail) {
        Kind::Station
    } else if is_junction(world, rail) {
        Kind::Junction
    } else {
        Kind::Plain
    }
}

/// The rail shape that joins two sides of a junction.
#[must_use]
pub fn shape(a: Direction, b: Direction) -> RailShape {
    let has = |side| a == side || b == side;
    let (north, south) = (has(Direction::North), has(Direction::South));
    let (east, west) = (has(Direction::East), has(Direction::West));
    if north && south {
        RailShape::NorthSouth
    } else if east && west {
        RailShape::EastWest
    } else if north {
        if east {
            RailShape::NorthEast
        } else {
            RailShape::NorthWest
        }
    } else if east {
        RailShape::SouthEast
    } else {
        RailShape::SouthWest
    }
}

/// A way from a node: the side to leave it by, and the shape to set at each junction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The side to leave the first node by.
    pub start: Direction,
    /// The station rail the way ends on.
    pub destination: BlockPos,
    /// The shape each junction on the way has to have.
    pub switches: Vec<(BlockPos, RailShape)>,
    /// The nodes on the way after the first, in order.
    pub nodes: Vec<BlockPos>,
    /// How many rails long the way is.
    pub length: u32,
}

/// A node as a search reaches it: through a side, or standing on it.
type State = (Key, Option<u8>);

impl RailGraph {
    /// The sides a cart can leave a node by: any when it stands there, straight on through
    /// a station, any other side at a junction.
    fn exits(
        node: BlockPos,
        entry: Option<Direction>,
        kind: &impl Fn(BlockPos) -> Kind,
    ) -> Vec<Direction> {
        match entry {
            None => HORIZONTAL.to_vec(),
            Some(entry) if kind(node) == Kind::Junction => HORIZONTAL
                .into_iter()
                .filter(|&side| side != entry)
                .collect(),
            Some(entry) => vec![entry.opposite()],
        }
    }

    /// Dijkstra from `from`, entered through `entry`. `visit` sees every node as it is
    /// settled, with its distance, and stops the search by returning `true`. Gives the
    /// edge each state was reached by.
    fn search(
        &self,
        from: BlockPos,
        entry: Option<Direction>,
        kind: &impl Fn(BlockPos) -> Kind,
        mut visit: impl FnMut(BlockPos, Option<Direction>, u32) -> bool,
    ) -> (Option<State>, BTreeMap<State, (State, Edge)>) {
        let start: State = (key(from), entry.map(side_index));
        let mut best: BTreeMap<State, u32> = BTreeMap::new();
        let mut came: BTreeMap<State, (State, Edge)> = BTreeMap::new();
        let mut queue: BinaryHeap<Reverse<(u32, State)>> = BinaryHeap::new();
        best.insert(start, 0);
        queue.push(Reverse((0, start)));
        while let Some(Reverse((cost, state))) = queue.pop() {
            if best.get(&state).is_some_and(|&known| cost > known) {
                continue;
            }
            let ((x, y, z), entered) = state;
            let node = BlockPos::new(x, y, z);
            let entered = entered.and_then(|index| HORIZONTAL.get(usize::from(index)).copied());
            if state != start && visit(node, entered, cost) {
                return (Some(state), came);
            }
            for exit in Self::exits(node, entered, kind) {
                let Some(edge) = self.edge(node, exit) else {
                    continue;
                };
                let next: State = (key(edge.to), Some(side_index(edge.entry)));
                let cost = cost + edge.length;
                if best.get(&next).is_none_or(|&known| cost < known) {
                    best.insert(next, cost);
                    came.insert(next, (state, edge.clone()));
                    queue.push(Reverse((cost, next)));
                }
            }
        }
        (None, came)
    }

    /// The shortest way from rail `from` to the station rail `destination`. Entered through
    /// `entry` (`None`: standing there, any way out).
    #[must_use]
    pub fn plan(
        &self,
        from: BlockPos,
        entry: Option<Direction>,
        destination: BlockPos,
        kind: impl Fn(BlockPos) -> Kind,
    ) -> Option<Plan> {
        let mut length = 0;
        let (goal, came) = self.search(from, entry, &kind, |node, _, cost| {
            length = cost;
            node == destination
        });
        let start: State = (key(from), entry.map(side_index));
        let mut path = Vec::new();
        let mut at = goal?;
        while at != start {
            let (before, edge) = came.get(&at)?;
            path.push(edge.clone());
            at = *before;
        }
        path.reverse();
        let first = path.first()?.exit;
        let mut switches = Vec::new();
        if let Some(entry) = entry
            && kind(from) == Kind::Junction
        {
            switches.push((from, shape(entry, first)));
        }
        for pair in path.windows(2) {
            if let [edge, next] = pair
                && kind(edge.to) == Kind::Junction
            {
                switches.push((edge.to, shape(edge.entry, next.exit)));
            }
        }
        Some(Plan {
            start: first,
            destination,
            switches,
            nodes: path.iter().map(|edge| edge.to).collect(),
            length,
        })
    }

    /// The station rails reachable from `from` (standing there), nearest first, with their
    /// distance.
    #[must_use]
    pub fn reachable_stations(
        &self,
        from: BlockPos,
        kind: impl Fn(BlockPos) -> Kind,
    ) -> Vec<(BlockPos, u32)> {
        let mut stations: Vec<(BlockPos, u32)> = Vec::new();
        self.search(from, None, &kind, |node, _, cost| {
            if node != from
                && kind(node) == Kind::Station
                && !stations.iter().any(|&(known, _)| known == node)
            {
                stations.push((node, cost));
            }
            false
        });
        stations
    }
}

#[cfg(test)]
mod tests {
    use std::{env, process};

    use super::*;

    fn rail(x: i32, z: i32) -> BlockPos {
        BlockPos::new(x, 64, z)
    }

    /// Drives a cart along `way`, where the rails in `nodes` are nodes.
    fn drive(graph: &mut RailGraph, way: &[BlockPos], nodes: &[BlockPos]) -> bool {
        let mut nav = Nav::default();
        let mut changed = false;
        for &pos in way {
            changed |= learn(graph, &mut nav, pos, |pos| nodes.contains(&pos)).changed;
        }
        changed
    }

    #[test]
    fn a_cart_learns_the_run_between_two_stations_both_ways() {
        let mut graph = RailGraph::default();
        // East from (0, 0) to (3, 0), then south to (3, 4).
        let mut way: Vec<BlockPos> = (0..=3).map(|x| rail(x, 0)).collect();
        way.extend((1..=4).map(|z| rail(3, z)));
        let (a, b) = (rail(0, 0), rail(3, 4));
        assert!(drive(&mut graph, &way, &[a, b]));
        assert_eq!(graph.len(), 2);

        let there = graph.edge(a, Direction::East).cloned();
        assert_eq!(
            there,
            Some(Edge {
                from: a,
                exit: Direction::East,
                to: b,
                entry: Direction::North,
                length: 7,
                points: vec![a, rail(3, 0), b],
            })
        );
        let back = graph.edge(b, Direction::North).cloned();
        assert_eq!(back.as_ref().map(|edge| edge.to), Some(a));
        assert_eq!(back.map(|edge| edge.points), Some(vec![b, rail(3, 0), a]));
        // The same trip again teaches nothing.
        assert!(!drive(&mut graph, &way, &[a, b]));
    }

    #[test]
    fn a_cart_put_down_elsewhere_starts_over() {
        let mut graph = RailGraph::default();
        let (a, b) = (rail(0, 0), rail(40, 0));
        assert!(!drive(
            &mut graph,
            &[a, rail(1, 0), rail(39, 0), b],
            &[a, b]
        ));
        assert!(graph.is_empty());
    }

    #[test]
    fn a_removed_node_takes_its_edges() {
        let mut graph = RailGraph::default();
        let way: Vec<BlockPos> = (0..=8).map(|x| rail(x, 0)).collect();
        let nodes = [rail(0, 0), rail(4, 0), rail(8, 0)];
        drive(&mut graph, &way, &nodes);
        assert_eq!(graph.len(), 4);
        assert!(graph.remove_node(rail(8, 0)));
        assert_eq!(graph.len(), 2);
        assert!(!graph.remove_node(rail(8, 0)));
    }

    #[test]
    fn the_graph_comes_back_after_a_restart() {
        let directory = env::temp_dir().join(format!("minehouse-rail-graph-{}", process::id()));
        drop(fs::remove_dir_all(&directory));
        let file = directory.join("metro-rail-graph.json");
        let nether = Identifier::new_static("minecraft", "the_nether");
        let way: Vec<BlockPos> = (0..=5).map(|x| rail(x, 0)).collect();

        let mut graphs = RailGraphs::load(file.clone());
        drive(graphs.of(&nether), &way, &[rail(0, 0), rail(5, 0)]);
        graphs.save();

        let mut graphs = RailGraphs::load(file);
        assert_eq!(graphs.count(), 2);
        let edge = graphs
            .of(&nether)
            .edge(rail(5, 0), Direction::West)
            .cloned();
        assert_eq!(
            edge.map(|edge| (edge.to, edge.length)),
            Some((rail(0, 0), 5))
        );
    }

    /// A line west to east through a junction at (4, 0), with a branch south to (4, 5).
    fn fork() -> (RailGraph, [BlockPos; 4]) {
        let (west, junction, east, south) = (rail(0, 0), rail(4, 0), rail(8, 0), rail(4, 5));
        let nodes = [west, junction, east, south];
        let mut graph = RailGraph::default();
        let line: Vec<BlockPos> = (0..=8).map(|x| rail(x, 0)).collect();
        drive(&mut graph, &line, &nodes);
        let branch: Vec<BlockPos> = (0..=5).map(|z| rail(4, z)).collect();
        drive(&mut graph, &branch, &nodes);
        (graph, nodes)
    }

    fn fork_kind(nodes: [BlockPos; 4]) -> impl Fn(BlockPos) -> Kind {
        move |pos| {
            if pos == nodes[1] {
                Kind::Junction
            } else if nodes.contains(&pos) {
                Kind::Station
            } else {
                Kind::Plain
            }
        }
    }

    #[test]
    fn a_way_turns_at_the_junction() {
        let (graph, nodes) = fork();
        let [west, junction, east, south] = nodes;
        let plan = graph.plan(west, None, south, fork_kind(nodes));
        assert_eq!(
            plan,
            Some(Plan {
                start: Direction::East,
                destination: south,
                switches: vec![(junction, RailShape::SouthWest)],
                nodes: vec![junction, south],
                length: 9,
            })
        );
        let plan = graph.plan(south, None, east, fork_kind(nodes));
        assert_eq!(
            plan.map(|plan| plan.switches),
            Some(vec![(junction, RailShape::SouthEast)])
        );
        // A cart on the junction that came in from the west is switched where it stands.
        let plan = graph.plan(junction, Some(Direction::West), south, fork_kind(nodes));
        assert_eq!(
            plan.map(|plan| (plan.start, plan.switches)),
            Some((Direction::South, vec![(junction, RailShape::SouthWest)]))
        );
    }

    #[test]
    fn a_train_does_not_turn_back_through_a_station() {
        let mut graph = RailGraph::default();
        let way: Vec<BlockPos> = (0..=8).map(|x| rail(x, 0)).collect();
        let nodes = [rail(0, 0), rail(4, 0), rail(8, 0)];
        drive(&mut graph, &way, &nodes);
        let kind = |pos| {
            if nodes.contains(&pos) {
                Kind::Station
            } else {
                Kind::Plain
            }
        };
        // Came into the middle station from the west: the west end is behind it.
        assert_eq!(
            graph.plan(rail(4, 0), Some(Direction::West), rail(0, 0), kind),
            None
        );
        assert_eq!(
            graph.reachable_stations(rail(0, 0), kind),
            vec![(rail(4, 0), 4), (rail(8, 0), 8)]
        );
        assert_eq!(graph.reachable_stations(rail(20, 0), kind), vec![]);
    }

    #[test]
    fn three_rails_in_a_row_are_straight() {
        assert!(straight(rail(0, 0), rail(1, 0), rail(5, 0)));
        assert!(!straight(rail(0, 0), rail(1, 0), rail(1, 1)));
        assert!(!straight(rail(0, 0), rail(1, 0), rail(0, 0)));
    }
}
