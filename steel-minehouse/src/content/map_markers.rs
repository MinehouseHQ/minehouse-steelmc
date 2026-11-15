//! The metro for the web map and the site: the stations, the lines between them and the
//! transfers, written as a `BlueMap` marker set (the Fabric `BlueMapMarkers`). The Fabric
//! mod hands the set to `BlueMap`, which serves it as `maps/<map>/live/markers.json`; here
//! the same file is written to a directory a web server gives out, so the site's `/metro`
//! and a `BlueMap` web app read it unchanged.
//!
//! The ids carry what the site needs: `station/x/y/z/<line or none>`, `track/ax/ay/az/bx/by/bz`
//! between two stations, and `transfer/x/y/z` for a fork where the line color changes.
//!
//! The Fabric mod draws the trips its trains recorded. Here a line is the track the rail
//! graph knows between two stations, through the forks on the way.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fmt::Write as _,
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Once,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use serde_json::{Map, Value, json};
use steel_registry::dye_color::DyeColor;
use steel_utils::{BlockPos, locks::SyncMutex};

use super::{
    metro,
    rail_graph::{self, Edge},
    stations::{self, Station},
    transfers::{self, Track},
};
use crate::config;

/// The id of the marker set; the site looks for it.
pub const MARKER_SET: &str = "minehouse-metro";
/// The color of a line of no line, `0xRRGGBB`, as on the site.
const NO_LINE: u32 = 0x009A_A0A6;
/// The markers are written at most this often.
const EVERY: Duration = Duration::from_secs(2);
/// A line between two stations goes through at most this many forks.
const MAX_FORKS: usize = 64;

static DIRTY: AtomicBool = AtomicBool::new(true);
/// Marker sets of other modules, by their id and then by the name of the map.
static EXTRA: SyncMutex<BTreeMap<&'static str, BTreeMap<String, Value>>> =
    SyncMutex::new(BTreeMap::new());
static WRITER: Once = Once::new();

/// Stations or tracks changed: the markers are written again soon.
pub fn changed() {
    DIRTY.store(true, Ordering::Release);
    // A test that saves a list of its own does not write the map of the server.
    if cfg!(not(test)) {
        start();
    }
}

/// Puts the marker set `id` of another module beside the metro: one set a map. A map left
/// out loses the set.
pub fn put_extra(id: &'static str, sets: BTreeMap<String, Value>) {
    {
        let mut extra = EXTRA.lock();
        if extra.get(id) == Some(&sets) {
            return;
        }
        extra.insert(id, sets);
    }
    changed();
}

/// Starts the writer. It runs beside the server and takes the lists one at a time, so
/// writing a file never holds up a tick.
fn start() {
    WRITER.call_once(|| {
        let spawned = thread::Builder::new()
            .name("metro-markers".to_owned())
            .spawn(|| {
                loop {
                    if DIRTY.swap(false, Ordering::AcqRel) {
                        write(&directory());
                    }
                    thread::sleep(EVERY);
                }
            });
        if let Err(error) = spawned {
            log::error!("Could not start the metro map writer: {error}");
        }
    });
}

/// Where the files go: `MINEHOUSE_METRO_MAP_DIR` (the web root of the map), or `metro-map`
/// in the data directory.
fn directory() -> PathBuf {
    env::var("MINEHOUSE_METRO_MAP_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map_or_else(|| config::data_path("metro-map"), PathBuf::from)
}

/// The name `BlueMap` gives the map of a world: `world`, `world_the_nether`, `world_the_end`.
#[must_use]
pub fn map_name(world: &str) -> String {
    match world {
        "minecraft:overworld" => "world".to_owned(),
        "minecraft:the_nether" => "world_the_nether".to_owned(),
        "minecraft:the_end" => "world_the_end".to_owned(),
        other => other
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect(),
    }
}

/// A line between two stations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// The block of the station at one end, the smaller one.
    pub a: BlockPos,
    /// The block of the station at the other end.
    pub b: BlockPos,
    /// The rails where it turns, from `a` to `b`.
    pub points: Vec<BlockPos>,
    /// How many rails long it is.
    pub length: u32,
}

/// The `(x, y, z)` of a block: what the lists are sorted by.
type Pos = (i32, i32, i32);

const fn block((x, y, z): Pos) -> BlockPos {
    BlockPos::new(x, y, z)
}

const fn key(pos: BlockPos) -> Pos {
    (pos.x(), pos.y(), pos.z())
}

