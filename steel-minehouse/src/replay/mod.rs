//! Recordings of what every player saw (the mod `ServerReplay` the Fabric server had).
//!
//! Every packet the server queues for a player is also written to a file, from the moment
//! they join to the moment they leave, and that file becomes a replay the Replay Mod opens:
//! `<folder>/<nickname>/<date>--<time>.mcpr`, named by when the player joined. Off unless
//! `config/minehouse/replay.toml` says `enabled = true`.
//!
//! The packets go to one writer thread; a finished recording is packed on another, one at
//! a time. Replays older than `keep_days` are removed, and
//! while the disk has less than `min_free_gib` free nothing is recorded. A recording a
//! stopped server left unpacked is packed at the next start.

pub mod mcpr;
mod own;

use std::{
    fs::{self, File},
    io::{self, BufReader, BufWriter, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, FixedOffset, Utc};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use steel_core::{
    player::{
        Player,
        connection::{NetworkConnection as _, PacketTap, PacketTapSource, set_packet_tap_source},
    },
    server::{Server, usable_space},
};
use steel_protocol::{
    packet_traits::{ClientPacket, EncodedPacket},
    packets::{config::CFinishConfiguration, login::CLoginFinished},
    utils::ConnectionProtocol,
};
use steel_registry::packets::CURRENT_MC_PROTOCOL;
use steel_utils::{FrontVec, MC_VERSION};
use uuid::Uuid;

use self::{
    mcpr::Meta,
    own::{OwnPlayers, Recorded},
};
use crate::config;

/// The config module: `config/minehouse/replay.toml`.
const CONFIG: &str = "replay";
/// The extension of a recording that is still being written.
const UNPACKED: &str = "tmcpr";
/// The extension of a replay.
const PACKED: &str = "mcpr";
/// How often the disk and the old replays are looked at.
const HOUSEKEEPING: Duration = Duration::from_secs(60);
/// A recording is closed before it is too big for the zip that holds it.
const LARGEST: u64 = 3 << 30;
const GIB: u64 = 1 << 30;

/// `config/minehouse/replay.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplayConfig {
    /// Whether players are recorded.
    pub enabled: bool,
    /// Where the replays go, a folder per nickname.
    pub folder: String,
    /// Replays older than this many days are removed; 0 keeps them all.
    pub keep_days: u64,
    /// Nothing is recorded while the disk has less than this many GiB free.
    pub min_free_gib: u64,
    /// The time zone of the file names, hours east of UTC.
    pub utc_offset_hours: i32,
    /// The name of the server in a replay.
    pub server_name: String,
}

impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            folder: "recordings/players".to_owned(),
            keep_days: 7,
            min_free_gib: 20,
            utc_offset_hours: 3,
            server_name: "MineHouse".to_owned(),
        }
    }
}

/// What the writer thread is told.
enum Event {
    /// A player joined: `first` is what a client is sent before the play packets.
    Start {
        id: u64,
        name: String,
        player: Uuid,
        first: Vec<Vec<u8>>,
    },
    /// A packet was queued for the player of the recording `id`.
    Packet {
        id: u64,
        at: Instant,
        frame: Arc<FrontVec>,
    },
    /// The connection of the recording `id` is gone.
    Stop { id: u64 },
}

/// Sends the packets of one connection to the writer.
struct Tap {
    id: u64,
    player: Uuid,
    events: Sender<Event>,
    recorded: Recorded,
}

impl PacketTap for Tap {
    fn queued(&self, packet: &EncodedPacket) {
        // The writer only goes away with the server.
        let _ = self.events.send(Event::Packet {
            id: self.id,
            at: Instant::now(),
            frame: Arc::clone(&packet.encoded_data),
        });
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        self.recorded.lock().remove(&self.player);
        let _ = self.events.send(Event::Stop { id: self.id });
    }
}

/// Gives every joining player a tap.
struct Recorder {
    server: Weak<Server>,
    events: Sender<Event>,
    next: AtomicU64,
    recorded: Recorded,
}

fn encoded<P: ClientPacket>(packet: &P, protocol: ConnectionProtocol) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    packet.write_packet(&mut bytes, protocol).ok()?;
    Some(bytes)
}

