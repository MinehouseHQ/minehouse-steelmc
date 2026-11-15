//! Vanilla `/loot` (`LootCommand`): roll a loot table the way fishing,
//! a chest, a kill or mining a block would, then hand the drops to players,
//! a container, entity slots or the world.

use std::borrow::Cow;

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::item_stack::ItemStack;
use steel_registry::loot_table::LootTableRef;
use steel_registry::loot_table::{DamageSourceInfo, LootContext};
use steel_registry::vanilla_damage_types;
use steel_registry::{REGISTRY, RegistryExt as _};
use steel_utils::{BlockPos, BlockStateId, Identifier, translations};
use text_components::TextComponent;
use text_components::translation::TranslatedMessage;

use super::execute::loaded_block_position;
use crate::behavior::BlockLootContext;
use crate::behavior::ITEM_BEHAVIORS;
use crate::block_entity::BlockEntity;
use crate::command::brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError};
use crate::command::execution::{
    CommandSource, ElementType, ResponseTracker, SteelArgumentType, SteelCommandContext,
    SteelCommandRuntime, argument, literal,
};
use crate::command::registration::CommandRegistration;
use crate::entity::{Entity, entity_loot_ref, entity_loot_table_key, loot_equipment};
use crate::inventory::container::Container;
use crate::inventory::equipment::EquipmentSlot;
use crate::inventory::lock::{ContainerId, ContainerLockGuard, ContainerRef};
use crate::world::World;

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;
type Context = SteelCommandContext<CommandSource>;

/// Vanilla `LootCommand.DropConsumer`: where a target puts the drops.
type DropConsumer =
    fn(&Context, &[ItemStack], &mut ResponseTracker<ItemStack>) -> Result<(), CommandSyntaxError>;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("loot"), |_| command())
}

/// Vanilla `LootCommand.register` with `addTargets`.
fn command() -> Builder {
    literal("loot")
        .then(
            literal("replace")
                .then(
                    literal("entity").then(
                        argument("entities", SteelArgumentType::entities()).then(
                            sources(
                                argument("slot", SteelArgumentType::item_slot()),
                                |context, drops, used| {
                                    entity_replace(context, drops.len(), drops, used)
                                },
                            )
                            .then(sources(
                                argument("count", ArgumentType::integer(0, i32::MAX)),
                                |context, drops, used| {
                                    let count = context.integer("count")?;
                                    entity_replace(context, slot_count(count), drops, used)
                                },
                            )),
                        ),
                    ),
                )
                .then(
                    literal("block").then(
                        argument("targetPos", SteelArgumentType::block_pos()).then(
                            sources(
                                argument("slot", SteelArgumentType::item_slot()),
                                |context, drops, used| {
                                    block_replace(context, drops.len(), drops, used)
                                },
                            )
                            .then(sources(
                                argument("count", ArgumentType::integer(0, i32::MAX)),
                                |context, drops, used| {
                                    let count = context.integer("count")?;
                                    block_replace(context, slot_count(count), drops, used)
                                },
                            )),
                        ),
                    ),
                ),
        )
        .then(literal("insert").then(sources(
            argument("targetPos", SteelArgumentType::block_pos()),
            block_distribute,
        )))
        .then(literal("give").then(sources(
            argument("players", SteelArgumentType::players()),
            player_give,
        )))
        .then(literal("spawn").then(sources(
            argument("targetPos", SteelArgumentType::vec3(true)),
            drop_in_world,
        )))
}

/// A non-negative `count` argument as a slot count.
fn slot_count(count: i32) -> usize {
    usize::try_from(count).unwrap_or(0)
}

/// Which tool a `fish` or `mine` source uses.
#[derive(Clone, Copy)]
enum Tool {
    /// No tool: an empty stack.
    Empty,
    /// The `tool` item argument, one of it.
    Argument,
    /// What the source entity holds in that slot.
    Hand(EquipmentSlot),
}

