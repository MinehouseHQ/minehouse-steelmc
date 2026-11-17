//! Vanilla `BaseCommandBlock`: the command, success count and last output
//! shared by command blocks and command block minecarts, and the command
//! source they run their command as.

use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as NbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_registry::game_rules::GameRule;
use steel_registry::vanilla_game_rules::{
    COMMAND_BLOCK_OUTPUT, COMMAND_BLOCKS_WORK, SEND_COMMAND_FEEDBACK,
};
use steel_utils::locks::SyncMutex;
use text_components::{Modifier as _, TextComponent};

use crate::command::PendingCommandExecutionQueue;
use crate::command::execution::{CommandResultCallback, CommandSource, ExecutionCommandSource};
use crate::command::sender::CommandExecutionOwner;
use crate::entity::SharedEntity;
use crate::server::Server;
use crate::world::World;

/// Vanilla `BaseCommandBlock.NO_LAST_EXECUTION`.
const NO_LAST_EXECUTION: i64 = -1;

/// Vanilla `BaseCommandBlock.DEFAULT_NAME`.
const DEFAULT_NAME: &str = "@";

/// Called when a command block's command or last output changes, vanilla
/// `BaseCommandBlock.onUpdated`.
pub(crate) type OnCommandBlockUpdated = Arc<dyn Fn(&World) + Send + Sync>;

struct BaseCommandBlockState {
    last_execution: i64,
    update_last_execution: bool,
    success_count: i32,
    track_output: bool,
    last_output: Option<TextComponent>,
    command: String,
    custom_name: Option<TextComponent>,
}

/// Vanilla `BaseCommandBlock`: what a command block or command block minecart
/// runs, how often its last run succeeded and what it last printed.
pub struct BaseCommandBlock {
    state: SyncMutex<BaseCommandBlockState>,
}

impl Default for BaseCommandBlock {
    fn default() -> Self {
        Self::new()
    }
}

