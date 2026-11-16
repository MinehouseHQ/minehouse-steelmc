//! The shows on disk (the Fabric `Library`): `config/minehouse/events/shows/<id>.json`, read
//! at start and on `/show reload`. Where each show is staged (`/show stage`) is kept apart
//! in the data directory, so a script can be replaced without losing its place.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde_json::Value;

use super::{
    json::Object,
    script::{ShowScript, Stage},
};
use crate::config;

/// Every show that loaded, and why the others did not.
pub struct Library {
    shows_dir: PathBuf,
    stages_file: PathBuf,
    stages: Object,
    shows: BTreeMap<String, Arc<ShowScript>>,
    errors: BTreeMap<String, String>,
}

impl Library {
    /// The library of this server; nothing is read until [`Library::reload`].
    #[must_use]
    pub fn new() -> Self {
        Self::at(
            Path::new(config::CONFIG_DIR).join("events").join("shows"),
            config::data_path("events/stages.json"),
        )
    }

    fn at(shows_dir: PathBuf, stages_file: PathBuf) -> Self {
        Self {
            shows_dir,
            stages_file,
            stages: Object::new(),
            shows: BTreeMap::new(),
            errors: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn show(&self, id: &str) -> Option<&Arc<ShowScript>> {
        self.shows.get(id)
    }

    #[must_use]
    pub const fn shows(&self) -> &BTreeMap<String, Arc<ShowScript>> {
        &self.shows
    }

    /// The shows that did not load, with the reason.
    #[must_use]
    pub const fn errors(&self) -> &BTreeMap<String, String> {
        &self.errors
    }

    /// Where the scripts are read from.
    #[must_use]
    pub fn shows_dir(&self) -> &Path {
        &self.shows_dir
    }

    /// Reads every show again. Returns how many loaded; the rest are in [`Library::errors`].
    pub fn reload(&mut self) -> usize {
        self.shows.clear();
        self.errors.clear();
        self.stages = match read_object(&self.stages_file) {
            Ok(stages) => stages,
            Err(error) => {
                self.errors.insert("(сцены)".to_owned(), error);
                Object::new()
            }
        };
        if let Err(error) = fs::create_dir_all(&self.shows_dir) {
            log::debug!(
                "MineHouse events: could not create {}: {error}",
                self.shows_dir.display()
            );
        }
        let mut files: Vec<PathBuf> = match fs::read_dir(&self.shows_dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "json")
                })
                .collect(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                self.errors.insert("(папка)".to_owned(), error.to_string());
                Vec::new()
            }
        };
        files.sort();
        for file in files {
            let Some(id) = file.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let loaded = fs::read_to_string(&file)
                .map_err(|error| error.to_string())
                .and_then(|text| ShowScript::parse(id, &text, self.stage_override(id)));
            match loaded {
                Ok(show) => {
                    self.shows.insert(id.to_owned(), Arc::new(show));
                }
                Err(error) => {
                    log::warn!("MineHouse events: show {id} did not load: {error}");
                    self.errors.insert(id.to_owned(), error);
                }
            }
        }
        self.shows.len()
    }

    fn stage_override(&self, id: &str) -> Option<Stage> {
        match self.stages.get(id) {
            Some(Value::Object(stage)) => Stage::from_json(stage).ok(),
            _ => None,
        }
    }

    /// Puts the show on `stage` and reads it again, since a show may turn with its stage.
    pub fn set_stage(&mut self, id: &str, stage: &Stage) -> Result<(), String> {
        self.stages.insert(id.to_owned(), stage.to_json());
        if let Some(parent) = self.stages_file.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let text = serde_json::to_string_pretty(&self.stages).map_err(|error| error.to_string())?;
        fs::write(&self.stages_file, text).map_err(|error| error.to_string())?;
        self.reload();
        Ok(())
    }
}

impl Default for Library {
    fn default() -> Self {
        Self::new()
    }
}

fn read_object(file: &Path) -> Result<Object, String> {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Object::new()),
        Err(error) => return Err(format!("{}: {error}", file.display())),
    };
    match serde_json::from_str(&text) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(_) => Err(format!("{}: ожидаю объект", file.display())),
        Err(error) => Err(format!("{}: {error}", file.display())),
    }
}

#[cfg(test)]
mod tests {
    use std::{env, process};

    use glam::DVec3;

    use super::*;

    const SHOW: &str = r#"{
        "title": "Проба",
        "stageFacing": true,
        "actors": {"cam": {"type": "camera"}},
        "cues": [{"at": 0, "do": "cut", "pos": [0, 5, 10]}]
    }"#;

    #[test]
    fn a_show_is_read_and_staged() {
        let dir = env::temp_dir().join(format!("minehouse-events-library-{}", process::id()));
        let shows = dir.join("shows");
        fs::create_dir_all(&shows).expect("the shows directory");
        fs::write(shows.join("probe.json"), SHOW).expect("the show file");
        fs::write(shows.join("broken.json"), "{").expect("the broken file");

        let mut library = Library::at(shows, dir.join("data/stages.json"));
        assert_eq!(library.reload(), 1);
        assert!(library.errors().contains_key("broken"));
        assert_eq!(
            library.show("probe").and_then(|show| show.stage.clone()),
            None
        );

        let stage = Stage {
            world: "minecraft:overworld".to_owned(),
            pos: DVec3::new(10.5, 64.0, -3.5),
            yaw: 90,
        };
        library
            .set_stage("probe", &stage)
            .expect("the stage is saved");
        assert_eq!(
            library.show("probe").and_then(|show| show.stage.clone()),
            Some(stage)
        );
        fs::remove_dir_all(&dir).expect("the directory is removed");
    }
}
