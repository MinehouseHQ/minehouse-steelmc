//! Vanilla `RotateCommand`.

use steel_utils::{Identifier, translations};

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, Coordinates, SteelArgumentType, SteelCommandContext, SteelCommandRuntime,
        argument, literal,
    },
    registration::CommandRegistration,
};
use crate::entity::{EntityAnchor, SharedEntity};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("rotate"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("rotate").then(
        argument("target", SteelArgumentType::entity())
            .then(argument("rotation", SteelArgumentType::rotation()).executes(rotate_to))
            .then(
                literal("facing")
                    .then(
                        literal("entity").then(
                            argument("facingEntity", SteelArgumentType::entity())
                                .executes(|context| face_entity(context, EntityAnchor::Feet))
                                .then(
                                    argument("facingAnchor", SteelArgumentType::entity_anchor())
                                        .executes(|context| {
                                            let anchor = context.entity_anchor("facingAnchor")?;
                                            face_entity(context, anchor)
                                        }),
                                ),
                        ),
                    )
                    .then(
                        argument("facingLocation", SteelArgumentType::vec3(true))
                            .executes(face_location),
                    ),
            ),
    )
}

/// Vanilla `rotate` with a rotation: a relative part is sent as the change
/// from the target's rotation, so a player's client turns smoothly.
fn rotate_to(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let target = context.entity("target")?;
    let rotation: Coordinates = context.coordinates("rotation")?;
    let (yaw, pitch) = rotation.rotation(context.source());
    let (current_yaw, current_pitch) = target.rotation();
    let relative_yaw = rotation.is_y_relative();
    let relative_pitch = rotation.is_x_relative();
    target.force_set_rotation(
        if relative_yaw { yaw - current_yaw } else { yaw },
        relative_yaw,
        if relative_pitch {
            pitch - current_pitch
        } else {
            pitch
        },
        relative_pitch,
    );
    Ok(success(context.source(), &target))
}

/// Vanilla `LookAt.LookAtEntity.perform`.
fn face_entity(
    context: &SteelCommandContext<CommandSource>,
    anchor: EntityAnchor,
) -> Result<i32, CommandSyntaxError> {
    let target = context.entity("target")?;
    let facing = context.entity("facingEntity")?;
    target.look_at_entity(context.source().anchor(), facing.as_ref(), anchor);
    Ok(success(context.source(), &target))
}

/// Vanilla `LookAt.LookAtPosition.perform`.
fn face_location(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let target = context.entity("target")?;
    let position = context
        .coordinates("facingLocation")?
        .position(context.source());
    target.look_at(context.source().anchor(), position);
    Ok(success(context.source(), &target))
}

fn success(source: &CommandSource, target: &SharedEntity) -> i32 {
    source.send_success(
        &translations::COMMANDS_ROTATE_SUCCESS
            .message([target.display_name()])
            .component(),
        true,
    );
    1
}
