//! Vanilla `ItemCommands`: `item replace|fill|override` put items into
//! entity and container slots, given directly or copied from other slots.
//!
//! `item modify` and the `modifier` argument after `from` are not registered:
//! both take an item modifier (a loot function by id or inline), and Steel has
//! no `item_modifier` registry or runtime loot function decoding yet.

mod provider;
#[cfg(test)]
mod tests;

use steel_registry::item_stack::ItemStack;
use steel_utils::{BlockPos, Identifier, translations};
use text_components::TextComponent;
use text_components::translation::Translation;

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use super::execute::loaded_block_position;
use crate::behavior::ITEM_BEHAVIORS;
use crate::entity::SharedEntity;
use crate::inventory::lock::ContainerRef;
use crate::inventory::slot_access::SlotAccess;
use crate::inventory::slot_range::SlotRange;
use provider::{Distributor, ItemProvider};

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;
type Context = SteelCommandContext<CommandSource>;

/// The largest `count` of `with <item> [count]`.
const MAX_WITH_COUNT: i32 = 99;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("item"), |_| command())
}

/// Vanilla `ItemCommands.register`, in its provider and distributor order.
fn command() -> Builder {
    let mut root = literal("item");
    for target in AccessorKind::ALL {
        for distributor in Distributor::ALL {
            root = root.then(target.wrap(
                literal(distributor.literal()),
                "target",
                move |target_node| target_node.then(slots_node(target, distributor)),
            ));
        }
    }
    root
}

/// Vanilla `wrapSetItems`: `<slots>` then `from <source> <sourceSlots>` or
/// `with <item> [<count>]`.
fn slots_node(target: AccessorKind, distributor: Distributor) -> Builder {
    let mut slots = argument("slots", SteelArgumentType::slot_source());
    for source in AccessorKind::ALL {
        slots = slots.then(source.wrap(literal("from"), "source", move |source_node| {
            source_node.then(
                argument("sourceSlots", SteelArgumentType::slot_source()).executes(
                    move |context| {
                        let items = get_items(context, source)?;
                        set_items(context, target, distributor.provider(items), None)
                    },
                ),
            )
        }));
    }
    slots.then(
        literal("with").then(
            argument("item", SteelArgumentType::item_stack())
                .executes(move |context| set_item(context, target, distributor, 1))
                .then(
                    argument("count", ArgumentType::integer(1, MAX_WITH_COUNT)).executes(
                        move |context| {
                            let count = context.integer("count")?;
                            set_item(context, target, distributor, count)
                        },
                    ),
                ),
        ),
    )
}

/// Which of vanilla's item accessor providers a node reads or writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AccessorKind {
    Entity,
    Block,
}

impl AccessorKind {
    /// Vanilla `ItemCommands.ALL_PROVIDERS` order.
    const ALL: [Self; 2] = [Self::Entity, Self::Block];

    /// Vanilla `ArgProvider.wrap`: `<builder> <kind> <argument> ...`.
    fn wrap(
        self,
        builder: Builder,
        argument_name: &'static str,
        then: impl FnOnce(Builder) -> Builder,
    ) -> Builder {
        let (literal_name, argument_node) = match self {
            Self::Entity => (
                "entity",
                argument(argument_name, SteelArgumentType::entities()),
            ),
            Self::Block => (
                "block",
                argument(argument_name, SteelArgumentType::block_pos()),
            ),
        };
        builder.then(literal(literal_name).then(then(argument_node)))
    }
}

/// A resolved vanilla `ItemAccessor`.
enum ItemAccessor {
    Entity(Vec<SharedEntity>),
    Block {
        pos: BlockPos,
        container: ContainerRef,
    },
}

impl ItemAccessor {
    /// The provider's `access`, with `BlockItemAccessor.getContainer` run
    /// up front: a block without a container fails with `not_a_container`.
    fn access(
        context: &Context,
        kind: AccessorKind,
        argument_name: &str,
        not_a_container: &Translation<3>,
    ) -> Result<Self, CommandSyntaxError> {
        match kind {
            AccessorKind::Entity => context.entities(argument_name).map(Self::Entity),
            AccessorKind::Block => {
                let pos = loaded_block_position(context, argument_name)?;
                let container = context
                    .source()
                    .world()
                    .get_block_entity(pos)
                    .and_then(ContainerRef::from_block_entity)
                    .ok_or_else(|| {
                        CommandSyntaxError::dynamic(
                            not_a_container
                                .message([
                                    pos.x().to_string(),
                                    pos.y().to_string(),
                                    pos.z().to_string(),
                                ])
                                .component(),
                        )
                    })?;
                Ok(Self::Block { pos, container })
            }
        }
    }
}

