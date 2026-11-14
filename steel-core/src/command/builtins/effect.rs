//! Vanilla mob effect command (`EffectCommands`).

use std::borrow::Cow;
use std::slice;

use steel_registry::mob_effect::MobEffectRef;
use steel_utils::{Identifier, translations};
use text_components::{
    TextComponent,
    translation::{TranslatedMessage, Translation},
};

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use crate::behavior::MOB_EFFECT_BEHAVIORS;
use crate::entity::{Entity, MobEffectInstance, SharedEntity};

/// Vanilla `MobEffectInstance.INFINITE_DURATION`.
const INFINITE_DURATION: i32 = -1;
/// Vanilla default duration of a non-instant effect without `seconds`.
const DEFAULT_DURATION_TICKS: i32 = 600;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("effect"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("effect")
        .then(
            literal("clear").executes(clear_self).then(
                argument("targets", SteelArgumentType::entities())
                    .executes(clear_targets)
                    .then(
                        argument("effect", SteelArgumentType::mob_effect())
                            .executes(clear_specific),
                    ),
            ),
        )
        .then(
            literal("give").then(
                argument("targets", SteelArgumentType::entities()).then(
                    argument("effect", SteelArgumentType::mob_effect())
                        .executes(|context| give(context, None, 0, true))
                        .then(
                            argument("seconds", ArgumentType::integer(1, 1_000_000))
                                .executes(|context| {
                                    give(context, Some(context.integer("seconds")?), 0, true)
                                })
                                .then(
                                    argument("amplifier", ArgumentType::integer(0, 255))
                                        .executes(|context| {
                                            give(
                                                context,
                                                Some(context.integer("seconds")?),
                                                context.integer("amplifier")?,
                                                true,
                                            )
                                        })
                                        .then(
                                            argument("hideParticles", ArgumentType::bool())
                                                .executes(|context| {
                                                    give(
                                                        context,
                                                        Some(context.integer("seconds")?),
                                                        context.integer("amplifier")?,
                                                        !context.boolean("hideParticles")?,
                                                    )
                                                }),
                                        ),
                                ),
                        )
                        .then(
                            literal("infinite")
                                .executes(|context| give(context, Some(INFINITE_DURATION), 0, true))
                                .then(
                                    argument("amplifier", ArgumentType::integer(0, 255))
                                        .executes(|context| {
                                            give(
                                                context,
                                                Some(INFINITE_DURATION),
                                                context.integer("amplifier")?,
                                                true,
                                            )
                                        })
                                        .then(
                                            argument("hideParticles", ArgumentType::bool())
                                                .executes(|context| {
                                                    give(
                                                        context,
                                                        Some(INFINITE_DURATION),
                                                        context.integer("amplifier")?,
                                                        !context.boolean("hideParticles")?,
                                                    )
                                                }),
                                        ),
                                ),
                        ),
                ),
            ),
        )
}

/// Vanilla `EffectCommands.giveEffect`.
fn give(
    context: &SteelCommandContext<CommandSource>,
    seconds: Option<i32>,
    amplifier: i32,
    particles: bool,
) -> Result<i32, CommandSyntaxError> {
    let targets = context.entities("targets")?;
    let effect = context.mob_effect("effect")?;
    let duration = duration_in_ticks(seconds, is_instantaneous(effect));
    let source = context
        .source()
        .entity()
        .map(|entity| entity.as_ref() as &dyn Entity);

    let mut affected = Vec::new();
    for target in &targets {
        let Some(living) = target.as_living_entity() else {
            continue;
        };
        let instance =
            MobEffectInstance::with_duration(effect, duration, amplifier).with_visible(particles);
        if living.add_mob_effect_with_source(instance, source) {
            affected.push(target);
        }
    }

    let message = match affected.as_slice() {
        [] => {
            return Err(CommandSyntaxError::dynamic(TextComponent::from(
                &translations::COMMANDS_EFFECT_GIVE_FAILED,
            )));
        }
        [target] => give_message(
            &translations::COMMANDS_EFFECT_GIVE_SUCCESS_SINGLE,
            [
                effect_display_name(effect),
                target.display_name(),
                TextComponent::plain((duration / 20).to_string()),
            ],
        ),
        _ => give_message(
            &translations::COMMANDS_EFFECT_GIVE_SUCCESS_MULTIPLE,
            [
                effect_display_name(effect),
                TextComponent::plain(affected.len().to_string()),
                TextComponent::plain((duration / 20).to_string()),
            ],
        ),
    };
    context.source().send_success(&message.component(), true);
    result_count(affected.len())
}

