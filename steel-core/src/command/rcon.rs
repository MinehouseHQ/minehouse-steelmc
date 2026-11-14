//! Output collection for commands run by a remote console, like vanilla's `RconConsoleSource`.
//!
//! Vanilla buffers everything an RCON command prints and answers with the whole buffer once the
//! command returns. Steel commands can be suspended across ticks, so instead of returning
//! synchronously the sender keeps an [`RconSink`] alive for as long as any part of the command
//! may still print: the sink is cloned into every derived command source, and the waiting
//! [`RconCommand`] is woken when the last clone is dropped.

use std::{fmt::Write, sync::Arc, time::Duration};

use steel_utils::locks::SyncMutex;
use text_components::TextComponent;
use tokio::{sync::oneshot, time::timeout as wait_at_most};

use super::sender::CommandSender;

/// What one remote console command printed and how it ended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RconReport {
    /// Every message sent to the source, one per line.
    pub output: String,
    /// `(success, result)` of the last terminal command, `None` when none ran (parse errors).
    pub result: Option<(bool, i32)>,
}

/// Collects the output of one remote console command. Owned by [`CommandSender::Rcon`].
pub struct RconSink {
    report: Arc<SyncMutex<RconReport>>,
    /// Never sent: dropping it with the sink tells [`RconCommand::finish`] the command is done.
    _alive: oneshot::Sender<()>,
}

impl RconSink {
    pub(crate) fn push_message(&self, message: &TextComponent) {
        let mut report = self.report.lock();
        // `RconConsoleSource.sendSystemMessage` appends the text and a line break.
        let _ = writeln!(report.output, "{message}");
    }

    pub(crate) fn result_recorder(&self) -> impl Fn(bool, i32) + Send + Sync + 'static {
        let report = Arc::clone(&self.report);
        move |success, result| report.lock().result = Some((success, result))
    }
}

/// The console side of one remote command: submit [`Self::sender`], then await [`Self::finish`].
pub struct RconCommand {
    report: Arc<SyncMutex<RconReport>>,
    finished: oneshot::Receiver<()>,
}

impl RconCommand {
    /// Creates a command and the sender to submit it with.
    #[must_use]
    pub fn new() -> (Self, CommandSender) {
        let report = Arc::new(SyncMutex::new(RconReport::default()));
        let (alive, finished) = oneshot::channel();
        let sink = RconSink {
            report: Arc::clone(&report),
            _alive: alive,
        };
        (
            Self { report, finished },
            CommandSender::Rcon(Arc::new(sink)),
        )
    }

    /// Waits until nothing can print for the command anymore, at most `timeout`, and returns
    /// what it printed. The boolean is `false` when the timeout cut the wait short.
    pub async fn finish(self, timeout: Duration) -> (RconReport, bool) {
        let completed = wait_at_most(timeout, self.finished).await.is_ok();
        let report = self.report.lock().clone();
        (report, completed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn finishes_when_the_last_sender_clone_is_dropped() {
        let (command, sender) = RconCommand::new();
        let clone = sender.clone();
        sender.send_message(&TextComponent::plain("first"));
        drop(sender);
        clone.send_message(&TextComponent::plain("second"));
        if let CommandSender::Rcon(sink) = &clone {
            (sink.result_recorder())(true, 3);
        }
        drop(clone);

        let (report, completed) = command.finish(Duration::from_secs(1)).await;
        assert!(completed);
        assert_eq!(report.output, "first\nsecond\n");
        assert_eq!(report.result, Some((true, 3)));
    }

    #[tokio::test]
    async fn times_out_while_a_sender_is_alive() {
        let (command, sender) = RconCommand::new();
        sender.send_message(&TextComponent::plain("partial"));
        let (report, completed) = command.finish(Duration::from_millis(20)).await;
        assert!(!completed);
        assert_eq!(report.output, "partial\n");
        drop(sender);
    }
}
