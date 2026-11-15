//! Brigadier's `CommandDispatcherUsageTest` smart usage cases.

use super::super::{
    ArgumentType, CommandDispatcher, CommandNodeBuilder, CommandRequirement, NodeId, argument,
    literal,
};

fn command<S>(builder: CommandNodeBuilder<S>) -> CommandNodeBuilder<S> {
    builder.executes(|_| Ok(0))
}

fn denied(builder: CommandNodeBuilder<()>) -> CommandNodeBuilder<()> {
    builder.requires(CommandRequirement::contextual(|(): &()| false))
}

fn register(dispatcher: &mut CommandDispatcher<()>, builder: CommandNodeBuilder<()>) -> NodeId {
    let Ok(node) = dispatcher.register(builder) else {
        panic!("command registration should succeed");
    };
    node
}

/// Brigadier's usage test graph.
fn dispatcher() -> (CommandDispatcher<()>, NodeId) {
    let mut dispatcher = CommandDispatcher::new();
    register(
        &mut dispatcher,
        literal("a")
            .then(
                literal("1")
                    .then(command(literal("i")))
                    .then(command(literal("ii"))),
            )
            .then(
                literal("2")
                    .then(command(literal("i")))
                    .then(command(literal("ii"))),
            ),
    );
    register(&mut dispatcher, literal("b").then(command(literal("1"))));
    register(&mut dispatcher, command(literal("c")));
    register(&mut dispatcher, denied(command(literal("d"))));
    register(
        &mut dispatcher,
        command(literal("e")).then(
            command(literal("1"))
                .then(command(literal("i")))
                .then(command(literal("ii"))),
        ),
    );
    register(
        &mut dispatcher,
        literal("f")
            .then(
                literal("1")
                    .then(command(literal("i")))
                    .then(denied(command(literal("ii")))),
            )
            .then(
                literal("2")
                    .then(denied(command(literal("i"))))
                    .then(command(literal("ii"))),
            ),
    );
    register(
        &mut dispatcher,
        command(literal("g")).then(literal("1").then(command(literal("i")))),
    );
    let h = register(
        &mut dispatcher,
        command(literal("h"))
            .then(literal("1").then(command(literal("i"))))
            .then(literal("2").then(literal("i").then(command(literal("ii")))))
            .then(command(literal("3"))),
    );
    register(
        &mut dispatcher,
        command(literal("i"))
            .then(command(literal("1")))
            .then(command(literal("2"))),
    );
    let root = dispatcher.root();
    register(&mut dispatcher, literal("j").redirects(root));
    register(&mut dispatcher, literal("k").redirects(h));
    (dispatcher, h)
}

fn lines(dispatcher: &CommandDispatcher<()>, node: NodeId) -> Vec<String> {
    dispatcher
        .smart_usage(node, &())
        .into_iter()
        .map(|(_, usage)| usage)
        .collect()
}

#[test]
fn root_usage_has_a_line_per_usable_command() {
    let (dispatcher, _) = dispatcher();
    assert_eq!(
        lines(&dispatcher, dispatcher.root()),
        [
            "a (1|2)",
            "b 1",
            "c",
            "e [1]",
            "f (1|2)",
            "g [1]",
            "h [1|2|3]",
            "i [1|2]",
            "j ...",
            "k -> h",
        ]
    );
}

#[test]
fn usage_of_an_executable_command_marks_its_children_optional() {
    let (dispatcher, h) = dispatcher();
    let usage = dispatcher.smart_usage(h, &());
    let Some(children) = dispatcher.children(h) else {
        panic!("h should have children");
    };
    assert_eq!(
        usage.iter().map(|(node, _)| *node).collect::<Vec<_>>(),
        children
    );
    assert_eq!(lines(&dispatcher, h), ["[1] i", "[2] i ii", "[3]"]);
}

#[test]
fn arguments_show_in_angle_brackets_and_denied_children_drop_out() {
    let mut dispatcher = CommandDispatcher::new();
    register(
        &mut dispatcher,
        literal("x")
            .then(command(literal("open")))
            .then(denied(command(literal("secret")))),
    );
    register(
        &mut dispatcher,
        literal("y").then(command(argument("count", ArgumentType::integer(0, 10)))),
    );
    assert_eq!(
        lines(&dispatcher, dispatcher.root()),
        ["x open", "y <count>"]
    );
}
