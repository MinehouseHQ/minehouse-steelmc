//! Remote console without open ports, ported from the Fabric `minehouse-console` mod.
//!
//! A background task polls the site (`POST /api/mc/console/poll`), takes queued commands, runs
//! them like RCON does and sends back their output together with new log lines and the server
//! state. Commands come from site admins on `/admin/console` and from MCP clients they
//! authorized; the site keeps the audit log.

mod info;
mod logs;
mod settings;

use std::{
    mem,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use steel_core::{command::rcon::RconCommand, server::Server};
use steel_utils::locks::SyncMutex;
use tokio::{
    sync::Notify,
    task::JoinHandle,
    time::{sleep, timeout},
};
use tokio_util::sync::CancellationToken;

pub use logs::{LOG_BUFFER, LogBufferLayer, LogLine, log_layer, mask_ips};
pub use settings::ConsoleConfig;

use crate::{
    config,
    site::{SiteClient, SiteError},
};

/// Site endpoint the console polls.
pub const POLL_PATH: &str = "/api/mc/console/poll";
/// Longest command output sent to the site.
const MAX_OUTPUT: usize = 16_000;
const MAX_LOG_LINES_PER_POLL: usize = 300;
const MAX_LOG_CHARS_PER_POLL: usize = 256 * 1024;
/// How long a command may keep printing before its output is sent as it is.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// Outcome of one command, waiting to be reported.
#[derive(Clone, Debug, Serialize)]
struct CommandResult {
    id: String,
    success: bool,
    result: i32,
    output: String,
}

/// A command the site queued.
#[derive(Debug, Deserialize)]
struct RemoteCommand {
    id: String,
    command: String,
    #[serde(default)]
    by: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PollAnswer {
    #[serde(default)]
    commands: Vec<RemoteCommand>,
    #[serde(default)]
    poll_ms: Option<u64>,
}

/// The running remote console.
pub struct Console {
    server: Arc<Server>,
    config: ConsoleConfig,
    site: SiteClient,
    boot_id: String,
    started_at: u64,
    results: SyncMutex<Vec<CommandResult>>,
    /// Woken when there is something to report right away.
    wake: Notify,
    stopping: AtomicBool,
    cancel: CancellationToken,
    poller: SyncMutex<Option<JoinHandle<()>>>,
}

impl Console {
    /// Loads `config/minehouse/console.toml` and starts polling, unless the console is off.
    pub fn start(server: &Arc<Server>) -> Option<Arc<Self>> {
        let config = match config::load::<ConsoleConfig>(&config::path("console")) {
            Ok(config) => config.resolved(),
            Err(error) => {
                log::error!("Remote console is off, bad config: {error}");
                LOG_BUFFER.configure(false, 0, true);
                return None;
            }
        };
        if !config.enabled {
            log::info!("Remote console is disabled in config/minehouse/console.toml");
            LOG_BUFFER.configure(false, 0, true);
            return None;
        }
        if config.api_secret.is_empty() {
            log::warn!(
                "No console secret: set api_secret in config/minehouse/console.toml or MINEHOUSE_CONSOLE_SECRET. Remote console is off."
            );
            LOG_BUFFER.configure(false, 0, true);
            return None;
        }
        LOG_BUFFER.configure(true, config.log_buffer_lines, config.mask_ips);

        let site = match SiteClient::new(
            &config.api_url,
            &config.api_secret,
            Duration::from_millis(config.http_timeout_ms),
            &config.server_id,
        ) {
            Ok(site) => site,
            Err(error) => {
                log::error!("Remote console is off, no HTTP client: {error}");
                return None;
            }
        };

        let console = Arc::new(Self {
            server: Arc::clone(server),
            config,
            site,
            boot_id: uuid::Uuid::new_v4().to_string(),
            started_at: now_millis(),
            results: SyncMutex::new(Vec::new()),
            wake: Notify::new(),
            stopping: AtomicBool::new(false),
            cancel: CancellationToken::new(),
            poller: SyncMutex::new(None),
        });
        let handle = tokio::spawn(Arc::clone(&console).run());
        *console.poller.lock() = Some(handle);
        log::info!(
            "Remote console started for server {:?}",
            console.config.server_id
        );
        Some(console)
    }

    /// Reports what is left (the result of `stop` among it) and stops polling.
    pub async fn stop(self: &Arc<Self>) {
        if self.stopping.swap(true, Ordering::AcqRel) {
            return;
        }
        self.cancel.cancel();
        let poller = self.poller.lock().take();
        if let Some(poller) = poller {
            let wait = Duration::from_millis(self.config.http_timeout_ms + 1000);
            let _ = timeout(wait, poller).await;
        }
        if let Err(error) = self.poll_once().await {
            log::warn!("Could not send the last console report: {error}");
        }
    }

    async fn run(self: Arc<Self>) {
        let idle = Duration::from_secs(self.config.idle_poll_seconds);
        let mut failing = false;
        loop {
            let wait = match self.poll_once().await {
                Ok(wait) => {
                    if failing {
                        log::info!("Remote console reconnected to the site");
                        failing = false;
                    }
                    wait
                }
                Err(error) => {
                    if !failing {
                        log::warn!(
                            "Remote console can't reach the site, will keep trying: {error}"
                        );
                        failing = true;
                    }
                    idle
                }
            };
            tokio::select! {
                () = self.cancel.cancelled() => return,
                () = self.wake.notified() => {}
                () = sleep(wait) => {}
            }
        }
    }

    /// One round trip. Returns how long to wait before the next one.
    async fn poll_once(self: &Arc<Self>) -> Result<Duration, SiteError> {
        let sending = mem::take(&mut *self.results.lock());
        let lines = LOG_BUFFER.pending(MAX_LOG_LINES_PER_POLL, MAX_LOG_CHARS_PER_POLL);
        let body = json!({
            "boot_id": self.boot_id,
            "started_at": self.started_at,
            "stopping": self.stopping.load(Ordering::Acquire),
            "name": self.config.server_name,
            "info": info::snapshot(&self.server),
            "logs": lines,
            "results": sending,
        });

        let answer = match self.site.post_json(POLL_PATH, body.to_string()).await {
            Ok(answer) => answer,
            Err(error) => {
                // Try again with the next poll.
                self.results.lock().splice(0..0, sending);
                return Err(error);
            }
        };

        let last_sent = lines.last().map(|line| line.seq);
        if let Some(seq) = last_sent {
            LOG_BUFFER.ack(seq);
        }

        let answer: PollAnswer = serde_json::from_str(&answer).unwrap_or(PollAnswer {
            commands: Vec::new(),
            poll_ms: None,
        });
        let has_commands = !answer.commands.is_empty();
        for command in answer.commands {
            self.submit(command);
        }

        let more_logs = LOG_BUFFER.has_pending_after(last_sent.unwrap_or(0));
        if more_logs || has_commands || !self.results.lock().is_empty() {
            return Ok(Duration::ZERO);
        }
        Ok(answer.poll_ms.map_or_else(
            || Duration::from_secs(self.config.idle_poll_seconds),
            |ms| Duration::from_millis(ms.clamp(500, 300_000)),
        ))
    }

    /// Queues a command on the server and reports its output once it is done.
    fn submit(self: &Arc<Self>, command: RemoteCommand) {
        let by = command.by.as_deref().unwrap_or("?");
        if self.stopping.load(Ordering::Acquire) {
            self.report(
                command.id,
                false,
                0,
                "Сервер останавливается, команда не выполнена.".to_owned(),
            );
            return;
        }
        if self.config.is_blocked(&command.command) {
            log::warn!(
                "Remote console, {by}: refused blocked command: {}",
                command.command
            );
            self.report(
                command.id,
                false,
                0,
                "Команда запрещена в config/minehouse/console.toml (blocked_commands).".to_owned(),
            );
            return;
        }

        log::info!("Remote console, {by}: {}", command.command);
        let (rcon, sender) = RconCommand::new();
        if self
            .server
            .submit_command(sender, command.command.trim().to_owned())
            .is_err()
        {
            self.report(
                command.id,
                false,
                0,
                "Очередь команд сервера переполнена, команда не выполнена.".to_owned(),
            );
            return;
        }

        let console = Arc::clone(self);
        tokio::spawn(async move {
            let (report, completed) = rcon.finish(COMMAND_TIMEOUT).await;
            let mut output = report.output.trim_end().to_owned();
            if !completed {
                output.push_str("\n(команда ещё выполняется, вывод неполный)");
            }
            let (success, result) = report.result.unwrap_or((false, 0));
            console.report(command.id, success, result, output);
        });
    }

    fn report(&self, id: String, success: bool, result: i32, output: String) {
        let output = if self.config.mask_ips {
            mask_ips(&output)
        } else {
            output
        };
        self.results.lock().push(CommandResult {
            id,
            success,
            result,
            output: logs::clip(&output, MAX_OUTPUT),
        });
        self.wake.notify_one();
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_millis() as u64)
}