/// The four loot sources under each target, vanilla's `TailProvider`.
fn sources(target: Builder, output: DropConsumer) -> Builder {
    target
        .then(literal("fish").then(
            argument("loot_table", SteelArgumentType::loot_table()).then(tool_nodes(
                argument("pos", SteelArgumentType::block_pos()),
                move |context, tool| drop_fishing_loot(context, tool, output),
            )),
        ))
        .then(
            literal("loot").then(
                argument("loot_table", SteelArgumentType::loot_table())
                    .executes(move |context| drop_chest_loot(context, output)),
            ),
        )
        .then(
            literal("kill").then(
                argument("target", SteelArgumentType::entity())
                    .executes(move |context| drop_kill_loot(context, output)),
            ),
        )
        .then(literal("mine").then(tool_nodes(
            argument("pos", SteelArgumentType::block_pos()),
            move |context, tool| drop_block_loot(context, tool, output),
        )))
}

/// `<pos>` alone, then `<tool>`, `mainhand` and `offhand`.
fn tool_nodes(
    pos: Builder,
    run: impl Fn(&Context, Tool) -> Result<i32, CommandSyntaxError> + Copy + Send + Sync + 'static,
) -> Builder {
    pos.executes(move |context| run(context, Tool::Empty))
        .then(
            argument("tool", SteelArgumentType::item_stack())
                .executes(move |context| run(context, Tool::Argument)),
        )
        .then(
            literal("mainhand")
                .executes(move |context| run(context, Tool::Hand(EquipmentSlot::MainHand))),
        )
        .then(
            literal("offhand")
                .executes(move |context| run(context, Tool::Hand(EquipmentSlot::OffHand))),
        )
}

/// The tool stack, `ItemArgument.createItemStack(1)` for an argument.
fn resolve_tool(context: &Context, tool: Tool) -> Result<ItemStack, CommandSyntaxError> {
    match tool {
        Tool::Empty => Ok(ItemStack::empty()),
        Tool::Argument => Ok(context.item_stack("tool")?.copy_with_count(1)),
        Tool::Hand(slot) => source_hand_item(context.source(), slot),
    }
}

/// Vanilla `getSourceHandItem`.
fn source_hand_item(
    source: &CommandSource,
    slot: EquipmentSlot,
) -> Result<ItemStack, CommandSyntaxError> {
    let Some(entity) = source.entity() else {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::PERMISSIONS_REQUIRES_ENTITY,
        )));
    };
    let Some(living) = entity.as_living_entity() else {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_DROP_NO_HELD_ITEMS,
        )));
    };
    let mut held = ItemStack::empty();
    living.with_equipment_slot(slot, &mut |item| held = item.clone());
    Ok(held)
}

/// Vanilla `dropBlockLoot`: the block's drops as if mined with `tool`.
fn drop_block_loot(
    context: &Context,
    tool: Tool,
    output: DropConsumer,
) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let pos = loaded_block_position(context, "pos")?;
    let tool = resolve_tool(context, tool)?;
    let world = source.world();
    let state = world.get_block_state(pos);
    let Some(table_key) = block_loot_table_key(state) else {
        let block = state.get_block();
        let name = translated_name("block", &block.key);
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_DROP_NO_LOOT_TABLE_BLOCK
                .message([name])
                .component(),
        ));
    };
    let block_entity = world.get_block_entity(pos);
    let drops = BlockLootContext::new(world, pos)
        .with_entity(source.entity().map(AsRef::as_ref))
        .with_block_entity(
            block_entity
                .as_deref()
                .map(|entity| entity as &dyn BlockEntity),
        )
        .with_tool(&tool)
        .get_drops(state);
    let mut used = ResponseTracker::default();
    output(context, &drops, &mut used)?;
    send_feedback(source, &used, Some(&table_key))
}

/// Vanilla `Block.getLootTable`: the block's own table, or `blocks/<id>`
/// when one exists.
fn block_loot_table_key(state: BlockStateId) -> Option<Identifier> {
    let block = state.get_block();
    if let Some(table) = block.config.loot_table {
        return Some(table.key.clone());
    }
    let key = Identifier::vanilla(format!("blocks/{}", block.key.path));
    REGISTRY.loot_tables.by_key(&key).map(|_| key)
}