impl BaseCommandBlock {
    /// Creates an empty command block that tracks its output.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: SyncMutex::new(BaseCommandBlockState {
                last_execution: NO_LAST_EXECUTION,
                update_last_execution: true,
                success_count: 0,
                track_output: true,
                last_output: None,
                command: String::new(),
                custom_name: None,
            }),
        }
    }

    /// Vanilla `getSuccessCount`.
    #[must_use]
    pub fn success_count(&self) -> i32 {
        self.state.lock().success_count
    }

    /// Vanilla `setSuccessCount`.
    pub fn set_success_count(&self, success_count: i32) {
        self.state.lock().success_count = success_count;
    }

    /// Vanilla `getLastOutput`: the empty component when nothing was printed.
    #[must_use]
    pub fn last_output(&self) -> TextComponent {
        self.state
            .lock()
            .last_output
            .clone()
            .unwrap_or_else(|| TextComponent::plain(""))
    }

    /// Vanilla `setLastOutput`.
    pub fn set_last_output(&self, last_output: Option<TextComponent>) {
        self.state.lock().last_output = last_output;
    }

    /// Vanilla `getCommand`.
    #[must_use]
    pub fn command(&self) -> String {
        self.state.lock().command.clone()
    }

    /// Vanilla `setCommand`: a new command starts with no successes.
    pub fn set_command(&self, command: String) {
        let mut state = self.state.lock();
        state.command = command;
        state.success_count = 0;
    }

    /// Vanilla `getName`: the custom name, or `@`.
    #[must_use]
    pub fn name(&self) -> TextComponent {
        self.custom_name()
            .unwrap_or_else(|| TextComponent::plain(DEFAULT_NAME))
    }

    /// Vanilla `getCustomName`.
    #[must_use]
    pub fn custom_name(&self) -> Option<TextComponent> {
        self.state.lock().custom_name.clone()
    }

    /// Vanilla `setCustomName`.
    pub fn set_custom_name(&self, name: Option<TextComponent>) {
        self.state.lock().custom_name = name;
    }

    /// Vanilla `isTrackOutput`.
    #[must_use]
    pub fn is_track_output(&self) -> bool {
        self.state.lock().track_output
    }

    /// Vanilla `setTrackOutput`.
    pub fn set_track_output(&self, track_output: bool) {
        self.state.lock().track_output = track_output;
    }

    /// Vanilla `BaseCommandBlock.save`.
    pub fn save(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        nbt.insert("Command", state.command.as_str());
        nbt.insert("SuccessCount", state.success_count);
        if let Some(name) = &state.custom_name {
            nbt.insert("CustomName", name.to_codec_nbt());
        }
        nbt.insert("TrackOutput", i8::from(state.track_output));
        if state.track_output
            && let Some(last_output) = &state.last_output
        {
            nbt.insert("LastOutput", last_output.to_codec_nbt());
        }
        nbt.insert("UpdateLastExecution", i8::from(state.update_last_execution));
        if state.update_last_execution && state.last_execution != NO_LAST_EXECUTION {
            nbt.insert("LastExecution", state.last_execution);
        }
    }

    /// Vanilla `BaseCommandBlock.load`.
    pub fn load(&self, nbt: &NbtCompoundView<'_, '_>) {
        let text = |key: &str| {
            nbt.get(key)
                .and_then(|tag| TextComponent::from_nbt(&tag.to_owned()))
        };
        let boolean = |key: &str, default: bool| nbt.byte(key).map_or(default, |value| value != 0);
        let mut state = self.state.lock();
        state.command = nbt
            .string("Command")
            .map_or_else(String::new, ToString::to_string);
        state.success_count = nbt.int("SuccessCount").unwrap_or(0);
        state.custom_name = text("CustomName");
        state.track_output = boolean("TrackOutput", true);
        state.last_output = if state.track_output {
            text("LastOutput")
        } else {
            None
        };
        state.update_last_execution = boolean("UpdateLastExecution", true);
        state.last_execution = if state.update_last_execution {
            nbt.long("LastExecution").unwrap_or(NO_LAST_EXECUTION)
        } else {
            NO_LAST_EXECUTION
        };
    }

    /// Vanilla `BaseCommandBlock.performCommand`: runs the command at most once
    /// per game tick. Returns `false` when it already ran this tick.
    pub(crate) fn perform_command(
        self: &Arc<Self>,
        runner: &mut CommandBlockRunner<'_>,
        world: &Arc<World>,
        origin: CommandBlockOrigin,
    ) -> bool {
        let game_time = world.game_time();
        let command = {
            let mut state = self.state.lock();
            if game_time == state.last_execution {
                return false;
            }
            if is_searge(&state.command) {
                state.last_output = Some(TextComponent::plain("#itzlipofutzli"));
                state.success_count = 1;
                return true;
            }
            state.success_count = 0;
            state.command.clone()
        };

        if world.get_game_rule(&COMMAND_BLOCKS_WORK) && !command.is_empty() {
            self.set_last_output(None);
            let output = Arc::new(CommandBlockOutput {
                block: Arc::clone(self),
                world: Arc::downgrade(world),
                on_updated: Arc::clone(&origin.on_updated),
                tracking: self.is_track_output(),
                closed: AtomicBool::new(false),
            });
            let block = Arc::clone(self);
            let callback = CommandResultCallback::new(move |success, _result| {
                if success {
                    block.state.lock().success_count += 1;
                }
            });
            let source = CommandSource::for_command_block(
                Arc::clone(runner.server),
                Arc::clone(world),
                Arc::clone(&output),
                origin.position,
                origin.rotation,
                origin.entity,
            )
            .with_callback(callback);
            runner.run(source, &command);
            output.close();
        }

        let mut state = self.state.lock();
        state.last_execution = if state.update_last_execution {
            game_time
        } else {
            NO_LAST_EXECUTION
        };
        true
    }
}

