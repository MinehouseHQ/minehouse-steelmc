//! `config/minehouse/bank.toml`, the settings of the Fabric `minehouse-bank.json`.

use serde::{Deserialize, Serialize};

use crate::{
    config::{self, env_value},
    console::ConsoleConfig,
};

/// Bank settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BankConfig {
    /// Turns the bank off entirely.
    pub enabled: bool,
    /// Site with `/api/mc/bank/*`. Empty: the one of `console.toml`.
    pub api_url: String,
    /// Which server this is: the site keeps the accounts of production and of the test server
    /// apart. Empty: the one of `console.toml`.
    pub server_id: String,
    /// This server's secret. Empty: the one of `console.toml`, the bank signs its calls with
    /// the same per-server secret as the console. Env `MINEHOUSE_CONSOLE_SECRET` overrides it.
    pub api_secret: String,
    /// Timeout of one HTTP call.
    pub http_timeout_ms: u64,
    /// What the ATM charges for a payment terminal.
    pub terminal_price: i32,
    /// Most ores one withdrawal hands out.
    pub max_withdraw: i32,
    /// A till's invoice is gone after this many seconds.
    pub invoice_seconds: u64,
    /// How far from a terminal or an ATM a player may use it.
    pub use_distance: f64,
    /// Whether the spawn zones are open to players.
    pub zones: bool,
}

impl Default for BankConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api_url: String::new(),
            server_id: String::new(),
            api_secret: String::new(),
            http_timeout_ms: 10_000,
            terminal_price: 5,
            max_withdraw: 640,
            invoice_seconds: 120,
            use_distance: 6.0,
            zones: true,
        }
    }
}

impl BankConfig {
    /// Fills what is empty from the console settings and the environment.
    #[must_use]
    pub fn resolved(self) -> Self {
        let console = config::load::<ConsoleConfig>(&config::path("console"))
            .unwrap_or_default()
            .resolved();
        self.resolved_with(&console, env_value("MINEHOUSE_CONSOLE_SECRET"))
    }

    fn resolved_with(mut self, console: &ConsoleConfig, env_secret: Option<String>) -> Self {
        if self.api_url.trim().is_empty() {
            self.api_url.clone_from(&console.api_url);
        }
        if self.server_id.trim().is_empty() {
            self.server_id.clone_from(&console.server_id);
        }
        if self.api_secret.trim().is_empty() {
            self.api_secret.clone_from(&console.api_secret);
        }
        if let Some(secret) = env_secret {
            self.api_secret = secret;
        }
        self.http_timeout_ms = self.http_timeout_ms.clamp(1000, 60_000);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console() -> ConsoleConfig {
        ConsoleConfig {
            api_url: "https://example.test".to_owned(),
            server_id: "test".to_owned(),
            api_secret: "console-secret".to_owned(),
            ..ConsoleConfig::default()
        }
    }

    #[test]
    fn empty_fields_come_from_the_console_settings() {
        let config = BankConfig::default().resolved_with(&console(), None);
        assert_eq!(config.api_url, "https://example.test");
        assert_eq!(config.server_id, "test");
        assert_eq!(config.api_secret, "console-secret");
    }

    #[test]
    fn own_fields_win_and_the_environment_wins_over_both() {
        let own = BankConfig {
            api_url: "https://bank.test".to_owned(),
            server_id: "main".to_owned(),
            api_secret: "own".to_owned(),
            ..BankConfig::default()
        };
        let config = own.clone().resolved_with(&console(), None);
        assert_eq!(config.api_url, "https://bank.test");
        assert_eq!(config.server_id, "main");
        assert_eq!(config.api_secret, "own");
        let config = own.resolved_with(&console(), Some("env".to_owned()));
        assert_eq!(config.api_secret, "env");
    }
}