impl Recorder {
    /// What a client is sent between logging in and the play packets: the end of the login,
    /// the registries, the tags and the end of the configuration. A replay begins with it.
    fn first(server: &Server, player: &Player) -> Option<Vec<Vec<u8>>> {
        let compressed = player.connection.compression().is_some();
        let mut first = vec![encoded(
            &CLoginFinished::new(&player.gameprofile, Uuid::new_v4()),
            ConnectionProtocol::Login,
        )?];
        let cache = &server.registry_cache;
        for packet in cache.registry_packets.iter().chain([&*cache.tags_packet]) {
            first.push(mcpr::unframed(&packet.encoded_data, compressed)?);
        }
        first.push(encoded(
            &CFinishConfiguration {},
            ConnectionProtocol::Config,
        )?);
        Some(first)
    }
}

impl PacketTapSource for Recorder {
    fn tap_for(&self, player: &Arc<Player>) -> Option<Arc<dyn PacketTap>> {
        let server = self.server.upgrade()?;
        let Some(first) = Self::first(&server, player) else {
            log::warn!(
                "MineHouse replay: {} is not recorded, the start of a replay did not encode",
                player.gameprofile.name()
            );
            return None;
        };
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.events
            .send(Event::Start {
                id,
                name: player.gameprofile.name().to_owned(),
                player: player.gameprofile.id(),
                first,
            })
            .ok()?;
        let player = player.gameprofile.id();
        self.recorded.lock().insert(player);
        Some(Arc::new(Tap {
            id,
            player,
            events: self.events.clone(),
            recorded: Arc::clone(&self.recorded),
        }))
    }
}

/// One recording being written.
struct Recording {
    /// The file with the packets, `<stamp>_<uuid>_<started>.tmcpr`.
    file: PathBuf,
    out: BufWriter<File>,
    began: Instant,
    compressed: bool,
    written: u64,
    /// The time of the last packet, milliseconds since it began.
    last: u32,
}

fn millis_since_epoch(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    })
}

/// `2026-11-19--18-04-55` for a moment, in the time zone `hours` east of UTC.
fn stamp(started: u64, hours: i32) -> String {
    let zone = FixedOffset::east_opt(hours.clamp(-23, 23) * 3600);
    let moment = i64::try_from(started)
        .ok()
        .and_then(DateTime::<Utc>::from_timestamp_millis);
    match (moment, zone) {
        (Some(moment), Some(zone)) => moment
            .with_timezone(&zone)
            .format("%Y-%m-%d--%H-%M-%S")
            .to_string(),
        _ => started.to_string(),
    }
}

/// The name of the file of a recording that is being written.
fn unpacked_name(started: u64, hours: i32, player: Uuid) -> String {
    format!("{}_{player}_{started}.{UNPACKED}", stamp(started, hours))
}

/// What the name of such a file says: the replay it becomes, the player and the start.
fn parse_unpacked(file: &Path) -> Option<(PathBuf, Uuid, u64)> {
    if file.extension()?.to_str()? != UNPACKED {
        return None;
    }
    let mut parts = file.file_stem()?.to_str()?.rsplitn(3, '_');
    let started = parts.next()?.parse().ok()?;
    let player = Uuid::parse_str(parts.next()?).ok()?;
    let replay = file.with_file_name(format!("{}.{PACKED}", parts.next()?));
    Some((replay, player, started))
}

/// `replay`, or the same name with a number if there is such a replay already: one that
/// began in the same second.
fn unused(replay: PathBuf) -> PathBuf {
    if !replay.exists() {
        return replay;
    }
    let stem = replay
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    (2..100)
        .map(|number| replay.with_file_name(format!("{stem}-{number}.{PACKED}")))
        .find(|other| !other.exists())
        .unwrap_or(replay)
}

/// A folder name for a nickname: nothing in it can leave the folder of the replays.
fn folder_name(nickname: &str) -> String {
    let name: String = nickname
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '_')
        .collect();
    if name.is_empty() {
        "_".to_owned()
    } else {
        name
    }
}

