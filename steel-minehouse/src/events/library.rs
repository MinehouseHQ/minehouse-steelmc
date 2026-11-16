//! The shows on disk (the Fabric `Library`): `config/minehouse/events/shows/<id>.json`, read
//! at start and on `/show reload`. Where each show is staged (`/show stage`) is kept apart
//! in the data directory, so a script can be replaced without losing its place.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use rustc_hash::FxHashMap;
use serde_json::Value;

use super::{
    camera_path::CameraPath,
    json::Object,
    script::{ShowScript, Stage, check_name},
};
use crate::config;

/// Every show that loaded, and why the others did not.
pub struct Library {
    shows_dir: PathBuf,
    /// What the shows keep in files of their own: `<show>/paths/<name>.json`.
    assets_dir: PathBuf,
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
        let assets_dir = shows_dir
            .parent()
            .map_or_else(|| PathBuf::from("assets"), |root| root.join("assets"));
        Self {
            shows_dir,
            assets_dir,
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

/// Camera paths recorded with `/show cam`, in stage coordinates.
impl Library {
    /// The stage of a show, even one whose script doesn't load yet (so its paths can be
    /// recorded).
    #[must_use]
    pub fn stage_of(&self, id: &str) -> Option<Stage> {
        self.stage_override(id)
            .or_else(|| self.shows.get(id).and_then(|show| show.stage.clone()))
    }

    fn path_file(&self, show: &str, name: &str) -> Result<PathBuf, String> {
        check_name(show)?;
        check_name(name)?;
        Ok(self
            .assets_dir
            .join(show)
            .join("paths")
            .join(format!("{name}.json")))
    }

    /// The recorded path `name` of `show`, or `None` if nothing is recorded under that name.
    pub fn path_or_none(&self, show: &str, name: &str) -> Result<Option<CameraPath>, String> {
        let file = self.path_file(show, name)?;
        let text = match fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("путь \"{name}\" не читается: {error}")),
        };
        match serde_json::from_str(&text) {
            Ok(Value::Object(object)) => CameraPath::from_json(&object)
                .map(Some)
                .map_err(|error| format!("путь \"{name}\": {error}")),
            Ok(_) => Err(format!("путь \"{name}\": ожидаю объект")),
            Err(error) => Err(format!("путь \"{name}\" не читается: {error}")),
        }
    }

    /// The recorded path `name` of `show`.
    pub fn path(&self, show: &str, name: &str) -> Result<CameraPath, String> {
        self.path_or_none(show, name)?.ok_or_else(|| {
            format!("нет пути камеры \"{name}\" (запишите: /show cam add {show} {name})")
        })
    }

    /// Every recorded path `script` flies, or why the show can't start.
    pub fn paths_of(&self, script: &ShowScript) -> Result<FxHashMap<String, CameraPath>, String> {
        let mut paths = FxHashMap::default();
        for name in script.recorded_paths() {
            if !paths.contains_key(name) {
                paths.insert(name.to_owned(), self.path(&script.id, name)?);
            }
        }
        Ok(paths)
    }

    /// Keeps `path` under `name`; the old file stays whole if the write fails halfway.
    pub fn save_path(&self, show: &str, name: &str, path: &CameraPath) -> Result<(), String> {
        let file = self.path_file(show, name)?;
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let text =
            serde_json::to_string_pretty(&path.to_json()).map_err(|error| error.to_string())?;
        let partial = file.with_extension("json.tmp");
        fs::write(&partial, text).map_err(|error| error.to_string())?;
        fs::rename(&partial, &file).map_err(|error| error.to_string())
    }

    /// Forgets the path; one that was never recorded is no error.
    pub fn delete_path(&self, show: &str, name: &str) -> Result<(), String> {
        match fs::remove_file(self.path_file(show, name)?) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
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
    use crate::events::camera_path::Point;

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

    #[test]
    fn camera_paths_are_recorded_beside_the_shows() {
        let dir = env::temp_dir().join(format!("minehouse-events-paths-{}", process::id()));
        let shows = dir.join("shows");
        fs::create_dir_all(&shows).expect("the shows directory");
        fs::write(
            shows.join("probe.json"),
            r#"{"actors": {"cam": {"type": "camera"}},
                "cues": [{"at": 0, "do": "path", "path": "fly", "over": "2s"}]}"#,
        )
        .expect("the show file");
        let mut library = Library::at(shows, dir.join("data/stages.json"));
        assert_eq!(library.reload(), 1);
        let script = library.show("probe").cloned().expect("the show");

        // Nothing recorded yet: the show says what to do instead of starting.
        assert_eq!(library.path_or_none("probe", "fly"), Ok(None));
        let error = library.paths_of(&script).expect_err("no path yet");
        assert!(error.contains("/show cam add probe fly"), "{error}");

        let point = |x: f64, yaw: f32| Point {
            pos: DVec3::new(x, 5.25, -1.0),
            yaw,
            pitch: 12.5,
        };
        let path = CameraPath::new(vec![point(0.0, 0.0), point(4.0, 90.0)], None).expect("a path");
        library.save_path("probe", "fly", &path).expect("saved");
        assert!(dir.join("assets/probe/paths/fly.json").is_file());
        assert_eq!(library.path("probe", "fly"), Ok(path.clone()));
        assert_eq!(library.paths_of(&script).map(|paths| paths.len()), Ok(1));

        // Names become file names, so nothing can climb out of the folder.
        assert!(library.save_path("probe", "../fly", &path).is_err());
        assert!(library.path_or_none("..", "fly").is_err());

        library.delete_path("probe", "fly").expect("deleted");
        library
            .delete_path("probe", "fly")
            .expect("deleting nothing is fine");
        assert_eq!(library.path_or_none("probe", "fly"), Ok(None));
        fs::remove_dir_all(&dir).expect("the directory is removed");
    }
}
