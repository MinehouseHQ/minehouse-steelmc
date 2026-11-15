//! The log on disk: one `SQLite` file, written by one thread and read by another, so neither a
//! burst of events nor a wide lookup ever waits on the server tick.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread::{self, JoinHandle},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, params, params_from_iter, types::Value};
use rustc_hash::FxHashMap;
use steel_utils::locks::SyncMutex;

/// A block was broken.
pub const BREAK: u8 = 0;
/// A block was placed.
pub const PLACE: u8 = 1;
/// A block was used. Only in history brought over from `CoreProtect`.
pub const CLICK: u8 = 2;
/// Items were taken out of a container.
pub const TAKE: u8 = 0;
/// Items were put into a container.
pub const PUT: u8 = 1;

/// How many queued jobs one transaction takes at most.
const BATCH: usize = 20_000;
/// How often the writer looks at the clock when nothing is queued.
const IDLE: Duration = Duration::from_secs(60);
/// How often old rows are purged.
const PURGE_EVERY_MS: i64 = 6 * 3600 * 1000;
/// Rows deleted per transaction of a purge, so the queue keeps moving.
const PURGE_STEP: i64 = 50_000;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS actor (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE COLLATE NOCASE, uuid TEXT);
CREATE TABLE IF NOT EXISTS world (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS kind (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS block (
    id INTEGER PRIMARY KEY, time INTEGER NOT NULL, actor INTEGER NOT NULL, world INTEGER NOT NULL,
    x INTEGER NOT NULL, y INTEGER NOT NULL, z INTEGER NOT NULL, kind INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT '', action INTEGER NOT NULL, rolled_back INTEGER NOT NULL DEFAULT 0);
CREATE INDEX IF NOT EXISTS block_place ON block (world, x, z, y);
CREATE INDEX IF NOT EXISTS block_actor ON block (actor, time);
CREATE INDEX IF NOT EXISTS block_time ON block (time);
CREATE TABLE IF NOT EXISTS container (
    id INTEGER PRIMARY KEY, time INTEGER NOT NULL, actor INTEGER NOT NULL, world INTEGER NOT NULL,
    x INTEGER NOT NULL, y INTEGER NOT NULL, z INTEGER NOT NULL, kind INTEGER NOT NULL,
    amount INTEGER NOT NULL, data BLOB, action INTEGER NOT NULL, rolled_back INTEGER NOT NULL DEFAULT 0);
CREATE INDEX IF NOT EXISTS container_place ON container (world, x, z, y);
CREATE INDEX IF NOT EXISTS container_actor ON container (actor, time);
CREATE INDEX IF NOT EXISTS container_time ON container (time);
";

/// Who did it: a player, or a cause like `#tnt` in imported history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    /// The nickname, or `#cause`.
    pub name: String,
    /// The account, for players.
    pub uuid: Option<String>,
}

/// What an event is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// A block with its state properties (`facing=north,lit=false`, empty for none).
    Block {
        /// The properties of the state.
        state: String,
    },
    /// A stack in a container: its size and, if it is more than a plain item, its NBT.
    Items {
        /// How many items.
        amount: i32,
        /// The stack as NBT, when it has components of its own.
        data: Option<Vec<u8>>,
    },
}

/// One line of the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// Unix milliseconds.
    pub time: i64,
    /// Who did it.
    pub actor: Actor,
    /// The key of the world, `minecraft:overworld`.
    pub world: String,
    /// The block position.
    pub pos: (i32, i32, i32),
    /// The block or item id, `minecraft:stone`.
    pub kind: String,
    /// [`BREAK`], [`PLACE`], [`CLICK`], or [`TAKE`], [`PUT`] for items.
    pub action: u8,
    /// Whether an earlier rollback undid it.
    pub rolled_back: bool,
    /// The block state or the stack.
    pub subject: Subject,
}

/// Which events of one table a lookup wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actions {
    /// Every action.
    Any,
    /// One action: [`BREAK`], [`PLACE`], [`TAKE`], [`PUT`].
    Only(u8),
}

