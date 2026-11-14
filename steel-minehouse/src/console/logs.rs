//! The newest log lines, kept until the site has them.

use std::{
    collections::VecDeque,
    sync::LazyLock,
    time::{SystemTime, UNIX_EPOCH},
};

use regex::{Captures, Regex};
use serde::Serialize;
use steel_utils::{locks::SyncMutex, logger::LogData};
use tracing::{Level, Subscriber};
use tracing_subscriber::{Layer, layer::Context};

/// Longest message kept per line.
const MAX_MESSAGE: usize = 4000;

/// One log line. `seq` grows by one per line within a server run.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct LogLine {
    /// Number of the line in this run.
    pub seq: u64,
    /// Unix time in milliseconds.
    pub time: u64,
    /// `INFO`, `WARN` or `ERROR`.
    pub level: &'static str,
    /// Module that logged it.
    pub logger: String,
    /// The text, with IP addresses masked when enabled.
    pub message: String,
}

struct State {
    lines: VecDeque<LogLine>,
    capacity: usize,
    next_seq: u64,
    /// Lines up to this one reached the site.
    acked: u64,
    enabled: bool,
    mask_ips: bool,
}

/// Log lines waiting for the site. Collects from server start, before the console config is
/// read, so the site gets the whole startup log; [`LogBuffer::configure`] trims or disables it.
pub struct LogBuffer {
    state: SyncMutex<State>,
}

/// The buffer [`log_layer`] fills.
pub static LOG_BUFFER: LogBuffer = LogBuffer::new(5000);

impl LogBuffer {
    const fn new(capacity: usize) -> Self {
        Self {
            state: SyncMutex::new(State {
                lines: VecDeque::new(),
                capacity,
                next_seq: 1,
                acked: 0,
                enabled: true,
                mask_ips: true,
            }),
        }
    }

    /// Applies the console config. A disabled buffer drops what it has and stops collecting.
    pub fn configure(&self, enabled: bool, capacity: usize, mask_ips: bool) {
        let mut state = self.state.lock();
        state.enabled = enabled;
        state.capacity = capacity;
        state.mask_ips = mask_ips;
        if !enabled {
            state.lines = VecDeque::new();
            return;
        }
        if mask_ips {
            for line in &mut state.lines {
                line.message = self::mask_ips(&line.message);
            }
        }
        while state.lines.len() > capacity {
            state.lines.pop_front();
        }
    }

    fn push(&self, level: &'static str, logger: String, message: &str) {
        let mut state = self.state.lock();
        if !state.enabled {
            return;
        }
        let message = clip(
            &if state.mask_ips {
                mask_ips(message)
            } else {
                message.to_owned()
            },
            MAX_MESSAGE,
        );
        let line = LogLine {
            seq: state.next_seq,
            time: now_millis(),
            level,
            logger,
            message,
        };
        state.next_seq += 1;
        state.lines.push_back(line);
        while state.lines.len() > state.capacity {
            state.lines.pop_front();
        }
    }

    /// Up to `max_lines` lines the site doesn't have yet, oldest first, stopping before
    /// `max_chars` (at least one line).
    pub fn pending(&self, max_lines: usize, max_chars: usize) -> Vec<LogLine> {
        let state = self.state.lock();
        let mut out = Vec::new();
        let mut chars = 0;
        for line in state.lines.iter().filter(|line| line.seq > state.acked) {
            chars += line.message.len() + line.logger.len() + 64;
            if out.len() >= max_lines || (chars > max_chars && !out.is_empty()) {
                break;
            }
            out.push(line.clone());
        }
        out
    }

    /// Whether lines after `seq` are waiting.
    pub fn has_pending_after(&self, seq: u64) -> bool {
        self.state
            .lock()
            .lines
            .back()
            .is_some_and(|line| line.seq > seq)
    }

    /// The site stored everything up to `seq`: those lines can go.
    pub fn ack(&self, seq: u64) {
        let mut state = self.state.lock();
        state.acked = state.acked.max(seq);
        let acked = state.acked;
        while state.lines.front().is_some_and(|line| line.seq <= acked) {
            state.lines.pop_front();
        }
    }
}

