//! Vanilla `/defaultgamemode` command setting the default game mode for a domain's worlds.

use std::sync::Arc;

use steel_utils::types::GameType;
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
use super::gamemode::game_mode_translation;
use crate::world::World;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("defaultgamemode"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("defaultgamemode").then(
        argument("gamemode", SteelArgumentType::game_mode())
            .executes(set_default_game_mode)
            .then(
                argument("domain", SteelArgumentType::domain())
                    .executes(set_default_game_mode_domain),
            ),
    )
}

fn set_default_game_mode(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let domain = context.source().world().domain().to_owned();
    apply_default_game_mode(context, &domain)
}

fn set_default_game_mode_domain(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let domain = context.domain("domain")?;
    apply_default_game_mode(context, domain)
}

fn apply_default_game_mode(
    context: &SteelCommandContext<CommandSource>,
    domain: &str,
) -> Result<i32, CommandSyntaxError> {
    if !context.source().allows_execution_domain(domain) {
        return Err(CommandSyntaxError::dynamic(
            "Players cannot execute commands across Steel domains",
        ));
    }

    let game_mode = context.game_mode("gamemode")?;
    let worlds = context.source().server().worlds.worlds_in_domain(domain);

    for world in &worlds {
        world.set_default_gamemode(game_mode);
    }
    let count = if context.source().server().config.force_gamemode {
        enforce_game_mode(&worlds, game_mode)
    } else {
        0
    };

    let message = translations::COMMANDS_DEFAULTGAMEMODE_SUCCESS
        .message([TextComponent::from(game_mode_translation(game_mode))])
        .component();
    context.source().send_success(&message, true);
    Ok(count)
}

/// Vanilla `MinecraftServer.enforceGameTypeForPlayers`: moves every player in
/// `worlds` to `game_mode`, counting those whose mode changed.
fn enforce_game_mode(worlds: &[Arc<World>], game_mode: GameType) -> i32 {
    let mut players = Vec::new();
    for world in worlds {
        world.players.iter_players(|_, player| {
            players.push(Arc::clone(player));
            true
        });
    }
    let changed = players
        .iter()
        .filter(|player| player.set_game_mode(game_mode))
        .count();
    i32::try_from(changed).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use super::super::create_dispatcher;
    use crate::command::execution::SteelArgumentType;

    #[test]
    fn defaultgamemode_graph_matches_shape() {
        init_vanilla_registry();
        let Ok(dispatcher) = create_dispatcher() else {
            panic!("built-in commands should register");
        };
        let Some(root) = dispatcher.children(dispatcher.root()).and_then(|children| {
            children.iter().copied().find(|child| {
                dispatcher
                    .node(*child)
                    .is_some_and(|node| node.name() == "defaultgamemode")
            })
        }) else {
            panic!("defaultgamemode root should exist");
        };
        let Some(game_mode) = dispatcher
            .children(root)
            .and_then(|children| children.first())
            .copied()
        else {
            panic!("gamemode argument should exist");
        };
        assert!(matches!(
            dispatcher.node(game_mode),
            Some(node)
                if node.is_executable()
                    && node.argument_type() == Some(&SteelArgumentType::game_mode())
        ));
        let Some(domain) = dispatcher
            .children(game_mode)
            .and_then(|children| children.first())
            .copied()
        else {
            panic!("domain argument should exist");
        };
        assert!(matches!(
            dispatcher.node(domain),
            Some(node)
                if node.is_executable()
                    && node.argument_type() == Some(&SteelArgumentType::domain())
        ));
    }
}
