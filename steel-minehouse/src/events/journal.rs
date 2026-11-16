//! What a show changed (the Fabric `Journal`), kept on disk so nothing is lost if the
//! server dies mid-show: where each viewer stood and in what game mode, and what the blocks
//! it replaced for a while were. A viewer stays here until they are put back, even if they
//! only come online days later; a block stays until its chunk is loaded.

use std::{collections::BTreeMap, fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A viewer before the show.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerState {
    pub world: String,
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    /// Vanilla `GameType`, by id.
    pub game_mode: i32,
    pub invulnerable: bool,
    /// Put them back on this spot afterwards, not just give back their game mode.
    pub return_back: bool,
}

/// A block a show replaced for a while, as it was before.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedBlock {
    pub world: String,
    /// The block position.
    pub p: [i32; 3],
    /// The state, as `minecraft:stone` or `minecraft:oak_stairs[facing=north]`.
    pub s: String,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_show: Option<String>,
    #[serde(default)]
    viewers: BTreeMap<Uuid, ViewerState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    temporary: Vec<SavedBlock>,
}

/// The journal of the running show and of everyone not put back yet.
pub struct Journal {
    file: PathBuf,
    saved: Saved,
    dirty: bool,
}

impl Journal {
    /// Reads the journal; a file that can't be read is logged and starts empty.
    #[must_use]
    pub fn load(file: PathBuf) -> Self {
        let saved = match fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
                log::error!(
                    "MineHouse events: could not read the show journal {}, viewers of an \
                     unfinished show will not be put back: {error}",
                    file.display()
                );
                Saved::default()
            }),
            Err(_) => Saved::default(),
        };
        Self {
            file,
            saved,
            dirty: false,
        }
    }

    /// The show that was running, if the server stopped in the middle of it.
    #[must_use]
    pub fn active_show(&self) -> Option<&str> {
        self.saved.active_show.as_deref()
    }

    pub fn set_active_show(&mut self, show: Option<&str>) {
        self.saved.active_show = show.map(str::to_owned);
        self.dirty = true;
    }

    #[must_use]
    pub fn viewer(&self, id: Uuid) -> Option<&ViewerState> {
        self.saved.viewers.get(&id)
    }

    #[must_use]
    pub fn viewers(&self) -> usize {
        self.saved.viewers.len()
    }

    /// Remembers a viewer; the state saved first stays (they left and came back mid-show).
    pub fn save_viewer(&mut self, id: Uuid, state: ViewerState) {
        self.saved.viewers.entry(id).or_insert(state);
        self.dirty = true;
    }

    pub fn take_viewer(&mut self, id: Uuid) -> Option<ViewerState> {
        let state = self.saved.viewers.remove(&id);
        self.dirty |= state.is_some();
        state
    }

    /// Remembers what a block was the first time a show touches it.
    pub fn save_block(&mut self, world: &str, pos: [i32; 3], state: impl FnOnce() -> String) {
        let known = self
            .saved
            .temporary
            .iter()
            .any(|block| block.p == pos && block.world == world);
        if !known {
            self.saved.temporary.push(SavedBlock {
                world: world.to_owned(),
                p: pos,
                s: state(),
            });
            self.dirty = true;
        }
    }

    #[must_use]
    pub const fn has_blocks(&self) -> bool {
        !self.saved.temporary.is_empty()
    }

    /// Puts the saved blocks back with `restore`, which says whether the block is back (or
    /// can never be); the rest stay for the next try. Returns how many are left.
    pub fn restore_blocks(&mut self, mut restore: impl FnMut(&SavedBlock) -> bool) -> usize {
        let before = self.saved.temporary.len();
        self.saved.temporary.retain(|block| !restore(block));
        self.dirty |= self.saved.temporary.len() != before;
        self.saved.temporary.len()
    }

    /// Writes the file if anything changed; deletes it once there is nothing left to undo.
    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        if let Err(error) = self.write() {
            log::error!(
                "MineHouse events: could not write the show journal {}: {error}",
                self.file.display()
            );
            self.dirty = true;
        }
    }

    fn write(&self) -> io::Result<()> {
        if self.saved.active_show.is_none()
            && self.saved.viewers.is_empty()
            && self.saved.temporary.is_empty()
        {
            return match fs::remove_file(&self.file) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            };
        }
        if let Some(parent) = self.file.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(&self.saved).map_err(io::Error::other)?;
        fs::write(&self.file, text)
    }
}
