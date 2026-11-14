//! `config/minehouse/console.toml`, the settings of the Fabric `minehouse-console.json`.

use serde::{Deserialize, Serialize};

use crate::config::env_value;

/// Remote console settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ConsoleConfig {
    /// Turns the remote console off entirely.
    pub enabled: bool,
    /// Site with `/api/mc/console/poll`. Env `MINEHOUSE_API_URL` overrides it.
    pub api_url: String,
    /// Which server this is: `main` (production), `test`, `steel`, … The site keeps commands, logs
    /// and state per server and checks the signature with that server's own secret.
    /// Env `MINEHOUSE_SERVER_ID` overrides it.
    pub server_id: String,
    /// Name on the site and for Claude ("Прод", "Steel"). Env `MINEHOUSE_SERVER_NAME`.
    pub server_name: String,
    /// This server's console secret, `MC_CONSOLE_SECRET_<ID>` on the site. Empty: console off.
    /// Env `MINEHOUSE_CONSOLE_SECRET` overrides it.
    pub api_secret: String,
    /// Poll interval while nobody watches the console; the site asks for faster polls while
    /// someone does.
    pub idle_poll_seconds: u64,
    /// Timeout of one HTTP call.
    pub http_timeout_ms: u64,
    /// Replace IP addresses in log lines and command output with `[ip]` before they leave.
    pub mask_ips: bool,
    /// Commands (first word, without `/` and namespace) never run whatever the site says.
    pub blocked_commands: Vec<String>,
    /// Log lines kept in memory while the site is unreachable.
    pub log_buffer_lines: usize,
}

impl Default for ConsoleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api_url: "https://minehouse.pro".to_owned(),
            server_id: "main".to_owned(),
            server_name: String::new(),
            api_secret: String::new(),
            idle_poll_seconds: 10,
            http_timeout_ms: 10_000,
            mask_ips: true,
            blocked_commands: Vec::new(),
            log_buffer_lines: 5000,
        }
    }
}

impl ConsoleConfig {
    /// Applies environment overrides and clamps values to sane ranges.
    #[must_use]
    pub fn resolved(mut self) -> Self {
        if let Some(url) = env_value("MINEHOUSE_API_URL") {
            self.api_url = url;
        }
        if let Some(id) = env_value("MINEHOUSE_SERVER_ID") {
            self.server_id = id;
        }
        if let Some(name) = env_value("MINEHOUSE_SERVER_NAME") {
            self.server_name = name;
        }
        if let Some(secret) = env_value("MINEHOUSE_CONSOLE_SECRET") {
            self.api_secret = secret;
        }
        let valid_id = (1..=32).contains(&self.server_id.len())
            && self
                .server_id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !valid_id {
            log::warn!(
                "Console server_id {:?} is not [a-z0-9-]{{1,32}}, using main",
                self.server_id
            );
            "main".clone_into(&mut self.server_id);
        }
        self.idle_poll_seconds = self.idle_poll_seconds.clamp(2, 300);
        self.http_timeout_ms = self.http_timeout_ms.max(1000);
        self.log_buffer_lines = self.log_buffer_lines.clamp(100, 50_000);
        self
    }

    /// Whether the config forbids this command line.
    #[must_use]
    pub fn is_blocked(&self, command: &str) -> bool {
        let name = command_name(command);
        self.blocked_commands
            .iter()
            .any(|blocked| blocked.trim().eq_ignore_ascii_case(&name))
    }
}

/// `"/minecraft:Op Steve"` → `"op"`.
fn command_name(command: &str) -> String {
    let line = command.trim();
    let line = line.strip_prefix('/').unwrap_or(line);
    let first = line.split(' ').next().unwrap_or_default();
    let name = first.split_once(':').map_or(first, |(_, name)| name);
    name.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_commands_ignore_slash_namespace_and_case() {
        let config = ConsoleConfig {
            blocked_commands: vec!["op".to_owned()],
            ..ConsoleConfig::default()
        };
        assert!(config.is_blocked("/minecraft:Op Steve"));
        assert!(config.is_blocked("op Steve"));
        assert!(!config.is_blocked("opinion"));
    }
}