/// The lines of a world: from every station along the graph to the stations next to it,
/// straight through the forks. Of two ways between the same stations the shorter is kept.
#[must_use]
pub fn lines(stations: &BTreeMap<Pos, Station>, edges: &[Edge]) -> Vec<Line> {
    let mut leaving: BTreeMap<Pos, Vec<&Edge>> = BTreeMap::new();
    for edge in edges {
        leaving.entry(key(edge.from)).or_default().push(edge);
    }
    let is_station = |rail: BlockPos| stations.contains_key(&key(rail.below()));
    let mut found: BTreeMap<(Pos, Pos), Line> = BTreeMap::new();
    for &at in stations.keys() {
        let block = block(at);
        let start = block.above();
        // Runs still to follow: the node reached, the forks passed, the way so far.
        let mut todo: Vec<(BlockPos, Vec<BlockPos>, Vec<BlockPos>, u32)> =
            vec![(start, Vec::new(), vec![start], 0)];
        while let Some((node, forks, points, length)) = todo.pop() {
            for edge in leaving.get(&key(node)).map_or(&[][..], Vec::as_slice) {
                if edge.to == start || forks.contains(&edge.to) {
                    continue;
                }
                let mut way = points.clone();
                for &point in &edge.points {
                    if way.last() != Some(&point) {
                        way.push(point);
                    }
                }
                if way.last() != Some(&edge.to) {
                    way.push(edge.to);
                }
                let total = length.saturating_add(edge.length);
                if is_station(edge.to) {
                    let end = edge.to.below();
                    if key(block) >= key(end) {
                        continue;
                    }
                    let pair = (key(block), key(end));
                    if found.get(&pair).is_none_or(|known| total < known.length) {
                        found.insert(
                            pair,
                            Line {
                                a: block,
                                b: end,
                                points: way,
                                length: total,
                            },
                        );
                    }
                } else if forks.len() < MAX_FORKS {
                    let mut passed = forks.clone();
                    passed.push(edge.to);
                    todo.push((edge.to, passed, way, total));
                }
            }
        }
    }
    found.into_values().collect()
}

fn id(pos: BlockPos) -> String {
    format!("{}/{}/{}", pos.x(), pos.y(), pos.z())
}

fn rgb(line: Option<DyeColor>) -> u32 {
    line.map_or(NO_LINE, metro::line_rgb)
}

fn hex(line: Option<DyeColor>) -> String {
    format!("#{:06X}", rgb(line))
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn title(station: &Station) -> String {
    station.name.clone().unwrap_or_else(|| "Станция".to_owned())
}

fn card(title: &str, line: Option<DyeColor>) -> String {
    let what = line.map_or_else(
        || "Без линии".to_owned(),
        |line| format!("{} линия", metro::line_name(line)),
    );
    format!(
        "<div style=\"padding:4px 2px;line-height:1.4\"><b>{}</b><br><span style=\"color:{}\">●</span> {}</div>",
        escape(title),
        hex(line),
        escape(&what)
    )
}

/// An SVG picture as a data URL.
fn data_url(svg: &str) -> String {
    let mut url = String::from("data:image/svg+xml,");
    for c in svg.chars() {
        match c {
            '%' => url.push_str("%25"),
            '#' => url.push_str("%23"),
            '<' => url.push_str("%3C"),
            '>' => url.push_str("%3E"),
            '"' => url.push_str("%22"),
            other => url.push(other),
        }
    }
    url
}

/// The round metro sign in the color of the line.
fn station_icon(line: Option<DyeColor>) -> String {
    data_url(&format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='24' height='24' viewBox='0 0 24 24'>\
         <circle cx='12' cy='12' r='10.5' fill='{}' stroke='white' stroke-width='2'/>\
         <path d='M6.5 16.5 9 7.5l3 5.5 3-5.5 2.5 9' fill='none' stroke='white' stroke-width='2.2' \
         stroke-linejoin='round' stroke-linecap='round'/></svg>",
        hex(line)
    ))
}

/// A white ring, as transfers are drawn on metro maps.
fn transfer_icon() -> String {
    data_url(
        "<svg xmlns='http://www.w3.org/2000/svg' width='18' height='18' viewBox='0 0 18 18'>\
         <circle cx='9' cy='9' r='6.5' fill='white' stroke='#1b1d22' stroke-width='3'/></svg>",
    )
}

fn point(x: f64, y: f64, z: f64) -> Value {
    json!({ "x": x, "y": y, "z": z })
}

