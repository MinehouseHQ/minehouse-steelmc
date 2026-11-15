//! «Терминал оплаты», the block (the Fabric `TerminalBlock`).
//!
//! Small, stands on a counter or on the stock chest of a vending terminal. The client sees a
//! state of the plant carrier (no collision, breaks like cobblestone) that the server pack
//! draws with our model: off until the terminal is set up, lit once it is locked. Without
//! the pack it is a stone button.
//!
//! A text display above says what the terminal sells or what the till asks for. On the Fabric
//! server it was a virtual entity of Polymer. Here it is a real one, saved with the chunk and
//! tagged [`TAG`]: made with the block, removed with it, and made again when it is missing.

use std::{ptr, sync::Arc};

use glam::DVec3;
use steel_core::{
    behavior::{
        BlockBehavior, BlockLootContext, BlockPlaceContext, InteractionResult, InventoryAccess,
    },
    entity::{
        Entity as _, RemovalReason, SharedEntity, entities::TextDisplayEntity, next_entity_id,
    },
    player::Player,
    world::{LevelReader, World},
};
use steel_registry::{
    blocks::{
        Block, BlockRegistry, behavior::BlockConfig, block_state_ext::BlockStateExt as _,
        properties::BlockStateProperties,
    },
    entity_data::Vector3f,
    item_stack::ItemStack,
    items::item::BlockHitResult,
    sound_types, vanilla_blocks, vanilla_entities,
};
use steel_utils::{
    BlockPos, BlockStateId, Downcast as _, Identifier, WorldAabb,
    types::{InteractionHand, UpdateFlags},
};
use text_components::TextComponent;

use super::{bank, terminal};
use crate::content::{TERMINAL, blocks::show_block};

/// The tag of the text display above a terminal.
pub const TAG: &str = "minehouse_terminal";

/// Vanilla `Display.BillboardConstraints.CENTER`.
const BILLBOARD_CENTER: i8 = 3;
/// A third-transparent black, as on the Fabric server.
const LABEL_BACKGROUND: i32 = 0x6000_0000;

/// «Терминал оплаты».
pub static TERMINAL_BLOCK: Block = Block::new(
    Identifier::new_static("minehouse-bank", "terminal"),
    BlockConfig::new()
        .destroy_time(2.0)
        .explosion_resistance(6.0)
        .has_collision(false)
        .can_occlude(false)
        .sound_type(sound_types::STONE),
    &[
        &BlockStateProperties::HORIZONTAL_FACING,
        &BlockStateProperties::LOCKED,
    ],
);

/// Has clients see a terminal as its state of the pack, or as a stone button without one,
/// and its break as cobblestone.
pub(crate) fn show_to_clients(blocks: &mut BlockRegistry) {
    show_block(
        blocks,
        &TERMINAL_BLOCK,
        |blocks, _| blocks.get_default_state_id(&vanilla_blocks::STONE_BUTTON),
        |blocks, _| blocks.get_default_state_id(&vanilla_blocks::COBBLESTONE),
    );
}

/// Whether the block at `pos` is a terminal.
#[must_use]
pub fn is_terminal(world: &World, pos: BlockPos) -> bool {
    ptr::eq(
        world.get_block_state(pos).get_block(),
        &raw const TERMINAL_BLOCK,
    )
}

/// Lights the terminal at `pos` up: it is locked now.
pub(super) fn light_up(world: &Arc<World>, pos: BlockPos) {
    let state = world.get_block_state(pos);
    if ptr::eq(state.get_block(), &raw const TERMINAL_BLOCK) {
        world.set_block(
            pos,
            state.set_value(&BlockStateProperties::LOCKED, true),
            UpdateFlags::UPDATE_ALL,
        );
    }
}

fn label_at(pos: BlockPos) -> DVec3 {
    DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + 1.05,
        f64::from(pos.z()) + 0.5,
    )
}

