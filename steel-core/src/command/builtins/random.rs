//! Vanilla `RandomCommand`.

use steel_utils::random::Random;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{
        ArgumentType, CommandNodeBuilder, CommandRequirement, CommandSyntaxError,
        SuggestionsBuilder,
    },
    execution::{
        CommandPermissionSource, CommandSource, IntRange, SteelArgumentType, SteelCommandContext,
        SteelCommandRuntime, SteelSuggestionContext, argument, literal, suggest_list,
    },
    registration::CommandRegistration,
};
use crate::permission::{PermissionExpr, PermissionKey, PermissionState};
use crate::random_sequences::{RandomSequences, SeedDefaults};

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;

/// The command's own permission, which vanilla's `LEVEL_GAMEMASTERS` parts
/// need granted rather than merely unset.
const ROOT_PERMISSION: &str = "minecraft.command.random";

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    // Drawing a plain value is open to everyone, like vanilla's level 0.
    CommandRegistration::new(Identifier::vanilla_static("random"), |_| command()).default_access()
}

/// Vanilla `Commands.hasPermission(LEVEL_GAMEMASTERS)` for the nested nodes.
fn game_masters() -> CommandRequirement<CommandSource> {
    let permission = PermissionKey::parse(ROOT_PERMISSION).map(PermissionExpr::key);
    CommandRequirement::authorization(move |source: &CommandSource| {
        permission.as_ref().is_ok_and(|permission| {
            source.permission_state(permission) == Some(PermissionState::Allow)
        })
    })
}

fn command() -> Builder {
    literal("random")
        .then(draw_random_value_tree("value", false))
        .then(draw_random_value_tree("roll", true))
        .then(
            literal("reset")
                .requires(game_masters())
                .then(
                    literal("*")
                        .executes(|context| Ok(reset_all_sequences(context)))
                        .then(seed_arguments(|context, settings| {
                            Ok(reset_all_sequences_and_set_new_defaults(context, settings))
                        })),
                )
                .then(
                    sequence_argument()
                        .executes(|context| reset_sequence(context, None))
                        .then(seed_arguments(|context, settings| {
                            reset_sequence(context, Some(settings))
                        })),
                ),
        )
}

/// The `seed [includeWorldSeed [includeSequenceId]]` tail shared by both
/// `reset` forms; omitted flags default to `true`.
fn seed_arguments(
    run: fn(&SteelCommandContext<CommandSource>, SeedDefaults) -> Result<i32, CommandSyntaxError>,
) -> Builder {
    argument("seed", ArgumentType::integer(i32::MIN, i32::MAX))
        .executes(move |context| {
            run(
                context,
                SeedDefaults {
                    salt: context.integer("seed")?,
                    ..SeedDefaults::default()
                },
            )
        })
        .then(
            argument("includeWorldSeed", ArgumentType::bool())
                .executes(move |context| {
                    run(
                        context,
                        SeedDefaults {
                            salt: context.integer("seed")?,
                            include_world_seed: context.boolean("includeWorldSeed")?,
                            include_sequence_id: true,
                        },
                    )
                })
                .then(
                    argument("includeSequenceId", ArgumentType::bool()).executes(move |context| {
                        run(
                            context,
                            SeedDefaults {
                                salt: context.integer("seed")?,
                                include_world_seed: context.boolean("includeWorldSeed")?,
                                include_sequence_id: context.boolean("includeSequenceId")?,
                            },
                        )
                    }),
                ),
        )
}

fn sequence_argument() -> Builder {
    argument("sequence", SteelArgumentType::identifier()).suggests(suggest_random_sequence)
}

fn draw_random_value_tree(name: &'static str, announce: bool) -> Builder {
    literal(name).then(
        argument("range", SteelArgumentType::int_range())
            .executes(move |context| random_sample(context, None, announce))
            .then(
                sequence_argument()
                    .requires(game_masters())
                    .executes(move |context| {
                        let sequence = context.identifier("sequence")?.clone();
                        random_sample(context, Some(&sequence), announce)
                    }),
            ),
    )
}

fn sequences(source: &CommandSource) -> Option<&RandomSequences> {
    source
        .server()
        .random_sequences
        .get(source.world().domain())
}