/// Vanilla `"Searge".equalsIgnoreCase(command)`, with Java's per-character
/// case folding so that, like vanilla, `ſearge` matches too.
fn is_searge(command: &str) -> bool {
    const SEARGE: &str = "Searge";
    fn simple_upper(c: char) -> char {
        let mut upper = c.to_uppercase();
        match (upper.next(), upper.next()) {
            (Some(single), None) => single,
            _ => c,
        }
    }
    fn simple_lower(c: char) -> char {
        let mut lower = c.to_lowercase();
        match (lower.next(), lower.next()) {
            (Some(single), None) => single,
            _ => c,
        }
    }
    command.chars().count() == SEARGE.len()
        && command.chars().zip(SEARGE.chars()).all(|(a, b)| {
            let (upper_a, upper_b) = (simple_upper(a), simple_upper(b));
            a == b || upper_a == upper_b || simple_lower(upper_a) == simple_lower(upper_b)
        })
}

/// Where a command block's command runs, vanilla `createCommandSourceStack`.
pub(crate) struct CommandBlockOrigin {
    /// The source position.
    pub position: DVec3,
    /// The source rotation as `(yaw, pitch)`.
    pub rotation: (f32, f32),
    /// The executing entity: the minecart for a command block minecart.
    pub entity: Option<SharedEntity>,
    /// Vanilla `onUpdated`, called whenever the last output changes.
    pub on_updated: OnCommandBlockUpdated,
}

/// The command source a running command block prints to, vanilla
/// `BaseCommandBlock.CloseableCommandBlockSource`, or vanilla's
/// `CommandSource.NULL` when the block does not track its output.
///
/// It closes when the command returns, so output from work that resumes on a
/// later tick is dropped like vanilla drops it after `close`.
pub struct CommandBlockOutput {
    block: Arc<BaseCommandBlock>,
    world: Weak<World>,
    on_updated: OnCommandBlockUpdated,
    tracking: bool,
    closed: AtomicBool,
}

impl CommandBlockOutput {
    fn is_open(&self) -> bool {
        self.tracking && !self.closed.load(Ordering::Acquire)
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    fn game_rule(&self, rule: &GameRule<bool>) -> bool {
        self.world
            .upgrade()
            .is_some_and(|world| world.get_game_rule(rule))
    }

    /// Vanilla `acceptsSuccess`.
    pub(crate) fn accepts_success(&self) -> bool {
        self.is_open() && self.game_rule(&SEND_COMMAND_FEEDBACK)
    }

    /// Vanilla `acceptsFailure`.
    pub(crate) fn accepts_failure(&self) -> bool {
        self.is_open()
    }

    /// Vanilla `shouldInformAdmins`.
    pub(crate) fn should_inform_admins(&self) -> bool {
        self.is_open() && self.game_rule(&COMMAND_BLOCK_OUTPUT)
    }

    /// Vanilla `sendSystemMessage`: the message becomes the block's last
    /// output, stamped with the local time.
    pub(crate) fn push_message(&self, message: &TextComponent) {
        if !self.is_open() {
            return;
        }
        let time = chrono::Local::now().format("%H:%M:%S");
        self.block.set_last_output(Some(
            TextComponent::plain(format!("[{time}] ")).add_child(message.clone()),
        ));
        if let Some(world) = self.world.upgrade() {
            (self.on_updated)(&world);
        }
    }

    /// The command block's name, which vanilla gives its command source.
    pub(crate) fn name(&self) -> TextComponent {
        self.block.name()
    }
}

/// Runs command block commands on the server thread, where their executions
/// can suspend into the server's pending command queue.
pub(crate) struct CommandBlockRunner<'a> {
    /// The server running the command.
    pub server: &'a Arc<Server>,
    /// Where suspended executions wait for later ticks.
    pub pending: &'a mut PendingCommandExecutionQueue<CommandSource>,
}

impl CommandBlockRunner<'_> {
    /// Vanilla `Commands.performPrefixedCommand`.
    pub(crate) fn run(&mut self, source: CommandSource, command: &str) {
        let owner = CommandExecutionOwner::capture(source.sender().clone(), self.server);
        self.server
            .perform_prefixed_command(self.pending, owner, source, command);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searge_matches_like_java_equals_ignore_case() {
        assert!(is_searge("Searge"));
        assert!(is_searge("sEARGE"));
        assert!(is_searge("\u{17F}earge"), "long s upper-cases to S");
        assert!(!is_searge("Searg"));
        assert!(!is_searge("Seargee"));
        assert!(!is_searge("say hi"));
    }
}