/// The text displays of the terminal at `pos`.
fn labels(world: &World, pos: BlockPos) -> Vec<SharedEntity> {
    let at = label_at(pos);
    world
        .get_entities_in_aabb(&WorldAabb::from_min_max(at, at).inflate(0.5))
        .into_iter()
        .filter(|entity| {
            !entity.is_removed()
                && entity.downcast_ref::<TextDisplayEntity>().is_some()
                && entity.tags().iter().any(|tag| tag == TAG)
                && entity.position().distance_squared(at) < 0.01
        })
        .collect()
}

/// Whether the terminal at `pos` has its label.
pub(super) fn has_label(world: &World, pos: BlockPos) -> bool {
    !labels(world, pos).is_empty()
}

/// Has the label above the terminal at `pos` say `text`, making it when it is missing.
pub(super) fn show_label(world: &Arc<World>, pos: BlockPos, text: TextComponent) {
    let mut found = labels(world, pos).into_iter();
    if let Some(entity) = found.next() {
        if let Some(label) = entity.downcast_ref::<TextDisplayEntity>() {
            label.set_text(text);
        }
        // A terminal has one label.
        for extra in found {
            extra.set_removed(RemovalReason::Discarded);
        }
        return;
    }
    let label = TextDisplayEntity::new(
        &vanilla_entities::TEXT_DISPLAY,
        next_entity_id(),
        label_at(pos),
        Arc::downgrade(world),
    );
    label.edit(|data| {
        data.text.set(Box::new(text));
        data.background_color.set(LABEL_BACKGROUND);
        data.display
            .billboard_render_constraints
            .set(BILLBOARD_CENTER);
        data.display.scale.set(Vector3f {
            x: 0.5,
            y: 0.5,
            z: 0.5,
        });
        data.display.view_range.set(0.25);
    });
    label.base().add_tag(TAG.to_owned());
    let entity: SharedEntity = Arc::new(label);
    if let Err(error) = world.try_add_entity(entity) {
        log::warn!("Bank: could not label the terminal at {pos:?}: {error:?}");
    }
}

fn remove_labels(world: &World, pos: BlockPos) {
    for label in labels(world, pos) {
        label.set_removed(RemovalReason::Discarded);
    }
}

/// The terminal block.
pub(crate) struct Terminal;

impl BlockBehavior for Terminal {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        // The screen looks at the player.
        Some(TERMINAL_BLOCK.default_state().set_value(
            &BlockStateProperties::HORIZONTAL_FACING,
            context.horizontal_direction().opposite(),
        ))
    }

    fn can_survive(&self, _state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> bool {
        !world.get_block_state(pos.below()).is_air()
    }

    /// A terminal is a small thing: the chest under it opens.
    fn is_redstone_conductor(
        &self,
        _state: BlockStateId,
        _world: &dyn LevelReader,
        _pos: BlockPos,
    ) -> bool {
        false
    }

    fn get_drops(
        &self,
        _state: BlockStateId,
        _context: &BlockLootContext<'_>,
    ) -> Option<Vec<ItemStack>> {
        Some(vec![TERMINAL.stack(1)])
    }

    fn on_place(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _old_state: BlockStateId,
        _moved_by_piston: bool,
    ) {
        if let Some(bank) = bank() {
            bank.terminals.ensure(&world.key, pos);
            terminal::relabel(bank, world, pos);
        }
    }

    fn player_will_destroy(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
    ) -> BlockStateId {
        terminal::broken(world, pos, player.gameprofile.name());
        state
    }

    fn affect_neighbors_after_removal(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _moved_by_piston: bool,
    ) {
        // Lighting up changes the state, not the block: the terminal stays.
        if is_terminal(world, pos) {
            return;
        }
        if let Some(bank) = bank() {
            bank.terminals.remove(&world.key, pos);
        }
        remove_labels(world, pos);
    }

    fn use_item_on(
        &self,
        _state: BlockStateId,
        _world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        hand: InteractionHand,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        if hand != InteractionHand::MainHand {
            return InteractionResult::Pass;
        }
        terminal::open(player, pos);
        InteractionResult::Success
    }

    fn use_without_item(
        &self,
        _state: BlockStateId,
        _world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        terminal::open(player, pos);
        InteractionResult::Success
    }
}
