//! Written books placed on a lectern, resolved as vanilla's
//! `LecternBlockEntity.resolveBook` resolves them.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::Direction;
use steel_registry::data_components::components::{Filterable, WrittenBookContent};
use steel_registry::data_components::vanilla_components::WRITTEN_BOOK_CONTENT;
use steel_registry::item_stack::ItemStack;
use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_items};
use steel_utils::text::DisplayResolutor;
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, ChunkPos, Downcast as _};
use text_components::TextComponent;

use super::insert_ready_full_chunk;
use super::scoreboard_command::with_server_world;
use crate::behavior::{BLOCK_BEHAVIORS, BlockHitResult, InventoryAccess, init_behaviors};
use crate::block_entity::entities::LecternBlockEntity;
use crate::block_entity::init_block_entities;
use crate::test_support::TestPlayerBuilder;

const LECTERN: BlockPos = BlockPos::new(4, 64, 4);

#[test]
fn placing_a_written_book_on_a_lectern_resolves_its_pages() {
    init_vanilla_registry();
    init_behaviors();
    init_block_entities();
    with_server_world("lectern_book_resolve", |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        assert!(world.set_block(
            LECTERN,
            vanilla_blocks::LECTERN.default_state(),
            UpdateFlags::UPDATE_ALL,
        ));
        let player = TestPlayerBuilder::new(Arc::clone(world), "Reader", 9)
            .server(server)
            .build();
        assert!(world.players.insert(Arc::clone(&player)));
        assert!(server.online_players.insert(Arc::clone(&player)));

        let Ok(content) = WrittenBookContent::new(
            Filterable::pass_through("Tale".to_owned()),
            "Writer".to_owned(),
            0,
            vec![Filterable::pass_through(TextComponent::entity("@s", None))],
            false,
        ) else {
            panic!("the book content should be valid");
        };
        let mut book = ItemStack::new(&vanilla_items::WRITTEN_BOOK);
        book.set(WRITTEN_BOOK_CONTENT, content);
        player.inventory.lock().set_selected_item(book);

        let state = world.get_block_state(LECTERN);
        let hit = BlockHitResult {
            location: DVec3::new(4.5, 65.0, 4.5),
            direction: Direction::Up,
            block_pos: LECTERN,
            miss: false,
            inside: false,
            world_border_hit: false,
        };
        let mut inv =
            InventoryAccess::new(Arc::clone(&player.inventory), InteractionHand::MainHand);
        let result = BLOCK_BEHAVIORS.get_behavior(state.get_block()).use_item_on(
            state,
            world,
            LECTERN,
            &player,
            InteractionHand::MainHand,
            &hit,
            &mut inv,
        );
        assert!(result.consumes_action());

        let block_entity = world.get_block_entity(LECTERN).expect("a lectern");
        let lectern = block_entity
            .downcast_ref::<LecternBlockEntity>()
            .expect("lectern block entity");
        let book = lectern.book();
        let Some(content) = book.get(WRITTEN_BOOK_CONTENT) else {
            panic!("the lectern should hold the written book");
        };
        assert!(content.resolved());
        assert_eq!(
            content.pages()[0].raw().to_plain(&DisplayResolutor),
            "Reader"
        );
    });
}