/// A give feedback message with vanilla's three arguments; the English
/// strings leave out the trailing duration in seconds.
fn give_message(translation: &Translation<2>, args: [TextComponent; 3]) -> TranslatedMessage {
    TranslatedMessage {
        key: Cow::Borrowed(translation.0),
        args: Some(Box::new(args)),
        fallback: None,
    }
}

/// Vanilla `EffectCommands.computeDurationInTicks`.
const fn duration_in_ticks(seconds: Option<i32>, instantaneous: bool) -> i32 {
    match seconds {
        Some(seconds) if instantaneous || seconds == INFINITE_DURATION => seconds,
        Some(seconds) => seconds * 20,
        None if instantaneous => 1,
        None => DEFAULT_DURATION_TICKS,
    }
}

fn clear_self(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let Some(entity) = context.source().entity() else {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::PERMISSIONS_REQUIRES_ENTITY,
        )));
    };
    clear_all(context, slice::from_ref(entity))
}

fn clear_targets(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let targets = context.entities("targets")?;
    clear_all(context, &targets)
}

/// Vanilla `EffectCommands.clearEffects` through `LivingEntity.removeAllEffects`.
fn clear_all(
    context: &SteelCommandContext<CommandSource>,
    targets: &[SharedEntity],
) -> Result<i32, CommandSyntaxError> {
    let mut affected = Vec::new();
    for target in targets {
        let Some(living) = target.as_living_entity() else {
            continue;
        };
        let mut removed = false;
        for active in living.active_mob_effects() {
            removed |= living.remove_mob_effect(active.effect());
        }
        if removed {
            affected.push(target);
        }
    }

    let message = match affected.as_slice() {
        [] => {
            return Err(CommandSyntaxError::dynamic(TextComponent::from(
                &translations::COMMANDS_EFFECT_CLEAR_EVERYTHING_FAILED,
            )));
        }
        [target] => translations::COMMANDS_EFFECT_CLEAR_EVERYTHING_SUCCESS_SINGLE
            .message([target.display_name()]),
        _ => translations::COMMANDS_EFFECT_CLEAR_EVERYTHING_SUCCESS_MULTIPLE
            .message([TextComponent::plain(affected.len().to_string())]),
    };
    context.source().send_success(&message.component(), true);
    result_count(affected.len())
}

/// Vanilla `EffectCommands.clearEffect`.
fn clear_specific(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let targets = context.entities("targets")?;
    let effect = context.mob_effect("effect")?;
    let mut affected = Vec::new();
    for target in &targets {
        let Some(living) = target.as_living_entity() else {
            continue;
        };
        if living.remove_mob_effect(effect) {
            affected.push(target);
        }
    }

    let message = match affected.as_slice() {
        [] => {
            return Err(CommandSyntaxError::dynamic(TextComponent::from(
                &translations::COMMANDS_EFFECT_CLEAR_SPECIFIC_FAILED,
            )));
        }
        [target] => translations::COMMANDS_EFFECT_CLEAR_SPECIFIC_SUCCESS_SINGLE
            .message([effect_display_name(effect), target.display_name()]),
        _ => translations::COMMANDS_EFFECT_CLEAR_SPECIFIC_SUCCESS_MULTIPLE.message([
            effect_display_name(effect),
            TextComponent::plain(affected.len().to_string()),
        ]),
    };
    context.source().send_success(&message.component(), true);
    result_count(affected.len())
}

/// Vanilla `MobEffect.isInstantaneous`.
fn is_instantaneous(effect: MobEffectRef) -> bool {
    MOB_EFFECT_BEHAVIORS
        .get_behavior(effect)
        .as_instantaneous()
        .is_some()
}

/// Vanilla `MobEffect.getDisplayName`.
fn effect_display_name(effect: MobEffectRef) -> TextComponent {
    TextComponent::translated(TranslatedMessage {
        key: Cow::Owned(format!(
            "effect.{}.{}",
            effect.key.namespace, effect.key.path
        )),
        args: None,
        fallback: None,
    })
}

fn result_count(count: usize) -> Result<i32, CommandSyntaxError> {
    i32::try_from(count).map_err(|_| {
        CommandSyntaxError::dynamic("Affected entity count exceeds the command result range")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_follow_vanilla_seconds_rules() {
        assert_eq!(duration_in_ticks(None, false), 600);
        assert_eq!(duration_in_ticks(None, true), 1);
        assert_eq!(duration_in_ticks(Some(30), false), 600);
        assert_eq!(duration_in_ticks(Some(30), true), 30);
        assert_eq!(duration_in_ticks(Some(INFINITE_DURATION), false), -1);
    }
}
