//! What a show's permanent blocks replaced, kept in `events/permanent/<show>.txt` of the
//! data directory in case the change has to be undone by hand (`/show undo`). One line per
//! block: `world x y z state`. What chests and the like held is not here; the block log
//! has it.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::PathBuf,
    str::FromStr as _,
    time::{SystemTime, UNIX_EPOCH},
};

use steel_core::server::Server;
use steel_utils::{BlockPos, BlockStateId, Identifier, types::UpdateFlags};

use super::{
    blocks::{block_state, state_text},
    script::check_name,
};
use crate::config;

fn file(show: &str) -> Result<PathBuf, String> {
    check_name(show)?;
    Ok(config::data_path(&format!("events/permanent/{show}.txt")))
}

/// Writes down what `replaced` blocks were before show `show` changed them for good.
pub fn backup(show: &str, world: &str, replaced: &[(BlockPos, BlockStateId)]) -> io::Result<()> {
    if replaced.is_empty() {
        return Ok(());
    }
    let file = file(show).map_err(io::Error::other)?;
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = String::new();
    for (pos, state) in replaced {
        text.push_str(&line(world, *pos, *state));
        text.push('\n');
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)?
        .write_all(text.as_bytes())
}

fn line(world: &str, pos: BlockPos, state: BlockStateId) -> String {
    format!(
        "{world} {} {} {} {}",
        pos.x(),
        pos.y(),
        pos.z(),
        state_text(state)
    )
}

/// `world x y z state`, as [`line`] wrote it.
fn parse(line: &str) -> Option<(Identifier, BlockPos, BlockStateId)> {
    let mut parts = line.trim().splitn(5, ' ');
    let world = Identifier::from_str(parts.next()?).ok()?;
    let x = parts.next()?.parse().ok()?;
    let y = parts.next()?.parse().ok()?;
    let z = parts.next()?.parse().ok()?;
    let state = block_state(parts.next()?).ok()?;
    Some((world, BlockPos::new(x, y, z), state))
}

/// Puts back what the permanent blocks of `show` replaced and renames the file, so it is
/// not undone twice. Returns how many blocks went back, or `None` if there is nothing to
/// undo.
pub fn undo(server: &Server, show: &str) -> Result<Option<usize>, String> {
    let file = file(show)?;
    let text = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let mut restored = 0;
    // Backwards: when a block was changed twice, what it was first is what stays.
    for (world, pos, state) in text.lines().rev().filter_map(parse) {
        if let Some(world) = server.worlds.get(&world) {
            world.set_block(pos, state, UpdateFlags::UPDATE_CLIENTS);
            restored += 1;
        }
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let done = file.with_file_name(format!("{show}.undone-{stamp}.txt"));
    fs::rename(&file, done).map_err(|error| error.to_string())?;
    Ok(Some(restored))
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_blocks};

    use super::*;

    #[test]
    fn a_line_is_read_back() {
        init_vanilla_registry();
        let stairs = block_state("minecraft:oak_stairs[facing=east]").expect("stairs");
        let pos = BlockPos::new(-12, 64, 300);
        let text = line("minecraft:overworld", pos, stairs);
        assert!(text.starts_with("minecraft:overworld -12 64 300 minecraft:oak_stairs["));
        let (world, at, state) = parse(&text).expect("a line");
        assert_eq!(world.to_string(), "minecraft:overworld");
        assert_eq!((at, state), (pos, stairs));
        assert_eq!(
            parse("minecraft:overworld 1 2 3 minecraft:stone").map(|line| line.2),
            Some(vanilla_blocks::STONE.default_state())
        );
        assert!(parse("minecraft:overworld 1 2").is_none());
    }
}
