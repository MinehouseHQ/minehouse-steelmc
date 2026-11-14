//! Vanilla `SaveAllCommand`, `SaveOnCommand` and `SaveOffCommand`.

use steel_utils::{Identifier, translations};
use text_components::TextComponent;
use tokio::sync::oneshot;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandResultSuspension, CommandResultSuspensionPoll, CommandSource, SteelCommandContext,
        SteelCommandRuntime, literal,
    },
    registration::CommandRegistration,
};

pub(super) fn save_all_registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("save-all"), |_| {
        save_all_command()
    })
}

pub(super) fn save_on_registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("save-on"), |_| save_on_command())
}

pub(super) fn save_off_registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("save-off"), |_| {
        save_off_command()
    })
}

fn save_all_command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("save-all")
        .executes(save_all)
        .then(literal("flush").executes_suspended(save_all_flush))
}

fn save_on_command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("save-on").executes(|context| set_auto_save(context, true))
}

fn save_off_command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("save-off").executes(|context| set_auto_save(context, false))
}

/// Vanilla `SaveOnCommand` and `SaveOffCommand`.
fn set_auto_save(
    context: &SteelCommandContext<CommandSource>,
    enable: bool,
) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    if !source.server().set_auto_save(enable) {
        let already = if enable {
            &translations::COMMANDS_SAVE_ALREADY_ON
        } else {
            &translations::COMMANDS_SAVE_ALREADY_OFF
        };
        return Err(CommandSyntaxError::dynamic(TextComponent::from(already)));
    }
    let changed = if enable {
        &translations::COMMANDS_SAVE_ENABLED
    } else {
        &translations::COMMANDS_SAVE_DISABLED
    };
    source.send_success(&TextComponent::from(changed), true);
    Ok(1)
}

/// Vanilla `saveAll(source, false)`: the world is captured now, even when
/// `/save-off` is on, and written in the background like vanilla's
/// unflushed chunk writes.
#[expect(
    clippy::unnecessary_wraps,
    reason = "Command executors use a shared fallible callback signature."
)]
fn save_all(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    source.send_success(
        &TextComponent::from(&translations::COMMANDS_SAVE_SAVING),
        false,
    );
    let server = source.server();
    let save = server.prepare_save(true);
    server.spawn_save_write(save, |_| {});
    source.send_success(
        &TextComponent::from(&translations::COMMANDS_SAVE_SUCCESS),
        true,
    );
    Ok(1)
}

/// Vanilla `saveAll(source, true)`: reports once everything is on disk.
#[expect(
    clippy::unnecessary_wraps,
    reason = "Command executors use a shared fallible callback signature."
)]
fn save_all_flush(
    context: &SteelCommandContext<CommandSource>,
) -> Result<FlushedSave, CommandSyntaxError> {
    let source = context.source();
    source.send_success(
        &TextComponent::from(&translations::COMMANDS_SAVE_SAVING),
        false,
    );
    let server = source.server();
    let save = server.prepare_save(true);
    let (sender, written) = oneshot::channel();
    server.spawn_save_write(save, move |result| {
        let _ = sender.send(result.success);
    });
    Ok(FlushedSave {
        source: source.clone(),
        written,
    })
}

/// A `/save-all flush` waiting for its write.
struct FlushedSave {
    source: CommandSource,
    written: oneshot::Receiver<bool>,
}

impl CommandResultSuspension for FlushedSave {
    fn poll(&mut self) -> CommandResultSuspensionPoll {
        match self.written.try_recv() {
            Err(oneshot::error::TryRecvError::Empty) => CommandResultSuspensionPoll::Pending,
            Ok(true) => {
                self.source.send_success(
                    &TextComponent::from(&translations::COMMANDS_SAVE_SUCCESS),
                    true,
                );
                CommandResultSuspensionPoll::Ready(Ok(1))
            }
            Ok(false) | Err(oneshot::error::TryRecvError::Closed) => {
                CommandResultSuspensionPoll::Ready(Err(CommandSyntaxError::dynamic(
                    TextComponent::from(&translations::COMMANDS_SAVE_FAILED),
                )))
            }
        }
    }
}