/// A finished recording to pack.
struct Finished {
    /// The file with the packets.
    file: PathBuf,
    /// The time of the last packet; `None` reads it from the file.
    duration: Option<u32>,
}

impl Finished {
    /// Packs the packets into the replay next to them and removes them.
    fn pack(&self, server: &str) {
        let file = &self.file;
        let Some((replay, player, started)) = parse_unpacked(file) else {
            return;
        };
        let replay = unused(replay);
        let duration = self.duration.unwrap_or_else(|| {
            File::open(file).map_or(0, |packets| mcpr::duration(&mut BufReader::new(packets)))
        });
        let meta = Meta {
            server: server.to_owned(),
            duration,
            started,
            version: MC_VERSION.to_owned(),
            protocol: CURRENT_MC_PROTOCOL,
            player,
        };
        match mcpr::pack(file, &meta, &replay) {
            Ok(()) => {
                if let Err(error) = fs::remove_file(file) {
                    log::warn!("MineHouse replay: {} stays: {error}", file.display());
                }
            }
            Err(error) => {
                log::warn!(
                    "MineHouse replay: {} is not packed: {error}",
                    file.display()
                );
                // Half a zip is of no use, the packets stay for the next start.
                let _ = fs::remove_file(&replay);
            }
        }
    }
}

/// The writer thread: the recordings and the settings.
struct Writer {
    config: ReplayConfig,
    folder: PathBuf,
    compressed: bool,
    recordings: FxHashMap<u64, Recording>,
    /// Whether the disk is too full to record.
    full: bool,
    /// Where finished recordings go to be packed, one at a time, so that packing a long one
    /// does not hold up the players still being recorded.
    finished: Sender<Finished>,
}

impl Writer {
    fn pack(&self, file: PathBuf, duration: Option<u32>) {
        // The packing thread only goes away with the server.
        let _ = self.finished.send(Finished { file, duration });
    }

    fn start(&mut self, id: u64, name: &str, player: Uuid, first: &[Vec<u8>]) {
        if self.full {
            return;
        }
        let mut started = millis_since_epoch(SystemTime::now());
        let folder = self.folder.join(folder_name(name));
        if let Err(error) = fs::create_dir_all(&folder) {
            log::warn!("MineHouse replay: {name} is not recorded: {error}");
            return;
        }
        let (file, mut out) = loop {
            let file = folder.join(unpacked_name(started, self.config.utc_offset_hours, player));
            match File::create_new(&file) {
                Ok(out) => break (file, BufWriter::new(out)),
                // Two recordings never share a file: the later one begins a moment later.
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => started += 1,
                Err(error) => {
                    log::warn!("MineHouse replay: {name} is not recorded: {error}");
                    return;
                }
            }
        };
        let mut written = 0;
        for packet in first {
            if mcpr::write_packet(&mut out, 0, packet).is_err() {
                return;
            }
            written += packet.len() as u64 + 8;
        }
        self.recordings.insert(
            id,
            Recording {
                file,
                out,
                began: Instant::now(),
                compressed: self.compressed,
                written,
                last: 0,
            },
        );
    }

