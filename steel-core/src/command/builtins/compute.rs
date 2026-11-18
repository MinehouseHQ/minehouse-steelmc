//! Vanilla `ComputeCommand` and the `LootContextSources` it is built on:
//! `compute (default | block <pos> | entity <target>) (integer | float)`.

use steel_utils::java::float_to_string;
use steel_utils::{BlockStateId, Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandArgumentSource as _, CommandSource, SteelArgumentType, SteelCommandContext,
        SteelCommandRuntime, argument, literal,
    },
    registration::CommandRegistration,
};
use super::execute::loaded_block_position;
use crate::entity::SharedEntity;
use crate::loot::{Holder, LootContext, LootRegistries};

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;
type Context = SteelCommandContext<CommandSource>;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("compute"), |_| command())
}

/// Vanilla `LootContextSources.addContextSources`: which extra parameters the
/// loot context gets.
#[derive(Clone, Copy)]
enum ContextSource {
    /// `default`: `LootContextParamSets.COMMAND_COMPUTE_DEFAULT`.
    Default,
    /// `block <computePos>`: adds the block state.
    Block,
    /// `entity <computeTarget>`: adds the target entity.
    Entity,
}

fn command() -> Builder {
    literal("compute")
        .then(
            literal("default")
                .then(literal("float").then(float_branch(ContextSource::Default)))
                .then(literal("integer").then(int_branch(ContextSource::Default))),
        )
        .then(
            literal("block").then(
                argument("computePos", SteelArgumentType::block_pos())
                    .then(literal("float").then(float_branch(ContextSource::Block)))
                    .then(literal("integer").then(int_branch(ContextSource::Block))),
            ),
        )
        .then(
            literal("entity").then(
                argument("computeTarget", SteelArgumentType::entity())
                    .then(literal("float").then(float_branch(ContextSource::Entity)))
                    .then(literal("integer").then(int_branch(ContextSource::Entity))),
            ),
        )
}

fn float_branch(source: ContextSource) -> Builder {
    argument("provider", SteelArgumentType::float_provider())
        .executes(move |context| compute_as_float(context, source, 1.0))
        .then(
            argument("scale", ArgumentType::float(f32::MIN, f32::MAX)).executes(move |context| {
                compute_as_float(context, source, context.float("scale")?)
            }),
        )
}

fn int_branch(source: ContextSource) -> Builder {
    argument("provider", SteelArgumentType::int_provider())
        .executes(move |context| compute_as_int(context, source))
}

/// The parameters `ContextDecorator.createParams` adds to the command's own
/// (`THIS_ENTITY` and `ORIGIN`).
struct Parameters {
    block_state: Option<BlockStateId>,
    target: Option<SharedEntity>,
}

fn parameters(context: &Context, source: ContextSource) -> Result<Parameters, CommandSyntaxError> {
    Ok(match source {
        ContextSource::Default => Parameters {
            block_state: None,
            target: None,
        },
        ContextSource::Block => {
            let pos = loaded_block_position(context, "computePos")?;
            // TODO: Pass the block entity once loot contexts carry
            // `BLOCK_ENTITY`; block predicates on its data never match.
            Parameters {
                block_state: Some(context.source().world().get_block_state(pos)),
                target: None,
            }
        }
        ContextSource::Entity => Parameters {
            block_state: None,
            target: Some(context.entity("computeTarget")?),
        },
    })
}

/// The loot context a command builds from its source: `ORIGIN` at the
/// source's position and `THIS_ENTITY` for its entity, with the domain's
/// registries and command storage.
pub(super) fn command_loot_context<'a>(
    source: &'a CommandSource,
    registries: &'a LootRegistries,
) -> LootContext<'a> {
    let mut ctx = LootContext::new(source.world());
    ctx.origin = Some(source.position());
    ctx.this = source.entity().map(AsRef::as_ref);
    ctx.registries = Some(registries);
    ctx.storage = source.server().command_storage.get(source.world().domain());
    ctx
}

/// Vanilla `ComputeCommand.computeAsInt`.
fn compute_as_int(context: &Context, source: ContextSource) -> Result<i32, CommandSyntaxError> {
    let provider = context.int_provider("provider")?;
    let parameters = parameters(context, source)?;
    let registries = context.source().loot_registries();
    let mut ctx = command_loot_context(context.source(), &registries);
    ctx.block_state = parameters.block_state;
    ctx.target_entity = parameters.target.as_deref();
    let result = provider
        .value(&ctx)
        .ok_or_else(|| invalid_value(&provider, "Unbound value".to_owned()))?
        .get_int_unsafe(&ctx)
        .map_err(|error| invalid_value(&provider, error.0))?;
    print_exact(context.source(), &provider, result);
    Ok(result)
}

/// Vanilla `ComputeCommand.computeAsFloat`: the value times `scale`,
/// floored.
#[expect(clippy::float_cmp, reason = "vanilla compares the exact value")]
fn compute_as_float(
    context: &Context,
    source: ContextSource,
    scale: f32,
) -> Result<i32, CommandSyntaxError> {
    let provider = context.float_provider("provider")?;
    let parameters = parameters(context, source)?;
    let registries = context.source().loot_registries();
    let mut ctx = command_loot_context(context.source(), &registries);
    ctx.block_state = parameters.block_state;
    ctx.target_entity = parameters.target.as_deref();
    let original = provider
        .value(&ctx)
        .ok_or_else(|| invalid_value(&provider, "Unbound value".to_owned()))?
        .get_float_unsafe(&ctx)
        .map_err(|error| invalid_value(&provider, error.0))?;
    if !original.is_finite() {
        return Err(invalid_value(&provider, float_to_string(original)));
    }
    // Vanilla `Mth.floor(float)`: through an int, which saturates.
    let result = (original * scale).floor() as i32;
    if result as f32 == original {
        print_exact(context.source(), &provider, result);
    } else {
        print_rounded(context.source(), &provider, result, original);
    }
    Ok(result)
}

fn print_exact<T>(source: &CommandSource, provider: &Holder<T>, result: i32) {
    let result = TextComponent::from(result.to_string());
    let message = match provider.key() {
        Some(key) => translations::COMMAND_COMPUTE_RESULT_NAMED_EXACT
            .message([TextComponent::from(key.to_string()), result])
            .component(),
        None => translations::COMMAND_COMPUTE_RESULT_UNNAMED_EXACT
            .message([result])
            .component(),
    };
    source.send_success(&message, false);
}

fn print_rounded<T>(source: &CommandSource, provider: &Holder<T>, result: i32, original: f32) {
    let result = TextComponent::from(result.to_string());
    let original = TextComponent::from(float_to_string(original));
    let message = match provider.key() {
        Some(key) => translations::COMMAND_COMPUTE_RESULT_NAMED_ROUNDED
            .message([TextComponent::from(key.to_string()), original, result])
            .component(),
        None => translations::COMMAND_COMPUTE_RESULT_UNNAMED_ROUNDED
            .message([original, result])
            .component(),
    };
    source.send_success(&message, false);
}

/// Vanilla `ComputeCommand.throwInvalidValue`.
fn invalid_value<T>(provider: &Holder<T>, value: String) -> CommandSyntaxError {
    let message = match provider.key() {
        Some(key) => translations::COMMAND_COMPUTE_RESULT_NAMED_INVALID
            .message([key.to_string(), value])
            .component(),
        None => translations::COMMAND_COMPUTE_RESULT_UNNAMED_INVALID
            .message([value])
            .component(),
    };
    CommandSyntaxError::dynamic(message)
}