/// What a lookup asks for. Empty fields don't narrow it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// The key of the world.
    pub world: Option<String>,
    /// The middle of the searched box and how far it reaches; `0` is the one block.
    pub around: Option<((i32, i32, i32), i32)>,
    /// Nicknames (or `#causes`), any of them.
    pub actors: Vec<String>,
    /// Only events at or after this time.
    pub since: Option<i64>,
    /// Block or item ids, any of them.
    pub kinds: Vec<String>,
    /// Which block events; `None` with `containers` also `None` is everything.
    pub blocks: Option<Actions>,
    /// Which container events.
    pub containers: Option<Actions>,
    /// Rows to skip, newest first.
    pub offset: u32,
    /// Rows to return.
    pub limit: u32,
}

/// The answer to a [`Query`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    /// The rows, newest first.
    pub rows: Vec<Event>,
    /// How many rows match in all.
    pub total: u64,
}

/// How much the log holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Block rows.
    pub blocks: u64,
    /// Container rows.
    pub containers: u64,
    /// The oldest row, Unix milliseconds.
    pub oldest: Option<i64>,
}

type Reply<T> = Box<dyn FnOnce(Result<T, String>) + Send>;

enum Write {
    Event(Box<Event>),
    /// History from another log, with the last source row id of its table.
    Import(Vec<Event>, &'static str, i64),
    Flush(Sender<()>),
    Purge(i64, Reply<u64>),
    Stop,
}

enum Read {
    Lookup(Box<Query>, Reply<Page>),
    Stats(Reply<Stats>),
    Imported(&'static str, Reply<i64>),
    Stop,
}

/// The handle of the log. Cheap calls: everything is queued for the two threads.
pub struct Store {
    writer: Sender<Write>,
    reader: Sender<Read>,
    threads: SyncMutex<Vec<JoinHandle<()>>>,
}

impl Store {
    /// Opens (or creates) the log at `file` and starts its threads. `keep_ms` is how long rows
    /// are kept; `None` keeps them forever.
    pub fn open(file: &Path, keep_ms: Option<i64>) -> Result<Self, String> {
        if let Some(folder) = file.parent() {
            fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        }
        let connection = Connection::open(file).map_err(|error| error.to_string())?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 60000;",
            )
            .and_then(|()| connection.execute_batch(SCHEMA))
            .map_err(|error| error.to_string())?;

        let (writer, write_jobs) = mpsc::channel();
        let (reader, read_jobs) = mpsc::channel();
        let flusher = writer.clone();
        let path = file.to_path_buf();
        let writing = thread::Builder::new()
            .name("minehouse-blocklog-write".to_owned())
            .spawn(move || Writer::new(connection, keep_ms).run(&write_jobs))
            .map_err(|error| error.to_string())?;
        let reading = thread::Builder::new()
            .name("minehouse-blocklog-read".to_owned())
            .spawn(move || read_loop(&path, &read_jobs, &flusher))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            writer,
            reader,
            threads: SyncMutex::new(vec![writing, reading]),
        })
    }

    /// Queues one event. Never blocks.
    pub fn record(&self, event: Event) {
        let _ = self.writer.send(Write::Event(Box::new(event)));
    }

    /// Queues imported history of `table`, remembering `last_id` as how far the import got.
    pub fn import(&self, events: Vec<Event>, table: &'static str, last_id: i64) {
        let _ = self.writer.send(Write::Import(events, table, last_id));
    }

    /// Blocks until everything queued so far is on disk. Not for the tick.
    pub fn flush(&self) {
        flush(&self.writer);
    }

    /// Runs `query` on the reader thread and hands the page to `reply` there.
    pub fn lookup(&self, query: Query, reply: impl FnOnce(Result<Page, String>) + Send + 'static) {
        let _ = self
            .reader
            .send(Read::Lookup(Box::new(query), Box::new(reply)));
    }

    /// Counts the log on the reader thread.
    pub fn stats(&self, reply: impl FnOnce(Result<Stats, String>) + Send + 'static) {
        let _ = self.reader.send(Read::Stats(Box::new(reply)));
    }

    /// The last source row id an import of `table` reached, `0` for none.
    pub fn imported(
        &self,
        table: &'static str,
        reply: impl FnOnce(Result<i64, String>) + Send + 'static,
    ) {
        let _ = self.reader.send(Read::Imported(table, Box::new(reply)));
    }

    /// Deletes the rows older than `before` on the writer thread and reports how many.
    pub fn purge(&self, before: i64, reply: impl FnOnce(Result<u64, String>) + Send + 'static) {
        let _ = self.writer.send(Write::Purge(before, Box::new(reply)));
    }

    /// Writes what is queued and stops both threads.
    pub fn stop(&self) {
        let _ = self.reader.send(Read::Stop);
        let _ = self.writer.send(Write::Stop);
        for thread in self.threads.lock().drain(..) {
            let _ = thread.join();
        }
    }
}

