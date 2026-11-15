//! Undoing what the log holds (`/co rollback`) and redoing it (`/co restore`).
//!
//! The rows come from the reader thread; the world is changed on the server tick, a slice of
//! rows per tick, so a big rollback never becomes one long tick.

use std::{io::Cursor, str::FromStr as _, sync::Arc};

use simdnbt::borrow::{NbtCompound as NbtCompoundView, read_compound};
use steel_core::{
    inventory::lock::{ContainerLockGuard, ContainerRef},
    server::Server,
    world::World,
};
use steel_registry::{REGISTRY, RegistryExt as _, item_stack::ItemStack, vanilla_blocks};
use steel_utils::{BlockPos, BlockStateId, ChunkPos, Identifier, types::UpdateFlags};

use super::store::{BREAK, Event, PLACE, PUT, Row, Subject, TAKE};

/// Rows applied per server tick.
pub(super) const ROWS_PER_TICK: usize = 1500;
/// The most rows one rollback takes; a wider one is run again.
pub(super) const MAX_ROWS: u32 = 500_000;

/// The world is told and redrawn, but neighbours are not updated: a rollback puts back what
/// was, it does not replay physics.
const FLAGS: UpdateFlags = UpdateFlags::UPDATE_CLIENTS.union(UpdateFlags::UPDATE_KNOWN_SHAPE);

/// What happened to one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Applied {
    /// A block is as the row wants it now.
    Block,
    /// Items went in or out as the row wants.
    Items,
    /// The chunk is not loaded: the row stays for another run.
    Unloaded,
    /// The row cannot be applied: an unknown block or item, no container there, no room.
    Failed,
    /// The row changes nothing in the world (a click).
    Nothing,
}

/// The counts of a finished run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Outcome {
    pub blocks: u32,
    pub items: u32,
    pub unloaded: u32,
    pub failed: u32,
}

/// A run in progress: the rows still to apply and what the applied ones came to.
pub(super) struct Run {
    rows: Vec<Row>,
    next: usize,
    /// `true` undoes the rows, `false` redoes them.
    undo: bool,
    done_blocks: Vec<i64>,
    done_containers: Vec<i64>,
    outcome: Outcome,
}

impl Run {
    /// Starts a run over `rows`: newest first for an undo, oldest first for a redo.
    pub(super) const fn new(rows: Vec<Row>, undo: bool) -> Self {
        Self {
            rows,
            next: 0,
            undo,
            done_blocks: Vec::new(),
            done_containers: Vec::new(),
            outcome: Outcome {
                blocks: 0,
                items: 0,
                unloaded: 0,
                failed: 0,
            },
        }
    }

    /// Applies the next slice of rows. `true` once every row was tried.
    pub(super) fn step(&mut self, server: &Arc<Server>) -> bool {
        let end = (self.next + ROWS_PER_TICK).min(self.rows.len());
        for row in &self.rows[self.next..end] {
            let done = match &row.event.subject {
                Subject::Block { .. } => &mut self.done_blocks,
                Subject::Items { .. } => &mut self.done_containers,
            };
            match apply(server, &row.event, self.undo) {
                Applied::Block => {
                    self.outcome.blocks += 1;
                    done.push(row.id);
                }
                Applied::Items => {
                    self.outcome.items += 1;
                    done.push(row.id);
                }
                Applied::Unloaded => self.outcome.unloaded += 1,
                Applied::Failed => self.outcome.failed += 1,
                Applied::Nothing => {}
            }
        }
        self.next = end;
        self.next == self.rows.len()
    }

    /// The counts, and the block and container rows that were applied, to be marked.
    pub(super) fn finish(self) -> (Outcome, Vec<i64>, Vec<i64>) {
        (self.outcome, self.done_blocks, self.done_containers)
    }
}

/// `facing=north,lit=false` of `kind` as a block state.
pub(super) fn block_state(kind: &str, state: &str) -> Option<BlockStateId> {
    let key = Identifier::from_str(kind).ok()?;
    let properties: Vec<(&str, &str)> = state
        .split(',')
        .filter_map(|property| property.split_once('='))
        .collect();
    REGISTRY.blocks.state_id_from_properties(&key, &properties)
}

