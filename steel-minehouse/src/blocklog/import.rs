//! Brings the history of `CoreProtect` over, from the CSV files its database exports (see the
//! README). Runs on its own thread and can be run again: it goes on from where it stopped.

use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader},
    mem,
    path::Path,
    str::FromStr,
    sync::mpsc,
};

use rustc_hash::FxHashMap;

use super::{
    params::kind,
    store::{Actor, CLICK, Event, Store, Subject},
};

/// Rows handed to the writer at once.
const BATCH: usize = 20_000;

/// What an import did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    /// Block rows brought over.
    pub blocks: u64,
    /// Container rows brought over.
    pub containers: u64,
    /// Rows that could not be read.
    pub skipped: u64,
}

/// Reads CSV records: commas, `"quoted, fields"` with `""` for a quote, line breaks inside
/// quotes.
struct Records<R> {
    input: R,
    line: String,
}

impl<R: BufRead> Records<R> {
    const fn new(input: R) -> Self {
        Self {
            input,
            line: String::new(),
        }
    }
}

impl<R: BufRead> Iterator for Records<R> {
    type Item = Vec<String>;

    fn next(&mut self) -> Option<Vec<String>> {
        let mut fields = Vec::new();
        let mut field = String::new();
        let mut quoted = false;
        let mut started = false;
        loop {
            self.line.clear();
            if self.input.read_line(&mut self.line).ok()? == 0 {
                // The end of the file closes the last record, if there is one.
                return started.then(|| {
                    fields.push(field);
                    fields
                });
            }
            started = true;
            let mut characters = self.line.chars().peekable();
            while let Some(character) = characters.next() {
                match character {
                    '"' if quoted && characters.peek() == Some(&'"') => {
                        characters.next();
                        field.push('"');
                    }
                    '"' => quoted = !quoted,
                    ',' if !quoted => fields.push(mem::take(&mut field)),
                    '\r' | '\n' if !quoted => {}
                    other => field.push(other),
                }
            }
            if !quoted {
                fields.push(field);
                return Some(fields);
            }
        }
    }
}

/// A CSV file with a header: rows as maps from column to value.
struct Table {
    records: Records<BufReader<File>>,
    columns: FxHashMap<String, usize>,
}

impl Table {
    fn open(file: &Path) -> Result<Self, String> {
        let input = File::open(file).map_err(|error| format!("{}: {error}", file.display()))?;
        let mut records = Records::new(BufReader::new(input));
        let header = records
            .next()
            .ok_or_else(|| format!("{}: пустой файл", file.display()))?;
        let columns = header
            .into_iter()
            .enumerate()
            .map(|(index, name)| (name.trim().to_ascii_lowercase(), index))
            .collect();
        Ok(Self { records, columns })
    }

    fn need(&self, file: &Path, names: &[&str]) -> Result<(), String> {
        match names.iter().find(|name| !self.columns.contains_key(**name)) {
            Some(missing) => Err(format!("{}: нет колонки {missing}", file.display())),
            None => Ok(()),
        }
    }
}

struct Row<'a> {
    fields: &'a [String],
    columns: &'a FxHashMap<String, usize>,
}

impl Row<'_> {
    fn text(&self, column: &str) -> &str {
        self.columns
            .get(column)
            .and_then(|index| self.fields.get(*index))
            .map_or("", String::as_str)
    }

    fn number<T: FromStr>(&self, column: &str) -> Option<T> {
        self.text(column).trim().parse().ok()
    }
}

/// `CoreProtect` keeps seconds; we keep milliseconds.
const fn time_ms(time: i64) -> i64 {
    if time < 100_000_000_000 {
        time * 1000
    } else {
        time
    }
}

/// The properties of a block state out of what `CoreProtect` kept. Its `DuckDB` storage
/// writes words of `key=value`, `type=minecraft:chest data=facing=west,waterlogged=false`:
/// the properties are the `data` word. Older forms: `facing=north,lit=true`, a whole
/// `minecraft:chest[facing=north]`, or ids into its data map.
fn state(blockdata: &str, data_map: &FxHashMap<String, String>) -> String {
    let blockdata = blockdata.trim();
    if blockdata.is_empty() {
        return String::new();
    }
    if blockdata.starts_with("type=") {
        return blockdata
            .split(' ')
            .find_map(|word| word.strip_prefix("data="))
            .unwrap_or_default()
            .to_owned();
    }
    let resolved = if blockdata
        .split(',')
        .all(|part| data_map.contains_key(part.trim()))
    {
        blockdata
            .split(',')
            .filter_map(|part| data_map.get(part.trim()).map(String::as_str))
            .collect::<Vec<_>>()
            .join(",")
    } else {
        blockdata.to_owned()
    };
    match (resolved.find('['), resolved.rfind(']')) {
        (Some(open), Some(close)) if open < close => resolved[open + 1..close].to_owned(),
        _ if resolved.contains('=') => resolved,
        _ => String::new(),
    }
}