/// Tracing layer that feeds [`LOG_BUFFER`] with what goes to the console: `INFO` and above.
#[must_use]
pub const fn log_layer() -> LogBufferLayer {
    LogBufferLayer
}

/// See [`log_layer`].
pub struct LogBufferLayer;

impl<S: Subscriber> Layer<S> for LogBufferLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let level = match *metadata.level() {
            Level::ERROR => "ERROR",
            Level::WARN => "WARN",
            Level::INFO => "INFO",
            Level::DEBUG | Level::TRACE => return,
        };
        let mut data = LogData::new();
        event.record(&mut data);
        let logger = if data.module_path.is_empty() {
            metadata.target().to_owned()
        } else {
            data.module_path
        };
        // Like the console without `[log] extra`: the message alone, no extra fields.
        let message =
            String::from_utf8_lossy(&strip_ansi_escapes::strip(data.message)).into_owned();
        LOG_BUFFER.push(level, clip(&logger, 200), &message);
    }
}

/// Replaces IPv4 and IPv6 addresses with `[ip]`, like the Fabric console's `maskIps`.
///
/// The Fabric patterns use lookarounds, which `regex` lacks: an address only counts when it is
/// a whole run of address characters, so each run is matched and checked on its own.
#[must_use]
pub fn mask_ips(text: &str) -> String {
    static V4_RUN: LazyLock<Regex> = LazyLock::new(|| compile(r"[0-9.]+"));
    static V4: LazyLock<Regex> = LazyLock::new(|| compile(r"^(?:\d{1,3}\.){3}\d{1,3}$"));
    static V6_RUN: LazyLock<Regex> = LazyLock::new(|| compile(r"[\w:]+"));
    static V6: LazyLock<Regex> =
        LazyLock::new(|| compile(r"^(?:[0-9a-fA-F]{1,4}:){4,7}[0-9a-fA-F]{1,4}$"));

    let text = V4_RUN.replace_all(text, |run: &Captures<'_>| mask_run(&run[0], &V4));
    V6_RUN
        .replace_all(&text, |run: &Captures<'_>| mask_run(&run[0], &V6))
        .into_owned()
}

fn mask_run(run: &str, address: &Regex) -> String {
    if address.is_match(run) {
        "[ip]".to_owned()
    } else {
        run.to_owned()
    }
}

fn compile(pattern: &str) -> Regex {
    match Regex::new(pattern) {
        Ok(regex) => regex,
        Err(error) => unreachable!("static pattern {pattern} is valid: {error}"),
    }
}

/// Cuts `text` to at most `max` characters, ending with `…` when cut.
#[must_use]
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut clipped: String = text.chars().take(max.saturating_sub(1)).collect();
    clipped.push('…');
    clipped
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_whole_addresses_only() {
        assert_eq!(
            mask_ips("Accepted connection from 1.2.3.4:5555 and 10.0.0.1"),
            "Accepted connection from [ip]:5555 and [ip]"
        );
        assert_eq!(mask_ips("version 0.16.1.2.3"), "version 0.16.1.2.3");
        assert_eq!(
            mask_ips("v6 2001:db8:0:0:0:ff00:42:8329 end"),
            "v6 [ip] end"
        );
        assert_eq!(mask_ips("time 12:30:45"), "time 12:30:45");
    }

    #[test]
    fn acked_lines_leave_the_buffer() {
        let buffer = LogBuffer::new(3);
        for message in ["a", "b", "c", "d"] {
            buffer.push("INFO", "test".to_owned(), message);
        }
        let pending = buffer.pending(10, 10_000);
        assert_eq!(
            pending
                .iter()
                .map(|l| l.message.as_str())
                .collect::<Vec<_>>(),
            ["b", "c", "d"]
        );
        buffer.ack(pending[1].seq);
        assert_eq!(buffer.pending(10, 10_000).len(), 1);
        assert!(buffer.has_pending_after(pending[1].seq));
        assert!(!buffer.has_pending_after(pending[2].seq));
    }
}