    fn packet(&mut self, id: u64, at: Instant, frame: &[u8]) {
        let Some(recording) = self.recordings.get_mut(&id) else {
            return;
        };
        let wrote = mcpr::unframed(frame, recording.compressed)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "a broken frame"))
            .and_then(|packet| {
                let millis = at.saturating_duration_since(recording.began).as_millis();
                recording.last = u32::try_from(millis).unwrap_or(u32::MAX);
                recording.written += packet.len() as u64 + 8;
                mcpr::write_packet(&mut recording.out, recording.last, &packet)
            });
        if let Err(error) = wrote {
            log::warn!(
                "MineHouse replay: {} ends here: {error}",
                recording.file.display()
            );
            self.stop(id);
        } else if recording.written > LARGEST {
            self.stop(id);
        }
    }

    fn stop(&mut self, id: u64) {
        let Some(mut recording) = self.recordings.remove(&id) else {
            return;
        };
        if let Err(error) = recording.out.flush() {
            log::warn!(
                "MineHouse replay: {} lost its end: {error}",
                recording.file.display()
            );
        }
        drop(recording.out);
        self.pack(recording.file, Some(recording.last));
    }

    /// The files in the folders of the players.
    fn files(&self) -> Vec<PathBuf> {
        let folders = fs::read_dir(&self.folder).into_iter().flatten().flatten();
        folders
            .flat_map(|folder| fs::read_dir(folder.path()).into_iter().flatten().flatten())
            .map(|file| file.path())
            .collect()
    }

    /// Packs what a stopped server left unpacked.
    fn recover(&self) {
        for file in self.files() {
            if parse_unpacked(&file).is_none() {
                continue;
            }
            log::info!("MineHouse replay: packing {} left behind", file.display());
            self.pack(file, None);
        }
    }

    fn housekeeping(&mut self) {
        let free = usable_space(&self.folder);
        let full = free < self.config.min_free_gib.saturating_mul(GIB);
        if full != self.full {
            self.full = full;
            if full {
                log::warn!(
                    "MineHouse replay: {} GiB free, recording stops until there is more",
                    free / GIB
                );
                let ids: Vec<u64> = self.recordings.keys().copied().collect();
                for id in ids {
                    self.stop(id);
                }
            } else {
                log::info!("MineHouse replay: there is room again, new joins are recorded");
            }
        }
        if self.config.keep_days == 0 {
            return;
        }
        let keep = Duration::from_secs(self.config.keep_days.saturating_mul(24 * 3600));
        for file in self.files() {
            let old = file
                .extension()
                .is_some_and(|extension| extension == PACKED)
                && fs::metadata(&file)
                    .and_then(|metadata| metadata.modified())
                    .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age > keep));
            if old && let Err(error) = fs::remove_file(&file) {
                log::warn!("MineHouse replay: {} stays: {error}", file.display());
            }
        }
    }

    fn run(mut self, events: &Receiver<Event>) {
        if let Err(error) = fs::create_dir_all(&self.folder) {
            log::warn!("MineHouse replay: {}: {error}", self.folder.display());
        }
        self.recover();
        self.housekeeping();
        let mut looked = Instant::now();
        loop {
            match events.recv_timeout(Duration::from_secs(1)) {
                Ok(Event::Start {
                    id,
                    name,
                    player,
                    first,
                }) => self.start(id, &name, player, &first),
                Ok(Event::Packet { id, at, frame }) => self.packet(id, at, &frame),
                Ok(Event::Stop { id }) => self.stop(id),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if looked.elapsed() >= HOUSEKEEPING {
                looked = Instant::now();
                self.housekeeping();
            }
        }
    }
}

/// Starts recording the players who join from now on, if the config says so.
pub fn start(server: &Arc<Server>) {
    let config: ReplayConfig = match config::load(&config::path(CONFIG)) {
        Ok(config) => config,
        Err(error) => {
            log::warn!("MineHouse replay: {error}, nobody is recorded");
            return;
        }
    };
    if !config.enabled {
        return;
    }
    let (events, received) = mpsc::channel();
    let (finished, to_pack) = mpsc::channel::<Finished>();
    let server_name = config.server_name.clone();
    let packing = thread::Builder::new()
        .name("minehouse-replay-pack".to_owned())
        .spawn(move || {
            for finished in to_pack {
                finished.pack(&server_name);
            }
        });
    if let Err(error) = packing {
        log::warn!("MineHouse replay: no packing thread, nobody is recorded: {error}");
        return;
    }
    let writer = Writer {
        folder: PathBuf::from(&config.folder),
        compressed: server.config.compression.is_some(),
        config,
        recordings: FxHashMap::default(),
        full: false,
        finished,
    };
    let folder = writer.folder.clone();
    let spawned = thread::Builder::new()
        .name("minehouse-replay".to_owned())
        .spawn(move || writer.run(&received));
    if let Err(error) = spawned {
        log::warn!("MineHouse replay: no writer thread, nobody is recorded: {error}");
        return;
    }
    let players = Recorded::default();
    let recorder: &'static Recorder = Box::leak(Box::new(Recorder {
        server: Arc::downgrade(server),
        events,
        next: AtomicU64::new(0),
        recorded: Arc::clone(&players),
    }));
    if set_packet_tap_source(recorder) {
        server.jobs.spawn(OwnPlayers::new(players));
        log::info!(
            "MineHouse replay: recording players to {}",
            folder.display()
        );
    } else {
        log::warn!("MineHouse replay: another extension taps the packets, nobody is recorded");
    }
}