/// Vanilla `dropKillLoot`: the target's death loot, killed with magic by
/// the source entity.
fn drop_kill_loot(context: &Context, output: DropConsumer) -> Result<i32, CommandSyntaxError> {
    let target = context.entity("target")?;
    let Some(table_key) = entity_loot_table_key(target.as_ref()) else {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_DROP_NO_LOOT_TABLE_ENTITY
                .message([target.display_name()])
                .component(),
        ));
    };
    let source = context.source();
    let drops = REGISTRY
        .loot_tables
        .by_key(&table_key)
        .map_or_else(Vec::new, |table| kill_drops(table, target.as_ref(), source));
    let mut used = ResponseTracker::default();
    output(context, &drops, &mut used)?;
    send_feedback(source, &used, Some(&table_key))
}

/// The `ENTITY` loot params of `dropKillLoot`.
fn kill_drops(table: LootTableRef, target: &dyn Entity, source: &CommandSource) -> Vec<ItemStack> {
    let killer = source.entity().map(AsRef::as_ref);
    let target_equipment = loot_equipment(target);
    let killer_equipment = killer.and_then(loot_equipment);
    let killer_ref = killer.map(|killer| entity_loot_ref(killer, killer_equipment.as_ref()));
    let origin = source.position();
    let mut rng = rand::rng();
    let mut loot = LootContext::new(&mut rng)
        .with_origin(origin.x, origin.y, origin.z)
        .with_game_time(source.world().game_time())
        .with_this_entity(entity_loot_ref(target, target_equipment.as_ref()))
        .with_damage_source(DamageSourceInfo {
            damage_type: Some(&vanilla_damage_types::MAGIC.key),
            tags: &[],
            is_direct: true,
        });
    if let Some(killer_ref) = killer_ref {
        loot = loot
            .with_killer_entity(killer_ref)
            .with_direct_killer_entity(killer_ref);
        if killer.is_some_and(|killer| killer.as_player().is_some()) {
            loot = loot
                .with_last_damage_player(killer_ref)
                .with_killed_by_player(true);
        }
    }
    table.get_random_items(&mut loot)
}

/// Vanilla `dropChestLoot`: the `CHEST` params at the source.
fn drop_chest_loot(context: &Context, output: DropConsumer) -> Result<i32, CommandSyntaxError> {
    let table = context.loot_table("loot_table")?;
    let source = context.source();
    let origin = source.position();
    let this_entity = source.entity().map(AsRef::as_ref);
    let equipment = this_entity.and_then(loot_equipment);
    let mut rng = rand::rng();
    let mut loot = LootContext::new(&mut rng).with_origin(origin.x, origin.y, origin.z);
    if let Some(entity) = this_entity {
        loot = loot.with_this_entity(entity_loot_ref(entity, equipment.as_ref()));
    }
    let drops = table.get_random_items(&mut loot);
    drop(context, &drops, output)
}

/// Vanilla `dropFishingLoot`: the `FISHING` params at the block's center.
fn drop_fishing_loot(
    context: &Context,
    tool: Tool,
    output: DropConsumer,
) -> Result<i32, CommandSyntaxError> {
    let table = context.loot_table("loot_table")?;
    let pos = loaded_block_position(context, "pos")?;
    let tool = resolve_tool(context, tool)?;
    let source = context.source();
    let this_entity = source.entity().map(AsRef::as_ref);
    let equipment = this_entity.and_then(loot_equipment);
    let mut rng = rand::rng();
    let mut loot = LootContext::new(&mut rng)
        .with_origin(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()) + 0.5,
            f64::from(pos.z()) + 0.5,
        )
        .with_tool(&tool);
    if let Some(entity) = this_entity {
        loot = loot.with_this_entity(entity_loot_ref(entity, equipment.as_ref()));
    }
    let drops = table.get_random_items(&mut loot);
    drop(context, &drops, output)
}

