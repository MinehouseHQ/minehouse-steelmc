//! No phantoms from insomnia (the Fabric `PhantomSpawnerMixin`). Spawn eggs and `/summon`
//! still make them.
//!
//! Vanilla has the game rule `spawn_phantoms` for this, and the worlds brought over from the
//! Fabric server have it on, because there a mixin did the job. So the rule is turned off in
//! every world when the server starts, unless `config/minehouse/tweaks.toml` says
//! `insomnia = true` under `[phantoms]`.

use steel_core::server::Server;
use steel_registry::vanilla_game_rules::SPAWN_PHANTOMS;

use super::end_lock;

/// Turns the game rule off in the worlds that still have it on.
pub(super) fn start(server: &Server) {
    if end_lock::phantoms_from_insomnia() {
        return;
    }
    let mut changed = 0;
    for world in server.worlds.values() {
        if world.get_game_rule(&SPAWN_PHANTOMS) && world.set_game_rule(&SPAWN_PHANTOMS, false) {
            changed += 1;
        }
    }
    if changed > 0 {
        log::info!("MineHouse phantoms: insomnia turned off in {changed} worlds");
    }
}