/// Vanilla `getSlotsFromProvider` for an entity: the range's slots the
/// entity has.
fn entity_slots<'a>(entity: &'a SharedEntity, range: &SlotRange) -> Vec<SlotAccess<'a>> {
    range
        .slots()
        .iter()
        .filter_map(|&slot| entity.get_slot(slot))
        .collect()
}

/// Vanilla `Container.getSlot` over a slot range.
fn container_slots(container: &ContainerRef, range: &SlotRange) -> Vec<SlotAccess<'static>> {
    range
        .slots()
        .iter()
        .filter_map(|&slot| SlotAccess::for_container_ref(container.clone(), slot))
        .collect()
}

/// Vanilla `ItemCommands.getItems`: copies of the source slots' items.
fn get_items(context: &Context, kind: AccessorKind) -> Result<Vec<ItemStack>, CommandSyntaxError> {
    let range = context.slot_source("sourceSlots")?;
    let accessor = ItemAccessor::access(
        context,
        kind,
        "source",
        &translations::COMMANDS_ITEM_SOURCE_NOT_A_CONTAINER,
    )?;
    let items: Vec<ItemStack> = match &accessor {
        ItemAccessor::Entity(entities) => entities
            .iter()
            .flat_map(|entity| {
                entity_slots(entity, range)
                    .iter()
                    .map(SlotAccess::get)
                    .collect::<Vec<_>>()
            })
            .collect(),
        ItemAccessor::Block { container, .. } => container_slots(container, range)
            .iter()
            .map(SlotAccess::get)
            .collect(),
    };
    if items.is_empty() {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_ITEM_SOURCE_NO_SUCH_SLOT
                .message([range.name().to_owned()])
                .component(),
        ));
    }
    Ok(items)
}

/// Vanilla `setItem`: `with <item> [<count>]`, reporting the known item.
fn set_item(
    context: &Context,
    target: AccessorKind,
    distributor: Distributor,
    count: i32,
) -> Result<i32, CommandSyntaxError> {
    let item = create_item_stack(context.item_stack("item")?, count)?;
    set_items(
        context,
        target,
        distributor.provider(vec![item.clone()]),
        Some(&item),
    )
}

/// Vanilla `ItemInput.createItemStack`.
fn create_item_stack(prototype: &ItemStack, count: i32) -> Result<ItemStack, CommandSyntaxError> {
    let stack = prototype.copy_with_count(count);
    let max_stack_size = stack.max_stack_size();
    if count > max_stack_size {
        return Err(CommandSyntaxError::dynamic(
            translations::ARGUMENTS_ITEM_OVERSTACKED
                .message([stack.item().key.to_string(), max_stack_size.to_string()])
                .component(),
        ));
    }
    stack.validate_strict().map_err(|error| {
        CommandSyntaxError::dynamic(
            translations::ARGUMENTS_ITEM_MALFORMED
                .message([error.to_string()])
                .component(),
        )
    })?;
    Ok(stack)
}

/// Vanilla `SlotCollection.replaceSlotItems` with a tracking `ANY_SLOT`
/// selector: fills slots in order while items remain, counting every slot
/// offered in `selected` and returning how many accepted their item.
fn replace_slot_items(
    slots: &[SlotAccess<'_>],
    items: &mut ItemProvider,
    selected: &mut i32,
) -> i32 {
    let mut success = 0;
    for slot in slots {
        if !items.has_next() {
            break;
        }
        *selected += 1;
        if slot.set(items.next()) {
            success += 1;
        }
    }
    success
}

/// Vanilla `ItemCommands.setItems` with the accessor's `setItems` and
/// `getReplaceSuccess`.
fn set_items(
    context: &Context,
    kind: AccessorKind,
    mut items: ItemProvider,
    known_item: Option<&ItemStack>,
) -> Result<i32, CommandSyntaxError> {
    let range = context.slot_source("slots")?;
    let accessor = ItemAccessor::access(
        context,
        kind,
        "target",
        &translations::COMMANDS_ITEM_TARGET_NOT_A_CONTAINER,
    )?;
    let mut selected = 0;
    let message = match &accessor {
        ItemAccessor::Entity(entities) => {
            let mut tracker = ResponseTracker::default();
            for entity in entities {
                items.restart();
                let updated =
                    replace_slot_items(&entity_slots(entity, range), &mut items, &mut selected);
                tracker.track(entity, updated);
                if updated > 0
                    && let Some(player) = entity.as_player()
                {
                    player.broadcast_inventory_changes_and_trigger();
                }
            }
            check_selected(range, selected)?;
            entity_replace_message(&tracker, known_item)?
        }
        ItemAccessor::Block { pos, container } => {
            items.restart();
            let updated = replace_slot_items(
                &container_slots(container, range),
                &mut items,
                &mut selected,
            );
            check_selected(range, selected)?;
            let mut tracker = ResponseTracker::default();
            tracker.track(pos, updated);
            block_replace_message(&tracker, known_item)?
        }
    };
    context.source().send_success(&message.0, true);
    Ok(message.1)
}

/// Vanilla's `ERROR_TARGET_INAPPLICABLE_SLOT` when no target slot was
/// offered an item.
fn check_selected(range: &SlotRange, selected: i32) -> Result<(), CommandSyntaxError> {
    if selected == 0 {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_ITEM_TARGET_NO_SUCH_SLOT
                .message([range.name().to_owned()])
                .component(),
        ));
    }
    Ok(())
}