fn common(row: &Row<'_>, worlds: &BTreeMap<String, String>) -> Option<(Event, i64)> {
    let id = row.number("id")?;
    let world = row.text("world");
    let uuid = row.text("uuid").trim();
    let name = row.text("user").trim();
    if name.is_empty() || world.is_empty() {
        return None;
    }
    let event = Event {
        time: time_ms(row.number("time")?),
        actor: Actor {
            name: name.to_owned(),
            uuid: (!uuid.is_empty()).then(|| uuid.to_owned()),
        },
        world: worlds
            .get(world)
            .cloned()
            .unwrap_or_else(|| world.to_owned()),
        pos: (row.number("x")?, row.number("y")?, row.number("z")?),
        kind: kind(row.text("type").trim()),
        action: row.number("action")?,
        rolled_back: row.number::<i64>("rolled_back").unwrap_or(0) & 1 == 1,
        subject: Subject::Block {
            state: String::new(),
        },
    };
    Some((event, id))
}

fn data_map(folder: &Path) -> FxHashMap<String, String> {
    let Ok(table) = Table::open(&folder.join("data_map.csv")) else {
        return FxHashMap::default();
    };
    let columns = table.columns;
    table
        .records
        .map(|fields| {
            let row = Row {
                fields: &fields,
                columns: &columns,
            };
            (
                row.text("id").trim().to_owned(),
                row.text("data").to_owned(),
            )
        })
        .collect()
}

fn resume_point(store: &Store, table: &'static str) -> Result<i64, String> {
    let (answer, wait) = mpsc::channel();
    store.imported(table, move |last| {
        let _ = answer.send(last);
    });
    wait.recv().map_err(|error| error.to_string())?
}