/// Vanilla `suggestRandomSequence`: the existing sequence ids as plain text.
fn suggest_random_sequence(
    context: &SteelSuggestionContext<'_, CommandSource>,
    builder: &mut SuggestionsBuilder<'_>,
) {
    if let Some(sequences) = sequences(context.source()) {
        suggest_list(builder, sequences.ids().iter().map(ToString::to_string));
    }
}

fn missing_sequences(source: &CommandSource) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(format!(
        "Domain '{}' has no random sequences",
        source.world().domain()
    ))
}

/// Vanilla `Mth.randomBetweenInclusive`.
fn random_between_inclusive(random: &mut impl Random, min: i32, max: i32) -> i32 {
    random.next_i32_bounded(max - min + 1) + min
}

fn random_sample(
    context: &SteelCommandContext<CommandSource>,
    sequence: Option<&Identifier>,
    announce: bool,
) -> Result<i32, CommandSyntaxError> {
    let range: IntRange = context.int_range("range")?;
    let min = range.minimum().unwrap_or(i32::MIN);
    let max = range.maximum().unwrap_or(i32::MAX);
    let span = i64::from(max) - i64::from(min);
    if span == 0 {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_RANDOM_ERROR_RANGE_TOO_SMALL,
        )));
    }
    if span >= i64::from(i32::MAX) {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_RANDOM_ERROR_RANGE_TOO_LARGE,
        )));
    }

    let source = context.source();
    let value = if let Some(sequence) = sequence {
        let sequences = sequences(source).ok_or_else(|| missing_sequences(source))?;
        sequences.draw(sequence, source.world().seed(), |random| {
            random_between_inclusive(random, min, max)
        })
    } else {
        // Vanilla draws from the level's unseeded `RandomSource.create()`.
        random_between_inclusive(&mut LegacyRandom::from_seed(rand::random()), min, max)
    };

    if announce {
        let message = translations::COMMANDS_RANDOM_ROLL
            .message([
                source.display_name(),
                TextComponent::plain(value.to_string()),
                TextComponent::plain(min.to_string()),
                TextComponent::plain(max.to_string()),
            ])
            .component();
        source.server().broadcast_system_chat(&message, None);
    } else {
        let message = translations::COMMANDS_RANDOM_SAMPLE_SUCCESS
            .message([TextComponent::plain(value.to_string())])
            .component();
        source.send_success(&message, false);
    }
    Ok(value)
}

/// Vanilla's two `resetSequence` overloads: without settings the sequence
/// restarts from the domain's seed defaults.
fn reset_sequence(
    context: &SteelCommandContext<CommandSource>,
    settings: Option<SeedDefaults>,
) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let sequence = context.identifier("sequence")?;
    let sequences = sequences(source).ok_or_else(|| missing_sequences(source))?;
    let world_seed = source.world().seed();
    match settings {
        Some(settings) => sequences.reset_with(sequence, world_seed, settings),
        None => sequences.reset(sequence, world_seed),
    }
    let message = translations::COMMANDS_RANDOM_RESET_SUCCESS
        .message([TextComponent::plain(sequence.to_string())])
        .component();
    source.send_success(&message, false);
    Ok(1)
}

fn reset_all_sequences(context: &SteelCommandContext<CommandSource>) -> i32 {
    let source = context.source();
    let count = sequences(source).map_or(0, RandomSequences::clear);
    send_reset_all(source, count)
}

fn reset_all_sequences_and_set_new_defaults(
    context: &SteelCommandContext<CommandSource>,
    settings: SeedDefaults,
) -> i32 {
    let source = context.source();
    let count = sequences(source).map_or(0, |sequences| {
        sequences.set_seed_defaults(settings);
        sequences.clear()
    });
    send_reset_all(source, count)
}

fn send_reset_all(source: &CommandSource, count: usize) -> i32 {
    let message = translations::COMMANDS_RANDOM_RESET_ALL_SUCCESS
        .message([TextComponent::plain(count.to_string())])
        .component();
    source.send_success(&message, false);
    i32::try_from(count).unwrap_or(i32::MAX)
}