/// Vanilla `CommandResponseTracker` with the `NON_ZERO` element type that
/// `sendFeedback` uses.
struct ResponseTracker<T> {
    total: i32,
    only_non_zero: Option<T>,
    non_zero_count: i32,
}

impl<T> Default for ResponseTracker<T> {
    fn default() -> Self {
        Self {
            total: 0,
            only_non_zero: None,
            non_zero_count: 0,
        }
    }
}

impl<T> ResponseTracker<T> {
    fn track(&mut self, element: T, value: i32) {
        self.total += value;
        if value != 0 {
            self.non_zero_count += 1;
            self.only_non_zero = (self.non_zero_count == 1).then_some(element);
        }
    }
}

/// Vanilla `ERROR_TARGET_NO_CHANGES` or its `known_item` variant.
fn no_changes(known_item: Option<&ItemStack>) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(known_item.map_or_else(
        || TextComponent::from(&translations::COMMANDS_ITEM_TARGET_FAILED),
        |item| {
            translations::COMMANDS_ITEM_TARGET_FAILED_KNOWN_ITEM
                .message([ITEM_BEHAVIORS.display_name(item)])
                .component()
        },
    ))
}

/// Vanilla `EntityItemAccessor.RESPONSE_SET` and `RESPONSE_SET_KNOWN_ITEM`.
fn entity_replace_message(
    tracker: &ResponseTracker<&SharedEntity>,
    known_item: Option<&ItemStack>,
) -> Result<(TextComponent, i32), CommandSyntaxError> {
    if tracker.non_zero_count == 0 {
        return Err(no_changes(known_item));
    }
    let total = TextComponent::from(tracker.total.to_string());
    let message = match (tracker.only_non_zero, known_item) {
        (Some(entity), None) => translations::COMMANDS_ITEM_ENTITY_REPLACE_SUCCESS_SINGLE
            .message([total, entity.display_name()])
            .component(),
        (Some(entity), Some(item)) => {
            translations::COMMANDS_ITEM_ENTITY_REPLACE_SUCCESS_SINGLE_KNOWN_ITEM
                .message([
                    total,
                    entity.display_name(),
                    ITEM_BEHAVIORS.display_name(item),
                ])
                .component()
        }
        (None, None) => translations::COMMANDS_ITEM_ENTITY_REPLACE_SUCCESS_MULTIPLE
            .message([TextComponent::from(tracker.non_zero_count.to_string())])
            .component(),
        (None, Some(item)) => {
            translations::COMMANDS_ITEM_ENTITY_REPLACE_SUCCESS_MULTIPLE_KNOWN_ITEM
                .message([
                    TextComponent::from(tracker.non_zero_count.to_string()),
                    ITEM_BEHAVIORS.display_name(item),
                ])
                .component()
        }
    };
    Ok((message, tracker.total))
}

/// Vanilla `BlockItemAccessor.RESPONSE_SET` and `RESPONSE_SET_KNOWN_ITEM`.
fn block_replace_message(
    tracker: &ResponseTracker<&BlockPos>,
    known_item: Option<&ItemStack>,
) -> Result<(TextComponent, i32), CommandSyntaxError> {
    let Some(pos) = tracker.only_non_zero else {
        return Err(no_changes(known_item));
    };
    let [count, x, y, z] = [tracker.total, pos.x(), pos.y(), pos.z()]
        .map(|value| TextComponent::from(value.to_string()));
    let message = match known_item {
        Some(item) => translations::COMMANDS_ITEM_BLOCK_REPLACE_SUCCESS_KNOWN_ITEM
            .message([count, x, y, z, ITEM_BEHAVIORS.display_name(item)])
            .component(),
        None => translations::COMMANDS_ITEM_BLOCK_REPLACE_SUCCESS
            .message([count, x, y, z])
            .component(),
    };
    Ok((message, tracker.total))
}