/// Vanilla `drop`: hands the drops over and reports without a table name.
fn drop(
    context: &Context,
    drops: &[ItemStack],
    output: DropConsumer,
) -> Result<i32, CommandSyntaxError> {
    let mut used = ResponseTracker::default();
    output(context, drops, &mut used)?;
    send_feedback(context.source(), &used, None)
}

/// `RESPONSE_WITH_LOOT_TABLE` or `RESPONSE_WITHOUT_LOOT_TABLE`, sent to the
/// source only, counting every used drop.
fn send_feedback(
    source: &CommandSource,
    used: &ResponseTracker<ItemStack>,
    table: Option<&Identifier>,
) -> Result<i32, CommandSyntaxError> {
    used.send_feedback(
        source,
        false,
        ElementType::NonZero,
        None,
        |drop, _| {
            let count = TextComponent::plain(drop.count().to_string());
            let name = ITEM_BEHAVIORS.display_name(drop);
            match table {
                Some(key) => translations::COMMANDS_DROP_SUCCESS_SINGLE_WITH_TABLE
                    .message([count, name, TextComponent::plain(key.to_string())])
                    .component(),
                None => translations::COMMANDS_DROP_SUCCESS_SINGLE
                    .message([count, name])
                    .component(),
            }
        },
        |drop_count, _| {
            let count = drop_count.to_string();
            match table {
                Some(key) => translations::COMMANDS_DROP_SUCCESS_MULTIPLE_WITH_TABLE
                    .message([count, key.to_string()])
                    .component(),
                None => translations::COMMANDS_DROP_SUCCESS_MULTIPLE
                    .message([count])
                    .component(),
            }
        },
    )
}

/// A `<kind>.<namespace>.<path>` description translation.
fn translated_name(kind: &str, key: &Identifier) -> TextComponent {
    TextComponent::translated(TranslatedMessage {
        key: Cow::Owned(format!("{kind}.{}.{}", key.namespace, key.path)),
        fallback: None,
        args: None,
    })
}

/// Vanilla `BlockItemAccessor.getContainer` with `ERROR_TARGET_NOT_A_CONTAINER`.
fn target_container(world: &World, pos: BlockPos) -> Result<ContainerRef, CommandSyntaxError> {
    world
        .get_block_entity(pos)
        .and_then(ContainerRef::from_block_entity)
        .ok_or_else(|| {
            CommandSyntaxError::dynamic(
                translations::COMMANDS_ITEM_TARGET_NOT_A_CONTAINER
                    .message([
                        pos.x().to_string(),
                        pos.y().to_string(),
                        pos.z().to_string(),
                    ])
                    .component(),
            )
        })
}

/// Vanilla `blockDistribute`: `loot insert`.
fn block_distribute(
    context: &Context,
    drops: &[ItemStack],
    used: &mut ResponseTracker<ItemStack>,
) -> Result<(), CommandSyntaxError> {
    let pos = loaded_block_position(context, "targetPos")?;
    let container = target_container(context.source().world(), pos)?;
    let id = container.container_id();
    let mut guard = ContainerLockGuard::lock_all(&[&container]);
    for drop in drops {
        if distribute_to_container(&mut guard, id, drop.clone()) {
            guard.set_changed(id);
            used.track_one(drop);
        }
    }
    Ok(())
}

/// Vanilla `distributeToContainer`: merges into matching stacks in slot
/// order and puts the rest in the first empty slot that takes it.
fn distribute_to_container(
    guard: &mut ContainerLockGuard,
    id: ContainerId,
    mut stack: ItemStack,
) -> bool {
    let size = guard.get(id).map_or(0, Container::get_container_size);
    let mut changed = false;
    for slot in 0..size {
        if stack.is_empty() {
            break;
        }
        let Some(container) = guard.get_mut(id) else {
            break;
        };
        if !container.can_place_item(slot, &stack) {
            continue;
        }
        let current = container.get_item_mut(slot);
        if current.is_empty() {
            guard.set_item(id, slot, stack);
            return true;
        }
        if can_merge_items(current, &stack) {
            let space = stack.max_stack_size() - current.count();
            let count = stack.count().min(space);
            stack.shrink(count);
            current.grow(count);
            changed = true;
        }
    }
    changed
}