/// The stack of an items row: from its NBT if it has one, else a plain item.
fn stack(event: &Event, amount: i32, data: Option<&[u8]>) -> Option<ItemStack> {
    if let Some(data) = data {
        let compound = read_compound(&mut Cursor::new(data)).ok()?;
        let stack = ItemStack::from_borrowed_compound(&NbtCompoundView::from(&compound))?;
        return Some(stack.copy_with_count(amount));
    }
    let item = REGISTRY
        .items
        .by_key(&Identifier::from_str(&event.kind).ok()?)?;
    Some(ItemStack::with_count(item, amount))
}

fn apply(server: &Arc<Server>, event: &Event, undo: bool) -> Applied {
    let Some(world) = Identifier::from_str(&event.world)
        .ok()
        .and_then(|key| server.worlds.get(&key))
    else {
        return Applied::Failed;
    };
    let (x, y, z) = event.pos;
    let pos = BlockPos::new(x, y, z);
    if world
        .chunk_map
        .with_full_chunk(ChunkPos::from_block_pos(pos), |_| ())
        .is_none()
    {
        return Applied::Unloaded;
    }
    match &event.subject {
        Subject::Block { state } => {
            // Undoing a break puts the block back; undoing a placement takes it away.
            let present = match event.action {
                BREAK => undo,
                PLACE => !undo,
                _ => return Applied::Nothing,
            };
            let target = if present {
                match block_state(&event.kind, state) {
                    Some(target) => target,
                    None => return Applied::Failed,
                }
            } else {
                vanilla_blocks::AIR.default_state()
            };
            if world.get_block_state(pos) == target || world.set_block(pos, target, FLAGS) {
                Applied::Block
            } else {
                Applied::Failed
            }
        }
        Subject::Items { amount, data } => {
            // Undoing a take puts the items back; undoing a put takes them out.
            let add = match event.action {
                TAKE => undo,
                PUT => !undo,
                _ => return Applied::Nothing,
            };
            let Some(stack) = stack(event, *amount, data.as_deref()) else {
                return Applied::Failed;
            };
            if move_items(world, pos, stack, add) {
                Applied::Items
            } else {
                Applied::Failed
            }
        }
    }
}

/// Puts `stack` into the container at `pos`, or takes that many of it out. All or nothing.
fn move_items(world: &Arc<World>, pos: BlockPos, mut stack: ItemStack, add: bool) -> bool {
    let Some(container_ref) = world
        .get_block_entity(pos)
        .and_then(ContainerRef::from_block_entity)
    else {
        return false;
    };
    let mut guard = ContainerLockGuard::lock_all(&[&container_ref]);
    let Some(container) = guard.get_mut(container_ref.container_id()) else {
        return false;
    };
    if add {
        let room: i32 = container
            .items()
            .iter()
            .map(|slot| {
                if slot.is_empty() {
                    stack.max_stack_size()
                } else if ItemStack::is_same_item_same_components(slot, &stack) {
                    (stack.max_stack_size() - slot.count()).max(0)
                } else {
                    0
                }
            })
            .sum();
        if room < stack.count() {
            return false;
        }
        container.add(&mut stack);
        return stack.is_empty();
    }
    let held: i32 = container
        .items()
        .iter()
        .filter(|slot| ItemStack::is_same_item_same_components(slot, &stack))
        .map(ItemStack::count)
        .sum();
    if held < stack.count() {
        return false;
    }
    let mut left = stack.count();
    for slot in container.items_mut() {
        if left == 0 {
            break;
        }
        if ItemStack::is_same_item_same_components(slot, &stack) {
            let taken = slot.count().min(left);
            slot.shrink(taken);
            left -= taken;
        }
    }
    container.set_changed();
    true
}

#[cfg(test)]
mod tests {
    use steel_registry::{
        blocks::{
            block_state_ext::BlockStateExt as _,
            properties::{BlockStateProperties, Direction},
        },
        init_vanilla_registry,
    };

    use super::{super::state_text, *};

    #[test]
    fn a_logged_state_reads_back_as_the_same_block_state() {
        init_vanilla_registry();
        let furnace = vanilla_blocks::FURNACE
            .default_state()
            .set_value(&BlockStateProperties::HORIZONTAL_FACING, Direction::East)
            .set_value(&BlockStateProperties::LIT, true);
        let text = state_text(furnace);
        assert_eq!(block_state("minecraft:furnace", &text), Some(furnace));
        assert_eq!(
            block_state("minecraft:stone", ""),
            Some(vanilla_blocks::STONE.default_state())
        );
        // A block of a mod that is gone, or a property that no longer exists.
        assert_eq!(block_state("oldmod:machine", ""), None);
        assert_eq!(block_state("minecraft:stone", "shiny=true"), None);
    }
}