fn poi(label: &str, detail: &str, position: Value, icon: &str, anchor: i32, class: &str) -> Value {
    json!({
        "type": "poi",
        "label": label,
        "detail": detail,
        "position": position,
        "icon": icon,
        "anchor": { "x": anchor, "y": anchor },
        "classes": [class],
        "sorting": 0,
        "listed": true,
        "minDistance": 0.0,
        "maxDistance": 10_000_000.0,
    })
}

/// Forks where the line color changes, drawn like interchanges.
fn put_transfers(
    markers: &mut Map<String, Value>,
    stations: &BTreeMap<Pos, Station>,
    drawn: &[Track],
) {
    let rails: Vec<BlockPos> = stations.keys().map(|&at| block(at).above()).collect();
    for transfer in transfers::find(drawn, &rails) {
        let mut names = String::new();
        for &index in &transfer.lines {
            let Some(&line) = DyeColor::VALUES.get(index) else {
                continue;
            };
            let _ = write!(
                names,
                "<br><span style=\"color:{}\">●</span> {} линия",
                hex(Some(line)),
                escape(metro::line_name(line))
            );
        }
        let pos = transfer.pos;
        markers.insert(
            format!("transfer/{}", id(pos)),
            poi(
                "Переход",
                &format!(
                    "<div style=\"padding:4px 2px;line-height:1.4\"><b>Переход между линиями</b>{names}</div>"
                ),
                point(
                    f64::from(pos.x()) + 0.5,
                    f64::from(pos.y()) + 0.5,
                    f64::from(pos.z()) + 0.5,
                ),
                &transfer_icon(),
                9,
                "minehouse-metro-transfer",
            ),
        );
    }
}

/// The sign of every station.
fn put_stations(markers: &mut Map<String, Value>, stations: &BTreeMap<Pos, Station>) {
    for (&at, station) in stations {
        let pos = block(at);
        let name = station.name.as_deref().unwrap_or("без названия");
        markers.insert(
            format!(
                "station/{}/{}",
                id(pos),
                station.line.map_or("none", DyeColor::serialized_name)
            ),
            poi(
                &title(station),
                &card(&format!("Станция «{name}»"), station.line),
                point(
                    f64::from(pos.x()) + 0.5,
                    f64::from(pos.y()) + 1.0,
                    f64::from(pos.z()) + 0.5,
                ),
                &station_icon(station.line),
                12,
                "minehouse-metro-station",
            ),
        );
    }
}

/// The marker set of one world.
#[must_use]
pub fn marker_set(stations: &BTreeMap<Pos, Station>, edges: &[Edge]) -> Value {
    let mut markers = Map::new();
    let mut drawn = Vec::new();
    for line in lines(stations, edges) {
        let (Some(a), Some(b)) = (stations.get(&key(line.a)), stations.get(&key(line.b))) else {
            continue;
        };
        if line.points.len() < 2 {
            continue;
        }
        let color = a.line.or(b.line);
        let label = format!("{} — {}", title(a), title(b));
        let points: Vec<Value> = line
            .points
            .iter()
            .map(|p| {
                point(
                    f64::from(p.x()) + 0.5,
                    f64::from(p.y()) + 0.3,
                    f64::from(p.z()) + 0.5,
                )
            })
            .collect();
        let count = f64::from(u32::try_from(line.points.len()).unwrap_or(u32::MAX));
        let sum = |axis: fn(&BlockPos) -> i32| -> f64 {
            line.points.iter().map(|p| f64::from(axis(p))).sum::<f64>() / count
        };
        let center = point(
            sum(BlockPos::x) + 0.5,
            sum(BlockPos::y) + 0.3,
            sum(BlockPos::z) + 0.5,
        );
        let value = rgb(color);
        markers.insert(
            format!("track/{}/{}", id(line.a), id(line.b)),
            json!({
                "type": "line",
                "label": label,
                "detail": card(&label, color),
                "position": center,
                "line": points,
                "link": Value::Null,
                "newTab": false,
                // Most of a metro is underground.
                "depthTest": false,
                "lineWidth": 6,
                "lineColor": {
                    "r": (value >> 16) & 0xFF,
                    "g": (value >> 8) & 0xFF,
                    "b": value & 0xFF,
                    "a": 1.0,
                },
                "sorting": 0,
                "listed": true,
                "minDistance": 0.0,
                "maxDistance": 10_000_000.0,
            }),
        );
        drawn.push(Track {
            line: color,
            points: line.points,
        });
    }

    put_transfers(&mut markers, stations, &drawn);
    put_stations(&mut markers, stations);

    json!({
        "label": "Метро",
        "toggleable": true,
        "defaultHidden": false,
        "sorting": 0,
        "markers": markers,
    })
}

