//! Entity summoning command.

use std::sync::Arc;

use glam::DVec3;
use simdnbt::owned::NbtCompound;
use steel_registry::entity_type::EntityTypeRef;
use steel_utils::{BlockPos, Identifier, translations, types::Difficulty};
use text_components::{TextComponent, translation::Translation};

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use crate::{
    entity::{AddEntityError, EntitySpawnReason, SharedEntity, load_entity_recursive_owned},
    world::World,
};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("summon"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("summon").then(
        argument("entity", SteelArgumentType::summonable_entity())
            .executes(|context| {
                summon_entity(
                    context,
                    context.source().position(),
                    &NbtCompound::new(),
                    true,
                )
            })
            .then(
                argument("pos", SteelArgumentType::vec3(true))
                    .executes(|context| {
                        let position = context.coordinates("pos")?.position(context.source());
                        summon_entity(context, position, &NbtCompound::new(), true)
                    })
                    .then(
                        argument("nbt", SteelArgumentType::compound_tag()).executes(|context| {
                            let position = context.coordinates("pos")?.position(context.source());
                            summon_entity(context, position, context.compound_tag("nbt")?, false)
                        }),
                    ),
            ),
    )
}

fn summon_entity(
    context: &SteelCommandContext<CommandSource>,
    position: DVec3,
    nbt: &NbtCompound,
    finalize: bool,
) -> Result<i32, CommandSyntaxError> {
    let entity_type = context.entity_type("entity")?;
    let entity = create_entity(context, entity_type, position, nbt, finalize)?;
    let message = translations::COMMANDS_SUMMON_SUCCESS
        .message([entity.display_name()])
        .component();
    context.source().send_success(&message, true);
    Ok(1)
}

/// Vanilla `SummonCommand.createEntity`: loads `nbt` as the entity, with
/// passengers, at `position`. Only an entity summoned without data is
/// finalized like a natural spawn.
pub(super) fn create_entity(
    context: &SteelCommandContext<CommandSource>,
    entity_type: EntityTypeRef,
    position: DVec3,
    nbt: &NbtCompound,
    finalize: bool,
) -> Result<SharedEntity, CommandSyntaxError> {
    if !World::is_in_spawnable_bounds(BlockPos::from(position)) {
        return Err(command_failed(
            &translations::COMMANDS_SUMMON_INVALID_POSITION,
        ));
    }

    let world = context.source().world();
    if world.difficulty() == Difficulty::Peaceful && !entity_type.allowed_in_peaceful {
        return Err(command_failed(
            &translations::COMMANDS_SUMMON_FAILED_PEACEFUL,
        ));
    }

    let mut entity_tag = nbt.clone();
    entity_tag.insert("id", entity_type.key.to_string());
    let Some(loaded) =
        load_entity_recursive_owned(world, &entity_tag, EntitySpawnReason::Command, |entity| {
            let (yaw, pitch) = entity.rotation();
            entity.snap_to(position, yaw, pitch);
        })
    else {
        return Err(command_failed(&translations::COMMANDS_SUMMON_FAILED));
    };
    let entity = Arc::clone(&loaded.root);

    if finalize && let Some(mob) = entity.as_mob() {
        let _ = mob.finalize_spawn(world, EntitySpawnReason::Command, None);
    }

    match world.try_add_fresh_entity_with_passengers(Arc::clone(&entity)) {
        Ok(()) => Ok(entity),
        Err(AddEntityError::DuplicateUuid { .. }) => {
            Err(command_failed(&translations::COMMANDS_SUMMON_FAILED_UUID))
        }
        Err(_) => Err(command_failed(&translations::COMMANDS_SUMMON_FAILED)),
    }
}

fn command_failed(translation: &'static Translation<0>) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(translation))
}

#[cfg(test)]
mod tests {
    use super::super::create_dispatcher;
    use crate::bootstrap::init_globals;
    use crate::command::{
        brigadier::{CommandDispatcher, NodeId},
        execution::{CommandSource, SteelArgumentType, SteelCommandRuntime},
    };

    type Dispatcher = CommandDispatcher<CommandSource, SteelCommandRuntime>;

    fn child(dispatcher: &Dispatcher, parent: NodeId, name: &str) -> NodeId {
        let Some(children) = dispatcher.children(parent) else {
            panic!("parent node should exist");
        };
        let Some(child) = children.iter().copied().find(|child| {
            dispatcher
                .node(*child)
                .is_some_and(|node| node.name() == name)
        }) else {
            panic!("child {name} should exist");
        };
        child
    }

    #[test]
    fn summon_graph_uses_typed_entity_position_and_data_arguments() {
        init_globals();
        let Ok(dispatcher) = create_dispatcher() else {
            panic!("built-in commands should register");
        };
        let root = child(&dispatcher, dispatcher.root(), "summon");
        let entity = child(&dispatcher, root, "entity");
        assert_eq!(
            dispatcher
                .node(entity)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::summonable_entity())
        );
        assert!(matches!(
            dispatcher.node(entity),
            Some(node) if node.is_executable()
        ));

        let position = child(&dispatcher, entity, "pos");
        assert_eq!(
            dispatcher
                .node(position)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::vec3(true))
        );
        assert!(matches!(
            dispatcher.node(position),
            Some(node) if node.is_executable()
        ));
        let nbt = child(&dispatcher, position, "nbt");
        assert_eq!(
            dispatcher.node(nbt).and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::compound_tag())
        );
        assert!(matches!(dispatcher.node(nbt), Some(node) if node.is_executable()));
    }
}