fn flush(writer: &Sender<Write>) {
    let (done, wait) = mpsc::channel();
    if writer.send(Write::Flush(done)).is_ok() {
        let _ = wait.recv();
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

struct Writer {
    connection: Connection,
    keep_ms: Option<i64>,
    next_purge: i64,
    actors: FxHashMap<String, i64>,
    worlds: FxHashMap<String, i64>,
    kinds: FxHashMap<String, i64>,
}

impl Writer {
    fn new(connection: Connection, keep_ms: Option<i64>) -> Self {
        Self {
            connection,
            keep_ms,
            // Shortly after start, once the server has settled.
            next_purge: now_ms() + 5 * 60 * 1000,
            actors: FxHashMap::default(),
            worlds: FxHashMap::default(),
            kinds: FxHashMap::default(),
        }
    }

    fn run(mut self, jobs: &Receiver<Write>) {
        loop {
            let first = match jobs.recv_timeout(IDLE) {
                Ok(job) => Some(job),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            let mut batch: Vec<Write> = first.into_iter().collect();
            while batch.len() < BATCH
                && let Ok(job) = jobs.try_recv()
            {
                batch.push(job);
            }
            let stop = batch.iter().any(|job| matches!(job, Write::Stop));
            if let Err(error) = self.write(batch) {
                log::error!("Block log: a batch was not written: {error}");
            }
            if stop {
                return;
            }
            self.purge_if_due();
        }
    }

    fn write(&mut self, batch: Vec<Write>) -> rusqlite::Result<()> {
        let mut flushes = Vec::new();
        let mut purges = Vec::new();
        self.connection.execute_batch("BEGIN")?;
        let written = (|| {
            for job in batch {
                match job {
                    Write::Event(event) => self.insert(&event)?,
                    Write::Import(events, table, last_id) => {
                        for event in &events {
                            self.insert(event)?;
                        }
                        self.connection.execute(
                            "INSERT INTO meta (key, value) VALUES (?1, ?2)
                             ON CONFLICT (key) DO UPDATE SET value = max(value, excluded.value)",
                            params![format!("import:{table}"), last_id],
                        )?;
                    }
                    Write::Flush(done) => flushes.push(done),
                    Write::Purge(before, reply) => purges.push((before, reply)),
                    Write::Stop => {}
                }
            }
            Ok(())
        })();
        let finished = match written {
            Ok(()) => self.connection.execute_batch("COMMIT"),
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                // Ids handed out inside the lost transaction are gone with it.
                self.actors.clear();
                self.worlds.clear();
                self.kinds.clear();
                Err(error)
            }
        };
        for done in flushes {
            let _ = done.send(());
        }
        for (before, reply) in purges {
            reply(self.purge(before).map_err(|error| error.to_string()));
        }
        finished
    }

    fn insert(&mut self, event: &Event) -> rusqlite::Result<()> {
        let actor = self.actor(&event.actor)?;
        let world = intern(&self.connection, &mut self.worlds, "world", &event.world)?;
        let kind = intern(&self.connection, &mut self.kinds, "kind", &event.kind)?;
        let (x, y, z) = event.pos;
        match &event.subject {
            Subject::Block { state } => self
                .connection
                .prepare_cached(
                    "INSERT INTO block (time, actor, world, x, y, z, kind, state, action, rolled_back)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                )?
                .execute(params![
                    event.time, actor, world, x, y, z, kind, state, event.action, event.rolled_back
                ]),
            Subject::Items { amount, data } => self
                .connection
                .prepare_cached(
                    "INSERT INTO container (time, actor, world, x, y, z, kind, amount, data, action, rolled_back)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                )?
                .execute(params![
                    event.time, actor, world, x, y, z, kind, amount, data, event.action,
                    event.rolled_back
                ]),
        }
        .map(|_| ())
    }

    fn actor(&mut self, actor: &Actor) -> rusqlite::Result<i64> {
        let key = actor.name.to_lowercase();
        if let Some(id) = self.actors.get(&key) {
            return Ok(*id);
        }
        // A new nickname of a known account keeps the account's line.
        self.connection
            .prepare_cached(
                "INSERT INTO actor (name, uuid) VALUES (?1, ?2)
                 ON CONFLICT (name) DO UPDATE SET uuid = coalesce(excluded.uuid, uuid)",
            )?
            .execute(params![actor.name, actor.uuid])?;
        let id = self
            .connection
            .prepare_cached("SELECT id FROM actor WHERE name = ?1")?
            .query_row(params![actor.name], |row| row.get(0))?;
        self.actors.insert(key, id);
        Ok(id)
    }

    fn purge_if_due(&mut self) {
        let Some(keep_ms) = self.keep_ms else {
            return;
        };
        let now = now_ms();
        if now < self.next_purge {
            return;
        }
        self.next_purge = now + PURGE_EVERY_MS;
        match self.purge(now - keep_ms) {
            Ok(0) => {}
            Ok(rows) => log::info!("Block log: purged {rows} old rows"),
            Err(error) => log::warn!("Block log: purge failed: {error}"),
        }
    }

    fn purge(&self, before: i64) -> rusqlite::Result<u64> {
        let mut purged = 0;
        for table in ["block", "container"] {
            loop {
                let rows = self.connection.execute(
                    &format!(
                        "DELETE FROM {table} WHERE id IN
                         (SELECT id FROM {table} WHERE time < ?1 LIMIT {PURGE_STEP})"
                    ),
                    params![before],
                )?;
                purged += rows as u64;
                if rows == 0 {
                    break;
                }
            }
        }
        Ok(purged)
    }
}

fn intern(
    connection: &Connection,
    cache: &mut FxHashMap<String, i64>,
    table: &str,
    name: &str,
) -> rusqlite::Result<i64> {
    if let Some(id) = cache.get(name) {
        return Ok(*id);
    }
    connection
        .prepare_cached(&format!("INSERT OR IGNORE INTO {table} (name) VALUES (?1)"))?
        .execute(params![name])?;
    let id = connection
        .prepare_cached(&format!("SELECT id FROM {table} WHERE name = ?1"))?
        .query_row(params![name], |row| row.get(0))?;
    cache.insert(name.to_owned(), id);
    Ok(id)
}

fn read_loop(file: &PathBuf, jobs: &Receiver<Read>, writer: &Sender<Write>) {
    let opened = Connection::open_with_flags(
        file,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .and_then(|connection| {
        connection.execute_batch("PRAGMA busy_timeout = 60000;")?;
        Ok(connection)
    });
    while let Ok(job) = jobs.recv() {
        let connection = opened.as_ref().map_err(ToString::to_string);
        match job {
            Read::Lookup(query, reply) => {
                // So a lookup sees what happened a moment ago.
                flush(writer);
                reply(connection.and_then(|connection| {
                    lookup(connection, &query).map_err(|error| error.to_string())
                }));
            }
            Read::Stats(reply) => reply(
                connection
                    .and_then(|connection| stats(connection).map_err(|error| error.to_string())),
            ),
            Read::Imported(table, reply) => {
                flush(writer);
                reply(connection.and_then(|connection| {
                    connection
                        .query_row(
                            "SELECT coalesce(max(value), 0) FROM meta WHERE key = ?1",
                            params![format!("import:{table}")],
                            |row| row.get(0),
                        )
                        .map_err(|error| error.to_string())
                }));
            }
            Read::Stop => return,
        }
    }
}

fn stats(connection: &Connection) -> rusqlite::Result<Stats> {
    let count = |table: &str| -> rusqlite::Result<(u64, Option<i64>)> {
        connection.query_row(
            &format!("SELECT count(*), min(time) FROM {table}"),
            [],
            |row| Ok((row.get::<_, i64>(0)?.unsigned_abs(), row.get(1)?)),
        )
    };
    let (blocks, oldest_block) = count("block")?;
    let (containers, oldest_container) = count("container")?;
    Ok(Stats {
        blocks,
        containers,
        oldest: match (oldest_block, oldest_container) {
            (Some(block), Some(container)) => Some(block.min(container)),
            (block, container) => block.or(container),
        },
    })
}

/// The `WHERE` of one table for `query`, with its values.
fn condition(query: &Query, actions: Actions) -> (String, Vec<Value>) {
    let mut clauses = vec!["1 = 1".to_owned()];
    let mut values = Vec::new();
    if let Some(world) = &query.world {
        clauses.push("t.world = (SELECT id FROM world WHERE name = ?)".to_owned());
        values.push(Value::Text(world.clone()));
    }
    if let Some(((x, y, z), radius)) = query.around {
        clauses
            .push("t.x BETWEEN ? AND ? AND t.z BETWEEN ? AND ? AND t.y BETWEEN ? AND ?".to_owned());
        for middle in [x, z, y] {
            values.push(Value::Integer(i64::from(middle) - i64::from(radius)));
            values.push(Value::Integer(i64::from(middle) + i64::from(radius)));
        }
    }
    if !query.actors.is_empty() {
        clauses.push(format!(
            "t.actor IN (SELECT id FROM actor WHERE name IN ({}))",
            marks(query.actors.len())
        ));
        values.extend(query.actors.iter().cloned().map(Value::Text));
    }
    if let Some(since) = query.since {
        clauses.push("t.time >= ?".to_owned());
        values.push(Value::Integer(since));
    }
    if !query.kinds.is_empty() {
        clauses.push(format!(
            "t.kind IN (SELECT id FROM kind WHERE name IN ({}))",
            marks(query.kinds.len())
        ));
        values.extend(query.kinds.iter().cloned().map(Value::Text));
    }
    if let Actions::Only(action) = actions {
        clauses.push("t.action = ?".to_owned());
        values.push(Value::Integer(i64::from(action)));
    }
    (clauses.join(" AND "), values)
}

fn marks(count: usize) -> String {
    vec!["?"; count].join(", ")
}

fn lookup(connection: &Connection, query: &Query) -> rusqlite::Result<Page> {
    // Nothing chosen means both tables.
    let (blocks, containers) = match (query.blocks, query.containers) {
        (None, None) => (Some(Actions::Any), Some(Actions::Any)),
        chosen => chosen,
    };
    let mut selects = Vec::new();
    let mut values = Vec::new();
    let mut total = 0;
    let tables = [
        (blocks, "block", "0, t.state, NULL, NULL"),
        (containers, "container", "1, '', t.amount, t.data"),
    ];
    for (actions, table, columns) in tables {
        let Some(actions) = actions else {
            continue;
        };
        let (condition, condition_values) = condition(query, actions);
        total += connection.query_row(
            &format!("SELECT count(*) FROM {table} t WHERE {condition}"),
            params_from_iter(condition_values.iter()),
            |row| Ok(row.get::<_, i64>(0)?.unsigned_abs()),
        )?;
        selects.push(format!(
            "SELECT t.time, t.actor, t.world, t.x, t.y, t.z, t.kind, t.action, t.rolled_back,
                    t.id, {columns} FROM {table} t WHERE {condition}"
        ));
        values.extend(condition_values);
    }
    values.push(Value::Integer(i64::from(query.limit)));
    values.push(Value::Integer(i64::from(query.offset)));
    let sql = format!(
        "SELECT e.*, a.name, a.uuid, w.name, k.name FROM ({} ORDER BY 1 DESC, 10 DESC LIMIT ? OFFSET ?) e
         JOIN actor a ON a.id = e.actor JOIN world w ON w.id = e.world JOIN kind k ON k.id = e.kind
         ORDER BY 1 DESC, 10 DESC",
        selects.join(" UNION ALL ")
    );
    let rows = connection
        .prepare(&sql)?
        .query_map(params_from_iter(values.iter()), |row| {
            let items: bool = row.get(10)?;
            Ok(Event {
                time: row.get(0)?,
                pos: (row.get(3)?, row.get(4)?, row.get(5)?),
                action: row.get(7)?,
                rolled_back: row.get(8)?,
                subject: if items {
                    Subject::Items {
                        amount: row.get(12)?,
                        data: row.get(13)?,
                    }
                } else {
                    Subject::Block {
                        state: row.get(11)?,
                    }
                },
                actor: Actor {
                    name: row.get(14)?,
                    uuid: row.get(15)?,
                },
                world: row.get(16)?,
                kind: row.get(17)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Page { rows, total })
}

#[cfg(test)]
mod tests {
    use std::{env, process, sync::mpsc};

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let folder = env::temp_dir().join(format!(
            "minehouse-blocklog-{name}-{}-{}",
            process::id(),
            now_ms()
        ));
        folder.join("blocklog.db")
    }

    fn block(time: i64, who: &str, pos: (i32, i32, i32), kind: &str, action: u8) -> Event {
        Event {
            time,
            actor: Actor {
                name: who.to_owned(),
                uuid: Some(format!("uuid-of-{}", who.to_lowercase())),
            },
            world: "minecraft:overworld".to_owned(),
            pos,
            kind: kind.to_owned(),
            action,
            rolled_back: false,
            subject: Subject::Block {
                state: String::new(),
            },
        }
    }

    fn items(
        time: i64,
        who: &str,
        pos: (i32, i32, i32),
        kind: &str,
        amount: i32,
        action: u8,
    ) -> Event {
        Event {
            subject: Subject::Items {
                amount,
                data: Some(vec![1, 2, 3]),
            },
            ..block(time, who, pos, kind, action)
        }
    }

    fn ask(store: &Store, query: Query) -> Page {
        let (answer, wait) = mpsc::channel();
        store.lookup(query, move |page| {
            let _ = answer.send(page);
        });
        wait.recv()
            .expect("the reader answers")
            .expect("the lookup runs")
    }

    fn all() -> Query {
        Query {
            limit: 100,
            ..Query::default()
        }
    }

    #[test]
    fn events_come_back_newest_first_with_everything_they_had() {
        let file = scratch("roundtrip");
        let store = Store::open(&file, None).expect("the log opens");
        let placed = Event {
            subject: Subject::Block {
                state: "facing=north,type=single".to_owned(),
            },
            ..block(1_000, "Steve", (1, 64, 1), "minecraft:chest", PLACE)
        };
        let put = items(2_000, "Alex", (1, 64, 1), "minecraft:diamond", 5, PUT);
        let broken = block(3_000, "steve", (1, 64, 1), "minecraft:chest", BREAK);
        store.record(placed.clone());
        store.record(put.clone());
        store.record(broken.clone());

        let page = ask(&store, all());
        assert_eq!(page.total, 3);
        // One actor line for both spellings, named as first seen.
        let broken = Event {
            actor: placed.actor.clone(),
            ..broken
        };
        assert_eq!(page.rows, [broken, put, placed]);
        store.stop();
    }

    #[test]
    fn lookups_narrow_by_place_actor_time_kind_and_action() {
        let file = scratch("filters");
        let store = Store::open(&file, None).expect("the log opens");
        store.record(block(1_000, "Steve", (0, 64, 0), "minecraft:stone", BREAK));
        store.record(block(2_000, "Steve", (5, 64, 0), "minecraft:dirt", PLACE));
        store.record(block(3_000, "Alex", (50, 64, 0), "minecraft:stone", PLACE));
        store.record(items(4_000, "Alex", (5, 64, 0), "minecraft:stone", 2, TAKE));
        store.record(Event {
            world: "minecraft:the_nether".to_owned(),
            ..block(5_000, "Steve", (0, 64, 0), "minecraft:netherrack", BREAK)
        });

        let total = |query: Query| ask(&store, query).total;
        assert_eq!(total(all()), 5);
        assert_eq!(
            total(Query {
                world: Some("minecraft:overworld".to_owned()),
                around: Some(((0, 64, 0), 10)),
                ..all()
            }),
            3
        );
        assert_eq!(
            total(Query {
                world: Some("minecraft:overworld".to_owned()),
                around: Some(((5, 64, 0), 0)),
                ..all()
            }),
            2
        );
        assert_eq!(
            total(Query {
                actors: vec!["alex".to_owned()],
                ..all()
            }),
            2
        );
        assert_eq!(
            total(Query {
                since: Some(3_000),
                ..all()
            }),
            3
        );
        assert_eq!(
            total(Query {
                kinds: vec!["minecraft:stone".to_owned()],
                ..all()
            }),
            3
        );
        assert_eq!(
            total(Query {
                blocks: Some(Actions::Any),
                ..all()
            }),
            4
        );
        assert_eq!(
            total(Query {
                blocks: Some(Actions::Only(PLACE)),
                ..all()
            }),
            2
        );
        assert_eq!(
            total(Query {
                containers: Some(Actions::Only(TAKE)),
                ..all()
            }),
            1
        );
        assert_eq!(
            total(Query {
                containers: Some(Actions::Only(PUT)),
                ..all()
            }),
            0
        );

        let second = ask(
            &store,
            Query {
                offset: 1,
                limit: 2,
                ..Query::default()
            },
        );
        assert_eq!(second.total, 5);
        assert_eq!(
            second
                .rows
                .iter()
                .map(|event| event.time)
                .collect::<Vec<_>>(),
            [4_000, 3_000]
        );
        store.stop();
    }

    #[test]
    fn the_log_survives_a_restart_and_purges_old_rows() {
        let file = scratch("purge");
        let store = Store::open(&file, None).expect("the log opens");
        store.record(block(1_000, "Steve", (0, 64, 0), "minecraft:stone", BREAK));
        store.record(items(1_500, "Steve", (0, 64, 0), "minecraft:stone", 1, PUT));
        store.record(block(9_000, "Steve", (0, 64, 0), "minecraft:stone", PLACE));
        store.stop();

        let store = Store::open(&file, None).expect("the log opens again");
        assert_eq!(ask(&store, all()).total, 3);
        let (answer, wait) = mpsc::channel();
        store.purge(5_000, move |rows| {
            let _ = answer.send(rows);
        });
        assert_eq!(wait.recv().expect("the writer answers"), Ok(2));
        let page = ask(&store, all());
        assert_eq!(page.total, 1);
        assert_eq!(page.rows[0].time, 9_000);

        let (answer, wait) = mpsc::channel();
        store.stats(move |stats| {
            let _ = answer.send(stats);
        });
        let stats = wait
            .recv()
            .expect("the reader answers")
            .expect("the count runs");
        assert_eq!(
            (stats.blocks, stats.containers, stats.oldest),
            (1, 0, Some(9_000))
        );
        store.stop();
    }

    #[test]
    fn an_import_remembers_how_far_it_got() {
        let file = scratch("import");
        let store = Store::open(&file, None).expect("the log opens");
        let imported = |table: &'static str| {
            let (answer, wait) = mpsc::channel();
            store.imported(table, move |last| {
                let _ = answer.send(last);
            });
            wait.recv()
                .expect("the reader answers")
                .expect("the meta reads")
        };
        assert_eq!(imported("block"), 0);
        store.import(
            vec![block(1_000, "#tnt", (0, 64, 0), "minecraft:stone", BREAK)],
            "block",
            41,
        );
        // An older batch arriving late doesn't move the mark back.
        store.import(Vec::new(), "block", 7);
        assert_eq!(imported("block"), 41);
        assert_eq!(imported("container"), 0);
        assert_eq!(ask(&store, all()).rows[0].actor.name, "#tnt");
        store.stop();
    }
}
