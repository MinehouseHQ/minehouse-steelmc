//! Vanilla `RideCommand`.

use std::sync::Arc;

use steel_registry::vanilla_entities;
use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("ride"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("ride").then(
        argument("target", SteelArgumentType::entity())
            .then(
                literal("mount")
                    .then(argument("vehicle", SteelArgumentType::entity()).executes(mount)),
            )
            .then(literal("dismount").executes(dismount)),
    )
}

/// Vanilla `mount`: rides even a vehicle that would refuse the target.
fn mount(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let target = context.entity("target")?;
    let vehicle = context.entity("vehicle")?;
    if let Some(current) = target.vehicle() {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_RIDE_ALREADY_RIDING
                .message([target.display_name(), current.display_name()])
                .component(),
        ));
    }
    if vehicle.entity_type() == &vanilla_entities::PLAYER {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_RIDE_MOUNT_FAILURE_CANT_RIDE_PLAYERS,
        )));
    }
    if vehicle.id() == target.id() || target.has_indirect_passenger(vehicle.as_ref()) {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_RIDE_MOUNT_FAILURE_LOOP,
        )));
    }
    let same_world = match (target.level(), vehicle.level()) {
        (Some(target_world), Some(vehicle_world)) => Arc::ptr_eq(&target_world, &vehicle_world),
        _ => false,
    };
    if !same_world {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_RIDE_MOUNT_FAILURE_WRONG_DIMENSION,
        )));
    }
    if !target.start_riding(&vehicle, true) {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_RIDE_MOUNT_FAILURE_GENERIC
                .message([target.display_name(), vehicle.display_name()])
                .component(),
        ));
    }
    context.source().send_success(
        &translations::COMMANDS_RIDE_MOUNT_SUCCESS
            .message([target.display_name(), vehicle.display_name()])
            .component(),
        true,
    );
    Ok(1)
}

/// Vanilla `dismount`.
fn dismount(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let target = context.entity("target")?;
    let Some(vehicle) = target.vehicle() else {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_RIDE_NOT_RIDING
                .message([target.display_name()])
                .component(),
        ));
    };
    target.stop_riding();
    context.source().send_success(
        &translations::COMMANDS_RIDE_DISMOUNT_SUCCESS
            .message([target.display_name(), vehicle.display_name()])
            .component(),
        true,
    );
    Ok(1)
}