/// Writes `text` through a temporary file, so a reader never sees half of it.
fn replace(file: &Path, text: &str) -> io::Result<()> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = file.with_extension("json.tmp");
    fs::write(&temporary, text)?;
    fs::rename(&temporary, file)
}

/// Puts the set `id` of one map into its `markers.json`, leaving the other sets of the file
/// as they are. `None` takes the set out.
fn put(file: &Path, id: &str, set: Option<Value>) -> io::Result<()> {
    let mut all = fs::read_to_string(file)
        .ok()
        .and_then(|text| serde_json::from_str::<Map<String, Value>>(&text).ok())
        .unwrap_or_default();
    let before = all.get(id).cloned();
    if before == set && file.exists() {
        return Ok(());
    }
    match set {
        Some(set) => all.insert(id.to_owned(), set),
        None => all.remove(id),
    };
    let text = serde_json::to_string(&all).map_err(io::Error::other)?;
    replace(file, &text)
}

/// The maps `directory` has files of.
fn written(directory: &Path) -> BTreeSet<String> {
    let Ok(entries) = fs::read_dir(directory.join("maps")) else {
        return BTreeSet::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("live").join("markers.json").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
}

/// Lists the maps in `settings.json`, as `BlueMap` does, unless the file is someone else's:
/// then the directory is a real web root of `BlueMap` and the list is its business.
fn settings(directory: &Path, maps: &BTreeSet<String>) -> io::Result<()> {
    let file = directory.join("settings.json");
    let known = fs::read_to_string(&file)
        .ok()
        .and_then(|text| serde_json::from_str::<Map<String, Value>>(&text).ok());
    if let Some(known) = &known
        && known.get("minehouseMetro") != Some(&Value::Bool(true))
    {
        return Ok(());
    }
    let wanted = json!({ "minehouseMetro": true, "maps": maps });
    if known.map(Value::Object).as_ref() == Some(&wanted) {
        return Ok(());
    }
    let text = serde_json::to_string(&wanted).map_err(io::Error::other)?;
    replace(&file, &text)
}

/// Writes the markers of every world into `directory`.
pub fn write(directory: &Path) {
    let stations = stations::with(|stations| stations.all());
    let mut edges: BTreeMap<String, Vec<Edge>> = rail_graph::with(|graphs| graphs.all());
    let mut sets: BTreeMap<String, Option<Value>> = written(directory)
        .into_iter()
        .map(|map| (map, None))
        .collect();
    for (world, stations) in &stations {
        let edges = edges.remove(world).unwrap_or_default();
        sets.insert(map_name(world), Some(marker_set(stations, &edges)));
    }
    let extra = EXTRA.lock().clone();
    for map in extra.values().flat_map(BTreeMap::keys) {
        sets.entry(map.clone()).or_insert(None);
    }
    let maps: BTreeSet<String> = sets.keys().cloned().collect();
    let result = (|| {
        for (map, set) in sets {
            let file = directory
                .join("maps")
                .join(&map)
                .join("live")
                .join("markers.json");
            put(&file, MARKER_SET, set)?;
            for (id, by_map) in &extra {
                put(&file, id, by_map.get(&map).cloned())?;
            }
        }
        settings(directory, &maps)
    })();
    if let Err(error) = result {
        log::error!(
            "Could not write the metro map to {}: {error}",
            directory.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use std::process;

    use steel_utils::Direction;

    use super::*;

    fn rail(x: i32, z: i32) -> BlockPos {
        BlockPos::new(x, 64, z)
    }

    fn station(name: &str, line: Option<DyeColor>) -> Station {
        Station {
            name: Some(name.to_owned()),
            line,
        }
    }

    /// A straight run east from `from` to `to`, and the same run back.
    fn run(from: BlockPos, to: BlockPos) -> [Edge; 2] {
        let length = u32::try_from((to.x() - from.x()).abs() + (to.z() - from.z()).abs())
            .unwrap_or_default();
        [
            Edge {
                from,
                exit: Direction::East,
                to,
                entry: Direction::West,
                length,
                points: vec![from, to],
            },
            Edge {
                from: to,
                exit: Direction::West,
                to: from,
                entry: Direction::East,
                length,
                points: vec![to, from],
            },
        ]
    }

    #[test]
    fn a_line_joins_two_stations_through_a_fork() {
        let mut stations = BTreeMap::new();
        stations.insert(key(rail(0, 0).below()), station("А", Some(DyeColor::Red)));
        stations.insert(key(rail(40, 0).below()), station("Б", None));
        stations.insert(
            key(rail(20, 30).below()),
            station("В", Some(DyeColor::Blue)),
        );
        let fork = rail(20, 0);
        let edges: Vec<Edge> = [
            run(rail(0, 0), fork),
            run(fork, rail(40, 0)),
            run(fork, rail(20, 30)),
        ]
        .into_iter()
        .flatten()
        .collect();

        let lines = lines(&stations, &edges);
        assert_eq!(lines.len(), 3, "{lines:?}");
        let ab = lines
            .iter()
            .find(|line| line.a == rail(0, 0).below() && line.b == rail(40, 0).below());
        assert_eq!(
            ab.map(|line| (line.points.clone(), line.length)),
            Some((vec![rail(0, 0), fork, rail(40, 0)], 40))
        );

        let set = marker_set(&stations, &edges);
        let markers = set.get("markers").and_then(Value::as_object);
        let ids: Vec<&str> = markers.map_or_else(Vec::new, |markers| {
            markers.keys().map(String::as_str).collect()
        });
        assert!(ids.contains(&"station/0/63/0/red"), "{ids:?}");
        assert!(ids.contains(&"station/40/63/0/none"), "{ids:?}");
        assert!(ids.contains(&"track/0/63/0/40/63/0"), "{ids:?}");
        assert!(ids.contains(&"track/0/63/0/20/63/30"), "{ids:?}");
        let track = markers.and_then(|markers| markers.get("track/0/63/0/40/63/0"));
        assert_eq!(
            track.and_then(|track| track.pointer("/lineColor/r")),
            Some(&json!(0xEF))
        );
        assert_eq!(
            track.and_then(|track| track.pointer("/line/1/x")),
            Some(&json!(20.5))
        );
        assert_eq!(
            markers
                .and_then(|markers| markers.get("station/0/63/0/red"))
                .and_then(|station| station.get("label")),
            Some(&json!("А"))
        );
    }

    #[test]
    fn a_station_on_the_way_cuts_the_line() {
        let mut stations = BTreeMap::new();
        for x in [0, 20, 40] {
            stations.insert(key(rail(x, 0).below()), station("С", None));
        }
        let edges: Vec<Edge> = [run(rail(0, 0), rail(20, 0)), run(rail(20, 0), rail(40, 0))]
            .into_iter()
            .flatten()
            .collect();
        let lines = lines(&stations, &edges);
        assert_eq!(lines.len(), 2, "{lines:?}");
    }

    #[test]
    fn the_files_keep_other_marker_sets() {
        let directory = env::temp_dir().join(format!("metro-map-test-{}", process::id()));
        let file = directory
            .join("maps")
            .join("world")
            .join("live")
            .join("markers.json");
        let other = json!({ "label": "Other" });
        let wrote = replace(&file, &json!({ "other": other }).to_string())
            .and_then(|()| put(&file, MARKER_SET, Some(json!({ "label": "Метро" }))));
        assert!(wrote.is_ok(), "{wrote:?}");
        let read = |file: &Path| {
            fs::read_to_string(file)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .unwrap_or_default()
        };
        assert_eq!(read(&file).get("other"), Some(&other));
        assert_eq!(
            read(&file).pointer("/minehouse-metro/label"),
            Some(&json!("Метро"))
        );
        assert_eq!(written(&directory), BTreeSet::from(["world".to_owned()]));

        let listed = settings(&directory, &written(&directory));
        assert!(listed.is_ok(), "{listed:?}");
        assert_eq!(
            read(&directory.join("settings.json")).get("maps"),
            Some(&json!(["world"]))
        );
        assert!(put(&file, MARKER_SET, None).is_ok());
        assert_eq!(read(&file).get(MARKER_SET), None);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn maps_are_named_as_bluemap_names_them() {
        assert_eq!(map_name("minecraft:overworld"), "world");
        assert_eq!(map_name("minecraft:the_nether"), "world_the_nether");
        assert_eq!(map_name("minehouse:paradise"), "minehouse_paradise");
    }
}