#[cfg(test)]
mod tests {
    use std::env;

    use super::*;

    #[test]
    fn files_are_named_by_when_the_player_joined() {
        // 2026-11-19 15:04:55 UTC.
        let started = 1_795_100_695_000;
        assert_eq!(stamp(started, 3), "2026-11-19--18-04-55");
        assert_eq!(stamp(started, 0), "2026-11-19--15-04-55");
        let player = Uuid::from_u128(7);
        let name = unpacked_name(started, 3, player);
        assert_eq!(
            name,
            "2026-11-19--18-04-55_00000000-0000-0000-0000-000000000007_1795100695000.tmcpr"
        );
        let file = Path::new("recordings/players/Steve").join(name);
        assert_eq!(
            parse_unpacked(&file),
            Some((
                PathBuf::from("recordings/players/Steve/2026-11-19--18-04-55.mcpr"),
                player,
                started
            ))
        );
        assert_eq!(
            parse_unpacked(Path::new("a/2026-11-19--18-04-55.mcpr")),
            None
        );
        assert_eq!(parse_unpacked(Path::new("a/notes.tmcpr")), None);
    }

    #[test]
    fn a_nickname_cannot_leave_the_folder() {
        assert_eq!(folder_name("Steve_7"), "Steve_7");
        assert_eq!(folder_name("../../etc"), "etc");
        assert_eq!(folder_name(".."), "_");
    }

    /// A frame without compression: the length, then the packet.
    fn frame(packet: &[u8]) -> Vec<u8> {
        let mut frame = vec![u8::try_from(packet.len()).unwrap_or(0)];
        frame.extend_from_slice(packet);
        frame
    }

    #[test]
    fn a_session_becomes_a_replay_and_a_crashed_one_is_recovered() {
        let folder = env::temp_dir().join(format!("minehouse-replay-{}", Uuid::new_v4()));
        let (finished, to_pack) = mpsc::channel();
        let mut writer = Writer {
            config: ReplayConfig::default(),
            folder: folder.clone(),
            compressed: false,
            recordings: FxHashMap::default(),
            full: false,
            finished,
        };
        let player = Uuid::from_u128(7);
        writer.start(1, "Steve", player, &[vec![2, 1], vec![3]]);
        writer.packet(1, Instant::now(), &frame(&[0x2B, 9, 9]));
        writer.stop(1);
        let first: Vec<Finished> = to_pack.try_iter().collect();
        assert_eq!(first.len(), 1);
        for one in &first {
            assert!(one.duration.is_some());
            one.pack("MineHouse");
        }
        // A second session the server did not live to finish.
        writer.start(2, "Steve", player, &[vec![2, 1]]);
        writer.packet(2, Instant::now(), &frame(&[0x2B]));
        if let Some(recording) = writer.recordings.get_mut(&2) {
            assert!(recording.out.flush().is_ok());
        }
        writer.recordings.clear();
        writer.recover();
        drop(writer);
        let left: Vec<Finished> = to_pack.iter().collect();
        assert_eq!(left.len(), 1);
        for one in &left {
            assert!(one.duration.is_none());
            one.pack("MineHouse");
        }
        let mut names: Vec<String> = fs::read_dir(folder.join("Steve"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|file| file.path().extension()?.to_str().map(str::to_owned))
            .collect();
        names.sort();
        assert_eq!(names, [PACKED, PACKED]);
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn recording_is_off_unless_asked_for() {
        let config: ReplayConfig = toml::from_str("").unwrap_or_default();
        assert!(!config.enabled);
        assert_eq!(config.keep_days, 7);
        let on: Result<ReplayConfig, _> = toml::from_str("enabled = true\nkeep_days = 3\n");
        assert_eq!(
            on.map(|config| (config.enabled, config.keep_days)),
            Ok((true, 3))
        );
    }
}
