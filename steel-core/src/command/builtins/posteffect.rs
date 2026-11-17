//! Vanilla `PostEffectCommand`.

use std::sync::Arc;

use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, ElementType, ResponseTracker, SteelArgumentType, SteelCommandContext,
        SteelCommandRuntime, argument, literal,
    },
    registration::CommandRegistration,
};
use crate::entity::Entity as _;
use crate::player::Player;

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;
type Context = SteelCommandContext<CommandSource>;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("posteffect"), |_| command())
}

fn command() -> Builder {
    literal("posteffect")
        .then(
            literal("add").then(argument("targets", SteelArgumentType::players()).then(
                argument("posteffect", SteelArgumentType::post_effect()).executes(add_post_effect),
            )),
        )
        .then(
            literal("clear").then(
                argument("targets", SteelArgumentType::players()).executes(clear_post_effect),
            ),
        )
        .then(
            literal("list")
                .then(argument("target", SteelArgumentType::player()).executes(list_post_effects)),
        )
        .then(
            literal("remove").then(
                argument("targets", SteelArgumentType::players()).then(
                    argument("posteffect", SteelArgumentType::post_effect())
                        .executes(remove_post_effect),
                ),
            ),
        )
}

/// Vanilla `addPostEffect`.
fn add_post_effect(context: &Context) -> Result<i32, CommandSyntaxError> {
    let players = context.players("targets")?;
    let effect = context.identifier("posteffect")?.clone();
    let mut tracker = ResponseTracker::default();
    for player in &players {
        tracker.track(player, i32::from(player.add_post_effect(effect.clone())));
    }
    let name = effect.to_string();
    tracker.send_feedback(
        context.source(),
        true,
        ElementType::NonZero,
        Some(add_failed),
        |player: &Arc<Player>, _| {
            translations::COMMANDS_POSTEFFECT_ADD_SUCCESS_SINGLE
                .message([TextComponent::plain(name.clone()), player.display_name()])
                .component()
        },
        |count, _| {
            translations::COMMANDS_POSTEFFECT_ADD_SUCCESS_MULTIPLE
                .message([TextComponent::plain(name.clone()), plain(count)])
                .component()
        },
    )
}

/// Vanilla `clearPostEffect`.
fn clear_post_effect(context: &Context) -> Result<i32, CommandSyntaxError> {
    let players = context.players("targets")?;
    let mut tracker = ResponseTracker::default();
    for player in &players {
        tracker.track(player, i32::from(player.clear_post_effects()));
    }
    tracker.send_feedback(
        context.source(),
        true,
        ElementType::NonZero,
        Some(clear_failed),
        |player: &Arc<Player>, _| {
            translations::COMMANDS_POSTEFFECT_CLEAR_SUCCESS_SINGLE
                .message([player.display_name()])
                .component()
        },
        |count, _| {
            translations::COMMANDS_POSTEFFECT_CLEAR_SUCCESS_MULTIPLE
                .message([plain(count)])
                .component()
        },
    )
}

/// Vanilla `listPostEffects`: returns how many post effects are applied.
fn list_post_effects(context: &Context) -> Result<i32, CommandSyntaxError> {
    let player = context.player("target")?;
    let effects = player.post_effects();
    let message = if effects.is_empty() {
        translations::COMMANDS_POSTEFFECT_LIST_EMPTY
            .message([player.display_name()])
            .component()
    } else {
        let names = effects
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        translations::COMMANDS_POSTEFFECT_LIST_SUCCESS
            .message([
                player.display_name(),
                plain(i32::try_from(effects.len()).unwrap_or(i32::MAX)),
                TextComponent::plain(names),
            ])
            .component()
    };
    context.source().send_success(&message, false);
    Ok(i32::try_from(effects.len()).unwrap_or(i32::MAX))
}

/// Vanilla `removePostEffect`.
fn remove_post_effect(context: &Context) -> Result<i32, CommandSyntaxError> {
    let players = context.players("targets")?;
    let effect = context.identifier("posteffect")?.clone();
    let mut tracker = ResponseTracker::default();
    for player in &players {
        tracker.track(player, i32::from(player.remove_post_effect(&effect)));
    }
    let name = effect.to_string();
    tracker.send_feedback(
        context.source(),
        true,
        ElementType::NonZero,
        Some(remove_failed),
        |player: &Arc<Player>, _| {
            translations::COMMANDS_POSTEFFECT_REMOVE_SUCCESS_SINGLE
                .message([TextComponent::plain(name.clone()), player.display_name()])
                .component()
        },
        |count, _| {
            translations::COMMANDS_POSTEFFECT_REMOVE_SUCCESS_MULTIPLE
                .message([TextComponent::plain(name.clone()), plain(count)])
                .component()
        },
    )
}

fn plain(value: i32) -> TextComponent {
    TextComponent::plain(value.to_string())
}

fn add_failed() -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(
        &translations::COMMANDS_POSTEFFECT_ADD_FAILED,
    ))
}

fn clear_failed() -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(
        &translations::COMMANDS_POSTEFFECT_CLEAR_FAILED,
    ))
}

fn remove_failed() -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(
        &translations::COMMANDS_POSTEFFECT_REMOVE_FAILED,
    ))
}
