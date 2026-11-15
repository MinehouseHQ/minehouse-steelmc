//! `/mhgive <item> [count]`: an item of ours into the hands of the player who asks.

use steel_core::{
    command::{
        CommandArgument, CommandContext, CommandError, CommandNode, CommandRegistration,
        CommandRegistrationError, CommandRegistry, argument, literal,
    },
    player::player_inventory::Prediction,
};
use steel_utils::Identifier;

use super::{CustomItem, ITEMS};
use crate::moderation::format::render;

pub(super) fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    registry.register(CommandRegistration::new(
        Identifier::new("minehouse", "mhgive"),
        tree,
    ))?;
    Ok(())
}

fn tree() -> CommandNode {
    ITEMS.iter().copied().fold(literal("mhgive"), |root, item| {
        root.then(
            literal(item.short_id())
                .executes(move |context| give(*context, item, 1))
                .then(
                    argument("count", CommandArgument::integer(1, 64 * 36)).executes(
                        move |context| {
                            let count = context.integer("count")?;
                            give(*context, item, count)
                        },
                    ),
                ),
        )
    })
}

fn give(
    context: CommandContext<'_>,
    item: &'static CustomItem,
    count: i32,
) -> Result<i32, CommandError> {
    let source = context.source();
    let Some(player) = source.player() else {
        return Err(CommandError::from("Предметы выдаются только игроку."));
    };
    let mut left = count;
    while left > 0 {
        let size = left.min(item.max_stack);
        left -= size;
        player.add_item_or_drop(item.stack(size), Prediction::ServerOnly);
    }
    source.send_success(
        &render(
            "&7Выдано: &f{count} × {name}",
            &[("count", &count.to_string()), ("name", item.name)],
        ),
        true,
    );
    Ok(count)
}
