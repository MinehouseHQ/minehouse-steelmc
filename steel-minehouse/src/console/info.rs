//! Server state shown on `/admin/console` and to Claude.

use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};
use steel_core::{GIT_HASH_SHORT, server::Server};

/// The `info` object of a poll, same fields as the Fabric console.
pub fn snapshot(server: &Server) -> Value {
    let players: Vec<String> = server
        .get_players()
        .iter()
        .map(|player| player.gameprofile.name().to_string())
        .collect();
    let mspt = f64::from(server.tick_rate_manager.read().get_average_mspt());
    let tps = if mspt > 0.0 {
        (1000.0 / mspt).min(20.0)
    } else {
        20.0
    };
    json!({
        "players": players,
        "max_players": server.config.max_players,
        "mspt": (mspt * 10.0).round() / 10.0,
        "tps": (tps * 10.0).round() / 10.0,
        "memory_used_mb": resident_memory_mb(),
        "memory_max_mb": memory_limit_mb(),
        "version": format!("Steel {} ({GIT_HASH_SHORT})", env!("CARGO_PKG_VERSION")),
        "mod_version": env!("CARGO_PKG_VERSION"),
        "motd": server.config.motd,
        "time": SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_millis() as u64),
    })
}

/// Resident set size of this process (Linux), 0 when unknown.
fn resident_memory_mb() -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("VmRSS:"))
                .and_then(|value| {
                    value
                        .trim()
                        .trim_end_matches("kB")
                        .trim()
                        .parse::<u64>()
                        .ok()
                })
        })
        .map_or(0, |kb| kb / 1024)
}

/// Memory limit of the container (cgroup v2), 0 when there is none.
fn memory_limit_mb() -> u64 {
    fs::read_to_string("/sys/fs/cgroup/memory.max")
        .ok()
        .and_then(|limit| limit.trim().parse::<u64>().ok())
        .map_or(0, |bytes| bytes / (1024 * 1024))
}
