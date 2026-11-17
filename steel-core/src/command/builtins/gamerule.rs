use steel_registry::{
    REGISTRY,
    game_rules::{ErasedGameRuleRef, GameRuleType, GameRuleValue},
};
use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{CommandSource, SteelCommandContext, SteelCommandRuntime, argument, literal},
    registration::CommandRegistration,
};
use crate::permission::{PermissionContext, PermissionExpr, PermissionKey};
use crate::player::Player;
use crate::world::World;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("gamerule"), |_| command())
}

/// Whether `player` may read every game rule value from the client, which
/// vanilla grants to gamemasters, the same audience as `/gamerule`.
pub(crate) fn player_can_request_game_rule_values(player: &Player, world: &World) -> bool {
    let permission = match PermissionKey::parse("minecraft.command.gamerule") {
        Ok(key) => PermissionExpr::key(key),
        Err(error) => {
            log::error!("invalid built-in gamerule permission key: {error}");
            return false;
        }
    };
    player.has_permission_in(
        &permission,
        &PermissionContext::for_world(world.key.clone()),
    )
}

/// Vanilla `ServerGamePacketListenerImpl.sendGameRuleValues`: every game rule
/// of `world` with its value serialized like `GameRule.serialize`.
pub(crate) fn game_rule_values(world: &World) -> Vec<(Identifier, String)> {
    REGISTRY
        .game_rules
        .iter()
        .map(|(_, rule)| {
            (
                rule.key().clone(),
                world.get_erased_game_rule(rule).to_string(),
            )
        })
        .collect()
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    let mut command = literal("gamerule");
    for (_, rule) in REGISTRY.game_rules.iter() {
        // Vanilla's short identifier only omits the `minecraft` namespace.
        if rule.key().namespace == Identifier::VANILLA_NAMESPACE {
            command = command.then(rule_literal(rule.key().path.to_string(), rule));
        }
        command = command.then(rule_literal(rule.key().to_string(), rule));
    }
    command
}

fn rule_literal(
    name: String,
    rule: ErasedGameRuleRef,
) -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    match rule.value_type() {
        GameRuleType::Bool => literal(name)
            .executes(move |context| query_rule(context, rule))
            .then(
                argument("value", ArgumentType::bool())
                    .executes(move |context| set_bool_rule(context, rule)),
            ),
        GameRuleType::Int => {
            let minimum = rule.min_value().unwrap_or(i32::MIN);
            let maximum = rule.max_value().unwrap_or(i32::MAX);
            literal(name)
                .executes(move |context| query_rule(context, rule))
                .then(
                    argument("value", ArgumentType::integer(minimum, maximum))
                        .executes(move |context| set_int_rule(context, rule)),
                )
        }
    }
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "Command executors use a shared fallible callback signature."
)]
fn query_rule(
    context: &SteelCommandContext<CommandSource>,
    rule: ErasedGameRuleRef,
) -> Result<i32, CommandSyntaxError> {
    let value = context.source().world().get_erased_game_rule(rule);
    let message = translations::COMMANDS_GAMERULE_QUERY
        .message([
            TextComponent::from(rule_display_name(rule)),
            TextComponent::from(value.to_string()),
        ])
        .component();
    context.source().send_success(&message, false);
    Ok(rule.erased_command_result(&value))
}

fn set_bool_rule(
    context: &SteelCommandContext<CommandSource>,
    rule: ErasedGameRuleRef,
) -> Result<i32, CommandSyntaxError> {
    let value = context.boolean("value")?;
    set_rule(context, rule, GameRuleValue::new(value))
}

fn set_int_rule(
    context: &SteelCommandContext<CommandSource>,
    rule: ErasedGameRuleRef,
) -> Result<i32, CommandSyntaxError> {
    let value = context.integer("value")?;
    set_rule(context, rule, GameRuleValue::new(value))
}

fn set_rule(
    context: &SteelCommandContext<CommandSource>,
    rule: ErasedGameRuleRef,
    value: GameRuleValue,
) -> Result<i32, CommandSyntaxError> {
    let result = rule.erased_command_result(&value);
    let serialized_value = value.to_string();
    if !context.source().world().set_erased_game_rule(rule, value) {
        return Err(CommandSyntaxError::dynamic(format!(
            "Parsed value does not match game rule {}",
            rule.key()
        )));
    }

    let message = translations::COMMANDS_GAMERULE_SET
        .message([
            TextComponent::from(rule_display_name(rule)),
            TextComponent::from(serialized_value),
        ])
        .component();
    context.source().send_success(&message, true);
    Ok(result)
}

fn rule_display_name(rule: ErasedGameRuleRef) -> String {
    if rule.key().namespace == Identifier::VANILLA_NAMESPACE {
        rule.key().path.to_string()
    } else {
        rule.key().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::super::create_dispatcher;
    use crate::command::{
        brigadier::{ArgumentType, CommandDispatcher, NodeId},
        execution::{CommandSource, SteelArgumentType, SteelCommandRuntime},
    };
    use steel_registry::init_vanilla_registry;

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
            panic!("child `{name}` should exist");
        };
        child
    }

    #[test]
    fn vanilla_rules_have_short_and_qualified_literals() {
        init_vanilla_registry();
        let Ok(dispatcher) = create_dispatcher() else {
            panic!("built-in commands should register");
        };
        let gamerule = child(&dispatcher, dispatcher.root(), "gamerule");
        let short = child(&dispatcher, gamerule, "keep_inventory");
        let qualified = child(&dispatcher, gamerule, "minecraft:keep_inventory");

        for rule in [short, qualified] {
            let Some(rule_node) = dispatcher.node(rule) else {
                panic!("gamerule literal should exist");
            };
            assert!(rule_node.is_executable());
            let value = child(&dispatcher, rule, "value");
            assert_eq!(
                dispatcher.node(value).and_then(|node| node.argument_type()),
                Some(&SteelArgumentType::from(ArgumentType::bool()))
            );
        }
    }

    #[test]
    fn integer_rule_bounds_are_retained_in_the_graph() {
        init_vanilla_registry();
        let Ok(dispatcher) = create_dispatcher() else {
            panic!("built-in commands should register");
        };
        let gamerule = child(&dispatcher, dispatcher.root(), "gamerule");
        let rule = child(&dispatcher, gamerule, "max_command_forks");
        let value = child(&dispatcher, rule, "value");

        assert_eq!(
            dispatcher.node(value).and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::from(ArgumentType::integer(0, i32::MAX)))
        );
    }
}

#[cfg(test)]
mod value_tests {
    use steel_registry::{REGISTRY, vanilla_game_rules};
    use steel_utils::Identifier;

    use super::game_rule_values;
    use crate::bootstrap::init_globals;
    use crate::test_support::fresh_test_world;

    #[test]
    fn game_rule_values_serialize_every_rule_like_vanilla() {
        init_globals();
        let fixture = fresh_test_world("game_rule_values");
        let world = &fixture.world;
        assert!(world.set_game_rule(&vanilla_game_rules::KEEP_INVENTORY, true));
        assert!(world.set_game_rule(&vanilla_game_rules::RESPAWN_RADIUS, 3));

        let values = game_rule_values(world);

        assert_eq!(values.len(), REGISTRY.game_rules.iter().count());
        let value = |key: &'static str| {
            values
                .iter()
                .find(|(rule, _)| *rule == Identifier::vanilla_static(key))
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(value("keep_inventory"), Some("true"));
        assert_eq!(value("respawn_radius"), Some("3"));
    }
}