/// Vanilla `canMergeItems`.
fn can_merge_items(a: &ItemStack, b: &ItemStack) -> bool {
    a.count() <= a.max_stack_size() && ItemStack::is_same_item_same_components(a, b)
}

/// Vanilla `blockReplace`: `loot replace block <pos> <slot> [<count>]`.
fn block_replace(
    context: &Context,
    slot_count: usize,
    drops: &[ItemStack],
    used: &mut ResponseTracker<ItemStack>,
) -> Result<(), CommandSyntaxError> {
    let pos = loaded_block_position(context, "targetPos")?;
    let container = target_container(context.source().world(), pos)?;
    let start_slot = context.item_slot("slot")?;
    let id = container.container_id();
    let mut guard = ContainerLockGuard::lock_all(&[&container]);
    let size = guard.get(id).map_or(0, Container::get_container_size);
    let Some(start) = usize::try_from(start_slot)
        .ok()
        .filter(|&start| start < size)
    else {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_ITEM_TARGET_NO_SUCH_SLOT
                .message([start_slot.to_string()])
                .component(),
        ));
    };
    // Vanilla indexes past the container's end here and fails with an
    // out-of-bounds exception; Steel stops at the last slot instead.
    for (offset, slot) in (start..size).take(slot_count).enumerate() {
        let to_add = drops.get(offset).cloned().unwrap_or_else(ItemStack::empty);
        let placeable = guard
            .get(id)
            .is_some_and(|container| container.can_place_item(slot, &to_add));
        if placeable {
            used.track_one(&to_add);
            guard.set_item(id, slot, to_add);
        }
    }
    Ok(())
}

/// Vanilla `entityReplace` with `setSlots`.
fn entity_replace(
    context: &Context,
    slot_count: usize,
    drops: &[ItemStack],
    used: &mut ResponseTracker<ItemStack>,
) -> Result<(), CommandSyntaxError> {
    let entities = context.entities("entities")?;
    let start_slot = context.item_slot("slot")?;
    for entity in &entities {
        for offset in 0..slot_count {
            let item = drops.get(offset).cloned().unwrap_or_else(ItemStack::empty);
            let slot = i32::try_from(offset)
                .ok()
                .and_then(|offset| start_slot.checked_add(offset));
            let accepted = slot
                .and_then(|slot| entity.get_slot(slot))
                .is_some_and(|access| access.set(item.clone()));
            if accepted {
                used.track_one(&item);
            }
        }
        if let Some(player) = entity.as_player() {
            player.broadcast_inventory_changes_and_trigger();
        }
    }
    Ok(())
}

/// Vanilla `playerGive`: each drop to every player, counted per player
/// that took any of it.
fn player_give(
    context: &Context,
    drops: &[ItemStack],
    used: &mut ResponseTracker<ItemStack>,
) -> Result<(), CommandSyntaxError> {
    let players = context.players("players")?;
    for drop in drops {
        for player in &players {
            if player.inventory.lock().add(&mut drop.clone()) {
                used.track_one(drop);
            }
        }
    }
    Ok(())
}

/// Vanilla `dropInWorld`: an item entity per drop at the position, with
/// the default pickup delay.
fn drop_in_world(
    context: &Context,
    drops: &[ItemStack],
    used: &mut ResponseTracker<ItemStack>,
) -> Result<(), CommandSyntaxError> {
    let source = context.source();
    let pos = context.coordinates("targetPos")?.position(source);
    for drop in drops {
        if let Some(entity) = source.world().spawn_item(pos, drop.clone()) {
            entity.set_default_pickup_delay();
        }
        used.track_one(drop);
    }
    Ok(())
}