fn import_table(
    store: &Store,
    file: &Path,
    table: &'static str,
    worlds: &BTreeMap<String, String>,
    skipped: &mut u64,
    mut subject: impl FnMut(&Row<'_>) -> Option<Subject>,
) -> Result<u64, String> {
    let Table { records, columns } = {
        let opened = Table::open(file)?;
        opened.need(
            file,
            &[
                "id", "time", "user", "world", "x", "y", "z", "type", "action",
            ],
        )?;
        opened
    };
    let resume = resume_point(store, table)?;
    let mut imported = 0;
    let mut batch = Vec::with_capacity(BATCH);
    let mut last_id = resume;
    for fields in records {
        let row = Row {
            fields: &fields,
            columns: &columns,
        };
        let Some((mut event, id)) = common(&row, worlds) else {
            *skipped += 1;
            continue;
        };
        if id <= resume {
            continue;
        }
        let Some(subject) = subject(&row) else {
            *skipped += 1;
            continue;
        };
        event.subject = subject;
        last_id = last_id.max(id);
        batch.push(event);
        if batch.len() == BATCH {
            imported += batch.len() as u64;
            store.import(
                mem::replace(&mut batch, Vec::with_capacity(BATCH)),
                table,
                last_id,
            );
            // Wait for the disk, or a big file would pile up in memory.
            store.flush();
            if imported % 1_000_000 == 0 {
                log::info!("Block log import: {imported} rows of {table}");
            }
        }
    }
    imported += batch.len() as u64;
    store.import(batch, table, last_id);
    store.flush();
    Ok(imported)
}

/// Imports `blocks.csv` and `containers.csv` (and `data_map.csv`, if there) of `folder`.
/// Blocking: call from a thread of its own.
pub fn run(
    store: &Store,
    folder: &Path,
    worlds: &BTreeMap<String, String>,
) -> Result<Report, String> {
    let mut report = Report::default();
    let data_map = data_map(folder);
    let blocks = folder.join("blocks.csv");
    let containers = folder.join("containers.csv");
    if !blocks.exists() && !containers.exists() {
        return Err(format!(
            "В {} нет blocks.csv и containers.csv.",
            folder.display()
        ));
    }
    if blocks.exists() {
        report.blocks = import_table(
            store,
            &blocks,
            "block",
            worlds,
            &mut report.skipped,
            |row| {
                // `CoreProtect` keeps killed mobs and other entity events in the same table
                // (actions 3 and up): they are not block changes.
                let action: u8 = row.number("action")?;
                (action <= CLICK).then(|| Subject::Block {
                    state: state(row.text("blockdata"), &data_map),
                })
            },
        )?;
    }
    if containers.exists() {
        report.containers = import_table(
            store,
            &containers,
            "container",
            worlds,
            &mut report.skipped,
            |row| {
                Some(Subject::Items {
                    amount: row.number("amount")?,
                    data: None,
                })
            },
        )?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn csv_records_keep_quotes_commas_and_line_breaks() {
        let input =
            "id,name,note\r\n1,\"Smith, John\",\"said \"\"hi\"\"\"\n2,plain,\"two\nlines\"\n3,,";
        let records: Vec<_> = Records::new(Cursor::new(input)).collect();
        assert_eq!(records.len(), 4);
        assert_eq!(records[1], ["1", "Smith, John", "said \"hi\""]);
        assert_eq!(records[2], ["2", "plain", "two\nlines"]);
        assert_eq!(records[3], ["3", "", ""]);
    }

    #[test]
    fn block_data_becomes_state_properties() {
        let mut map = FxHashMap::default();
        map.insert("4".to_owned(), "facing=north".to_owned());
        map.insert("9".to_owned(), "lit=true".to_owned());
        assert_eq!(state("", &map), "");
        assert_eq!(state("4,9", &map), "facing=north,lit=true");
        assert_eq!(
            state("minecraft:chest[facing=east,type=single]", &map),
            "facing=east,type=single"
        );
        assert_eq!(state("waterlogged=true", &map), "waterlogged=true");
        // Ids without their map say nothing.
        assert_eq!(state("17", &map), "");
        // The words of the DuckDB storage.
        assert_eq!(
            state(
                "type=minecraft:chest data=facing=west,type=left,waterlogged=false",
                &map
            ),
            "facing=west,type=left,waterlogged=false"
        );
        assert_eq!(
            state("type=minecraft:water data=level=0 fluid_level=8", &map),
            "level=0"
        );
        assert_eq!(state("type=minecraft:stone", &map), "");
        assert_eq!(
            state("type=minecraft:evoker uuid=7cc0 health=0.0", &map),
            ""
        );
    }

    #[test]
    fn seconds_become_milliseconds_once() {
        assert_eq!(time_ms(1_790_000_000), 1_790_000_000_000);
        assert_eq!(time_ms(1_790_000_000_000), 1_790_000_000_000);
    }
}

#[cfg(test)]
mod file_tests {
    use std::{env, fs, process};

    use super::*;
    use crate::blocklog::store::{BREAK, PUT, Query};

    #[test]
    fn an_export_imports_once_and_goes_on_where_it_stopped() {
        let folder = env::temp_dir().join(format!(
            "minehouse-blocklog-export-{}-{}",
            process::id(),
            chrono::Utc::now().timestamp_millis()
        ));
        fs::create_dir_all(&folder).expect("a scratch folder");
        fs::write(folder.join("data_map.csv"), "id,data\n3,facing=north\n").expect("data_map.csv");
        fs::write(
            folder.join("blocks.csv"),
            "id,time,user,uuid,world,x,y,z,type,blockdata,action,rolled_back\n\
             1,1790000000,Steve,0f1e,world,1,64,-2,minecraft:furnace,3,1,0\n\
             2,1790000050,#tnt,,world,1,64,-2,furnace,,0,1\n\
             4,1790000060,Steve,0f1e,world,1,64,-2,minecraft:zombie,type=minecraft:zombie,3,0\n\
             3,broken row\n",
        )
        .expect("blocks.csv");
        fs::write(
            folder.join("containers.csv"),
            "id,time,user,uuid,world,x,y,z,type,amount,itemdata,action,rolled_back\n\
             8,1790000020,Steve,0f1e,world,1,64,-2,minecraft:coal,12,,1,0\n",
        )
        .expect("containers.csv");
        let worlds = BTreeMap::from([("world".to_owned(), "minecraft:overworld".to_owned())]);
        let store =
            Store::open(&folder.join("log").join("blocklog.db"), None).expect("the log opens");

        let report = run(&store, &folder, &worlds).expect("the import runs");
        assert_eq!(
            report,
            Report {
                blocks: 2,
                containers: 1,
                skipped: 2
            }
        );
        // Nothing twice.
        let again = run(&store, &folder, &worlds).expect("the import runs again");
        assert_eq!((again.blocks, again.containers), (0, 0));

        let (answer, wait) = mpsc::channel();
        store.lookup(
            Query {
                world: Some("minecraft:overworld".to_owned()),
                around: Some(((1, 64, -2), 0)),
                limit: 10,
                ..Query::default()
            },
            move |page| {
                let _ = answer.send(page);
            },
        );
        let page = wait
            .recv()
            .expect("the reader answers")
            .expect("the lookup runs");
        assert_eq!(page.total, 3);
        let [exploded, coal, placed] = page.rows.as_slice() else {
            panic!("three rows, got {:?}", page.rows);
        };
        assert_eq!(
            (exploded.actor.name.as_str(), exploded.action),
            ("#tnt", BREAK)
        );
        assert!(exploded.rolled_back);
        assert_eq!(exploded.kind, "minecraft:furnace");
        assert_eq!(coal.action, PUT);
        assert_eq!(
            coal.subject,
            Subject::Items {
                amount: 12,
                data: None
            }
        );
        assert_eq!(placed.time, 1_790_000_000_000);
        assert_eq!(placed.actor.uuid.as_deref(), Some("0f1e"));
        assert_eq!(
            placed.subject,
            Subject::Block {
                state: "facing=north".to_owned()
            }
        );
        store.stop();
    }
}
