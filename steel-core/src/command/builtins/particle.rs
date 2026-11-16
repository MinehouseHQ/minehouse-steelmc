//! Vanilla `ParticleCommand`.

use std::sync::Arc;

use glam::DVec3;
use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use crate::player::Player;

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;
type Context = SteelCommandContext<CommandSource>;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("particle"), |_| command())
}

/// Who sees the particles: everyone online, or the `viewers` argument.
#[derive(Clone, Copy)]
enum Viewers {
    All,
    Given,
}

fn command() -> Builder {
    literal("particle").then(
        argument("name", SteelArgumentType::particle())
            .executes(|context| send_simple(context, false))
            .then(
                argument("pos", SteelArgumentType::vec3(true))
                    .executes(|context| send_simple(context, true))
                    .then(argument("delta", SteelArgumentType::vec3(false)).then(
                        argument("speed", ArgumentType::float(0.0, f32::MAX)).then(count_tree()),
                    )),
            ),
    )
}

/// `<count> [force|normal [<viewers>]]`.
fn count_tree() -> Builder {
    argument("count", ArgumentType::integer(0, i32::MAX))
        .executes(|context| send_full(context, false, Viewers::All))
        .then(
            literal("force")
                .executes(|context| send_full(context, true, Viewers::All))
                .then(
                    argument("viewers", SteelArgumentType::players())
                        .executes(|context| send_full(context, true, Viewers::Given)),
                ),
        )
        .then(
            literal("normal")
                .executes(|context| send_full(context, false, Viewers::All))
                .then(
                    argument("viewers", SteelArgumentType::players())
                        .executes(|context| send_full(context, false, Viewers::Given)),
                ),
        )
}

/// `/particle <name> [<pos>]`: one particle, no spread, for everyone.
fn send_simple(context: &Context, has_pos: bool) -> Result<i32, CommandSyntaxError> {
    let position = if has_pos {
        context.coordinates("pos")?.position(context.source())
    } else {
        context.source().position()
    };
    let request = Request {
        position,
        delta: DVec3::ZERO,
        speed: 0.0,
        count: 0,
        force: false,
    };
    let players = context.source().server().get_players();
    send_particles(context, &request, &players)
}

fn send_full(context: &Context, force: bool, viewers: Viewers) -> Result<i32, CommandSyntaxError> {
    let request = Request {
        position: context.coordinates("pos")?.position(context.source()),
        delta: context.coordinates("delta")?.position(context.source()),
        speed: context.float("speed")?,
        count: context.integer("count")?,
        force,
    };
    let players = match viewers {
        Viewers::All => context.source().server().get_players(),
        Viewers::Given => context.players("viewers")?,
    };
    send_particles(context, &request, &players)
}

/// The particle distribution the command sends.
struct Request {
    position: DVec3,
    delta: DVec3,
    speed: f32,
    count: i32,
    force: bool,
}

/// Vanilla `ParticleCommand.sendParticles`.
fn send_particles(
    context: &Context,
    request: &Request,
    players: &[Arc<Player>],
) -> Result<i32, CommandSyntaxError> {
    let particle = context.particle("name")?;
    let world = context.source().world();
    let mut result = 0;
    for player in players {
        if world.send_particles_to(
            player,
            particle.clone(),
            request.force,
            false,
            request.position,
            request.count,
            request.delta,
            f64::from(request.speed),
        ) {
            result += 1;
        }
    }
    if result == 0 {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_PARTICLE_FAILED,
        )));
    }
    context.source().send_success(
        &translations::COMMANDS_PARTICLE_SUCCESS
            .message([particle.particle_type().key.to_string()])
            .component(),
        true,
    );
    Ok(result)
}
