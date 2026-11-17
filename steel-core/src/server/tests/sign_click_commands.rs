//! Waxed signs running their click commands, as vanilla's
//! `SignBlock.useWithoutItem` runs them through
//! `SignBlockEntity.executeClickCommandsIfPresent`.

use std::io::Cursor;
use std::sync::Arc;

use glam::DVec3;
use simdnbt::borrow::read_compound;
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::Direction;
use steel_registry::{init_vanilla_registry, vanilla_blocks};
use steel_utils::types::{InteractionHand, SignTextSlot, UpdateFlags};
use steel_utils::{BlockPos, ChunkPos, Downcast as _};
use text_components::interactivity::ClickEvent;
use text_components::{Modifier as _, TextComponent};

use super::insert_ready_full_chunk;
use super::scoreboard_command::with_server_world;
use crate::behavior::{
    BLOCK_BEHAVIORS, BlockHitResult, InteractionResult, InventoryAccess, init_behaviors,
};
use crate::block_entity::entities::{SignBlockEntity, SignText};
use crate::block_entity::{BlockEntity as _, init_block_entities};
use crate::command::PendingCommandExecutionQueue;
use crate::player::Player;
use crate::server::Server;
use crate::test_support::TestPlayerBuilder;
use crate::world::{CommandBlockRun, World};

const SIGN: BlockPos = BlockPos::new(4, 64, 4);

fn with_world(name: &'static str, test: impl FnOnce(&Arc<Server>, &Arc<World>)) {
    init_vanilla_registry();
    init_behaviors();
    init_block_entities();
    with_server_world(name, |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        test(server, world);
    });
}

/// Places an oak sign whose sides both run `command` when clicked, loading
/// its wax and op features from NBT as vanilla does.
fn place_sign(world: &Arc<World>, waxed: bool, allow_op_features: bool, command: &str) {
    let state = vanilla_blocks::OAK_SIGN.default_state();
    assert!(world.set_block(SIGN, state, UpdateFlags::UPDATE_ALL));
    let block_entity = world.get_block_entity(SIGN).expect("a sign block entity");
    let sign = block_entity
        .downcast_ref::<SignBlockEntity>()
        .expect("sign block entity");

    let mut nbt = NbtCompound::new();
    nbt.insert("is_waxed", i8::from(waxed));
    if allow_op_features {
        nbt.insert("allow_op_features", 1i8);
    }
    let mut bytes = Vec::new();
    nbt.write(&mut bytes);
    let borrowed =
        read_compound(&mut Cursor::new(bytes.as_slice())).expect("test sign NBT should reborrow");
    sign.load_additional(&borrowed);

    let mut text = SignText::new();
    text.set_message(
        0,
        TextComponent::plain("Click").click_event(ClickEvent::run_command(command.to_owned())),
    );
    sign.set_text(text.clone(), SignTextSlot::Front);
    sign.set_text(text, SignTextSlot::Back);
}

fn player(server: &Arc<Server>, world: &Arc<World>) -> Arc<Player> {
    let player = TestPlayerBuilder::new(Arc::clone(world), "Clicker", 7)
        .server(server)
        .build();
    assert!(world.players.insert(Arc::clone(&player)));
    assert!(server.online_players.insert(Arc::clone(&player)));
    player
}

fn click(world: &Arc<World>, player: &Player) -> InteractionResult {
    let state = world.get_block_state(SIGN);
    let hit = BlockHitResult {
        location: DVec3::new(4.5, 64.5, 4.5),
        direction: Direction::Up,
        block_pos: SIGN,
        miss: false,
        inside: false,
        world_border_hit: false,
    };
    let mut inv = InventoryAccess::new(Arc::clone(&player.inventory), InteractionHand::MainHand);
    BLOCK_BEHAVIORS
        .get_behavior(state.get_block())
        .use_without_item(state, world, SIGN, player, &hit, &mut inv)
}

#[test]
fn a_waxed_sign_with_op_features_runs_its_command_at_its_center() {
    with_world("sign_click_runs_command", |server, world| {
        place_sign(world, true, true, "/setblock ~ ~2 ~ stone");
        let player = player(server, world);

        assert!(click(world, &player).consumes_action());
        let mut pending = PendingCommandExecutionQueue::new();
        server.tick_command_blocks(&mut pending);

        assert_eq!(
            world.get_block_state(SIGN.above().above()).get_block(),
            &vanilla_blocks::STONE,
            "the command runs at the sign's center"
        );
    });
}

#[test]
fn click_commands_without_op_features_do_not_run() {
    with_world("sign_click_disabled", |server, world| {
        place_sign(world, true, false, "/setblock ~ ~2 ~ stone");
        let player = player(server, world);

        assert!(click(world, &player).consumes_action());
        assert!(
            world.drain_command_block_runs().is_empty(),
            "vanilla only tells the player click actions are disabled"
        );
    });
}

#[test]
fn an_unwaxed_sign_with_click_commands_runs_them_instead_of_opening_the_editor() {
    with_world("sign_click_unwaxed", |server, world| {
        place_sign(world, false, true, "/say hi");
        let player = player(server, world);

        assert!(click(world, &player).consumes_action());
        let runs = world.drain_command_block_runs();
        assert_eq!(
            runs,
            vec![CommandBlockRun::Sign {
                pos: SIGN,
                player: player.gameprofile.id(),
                command: "/say hi".to_owned(),
            }]
        );
        let block_entity = world.get_block_entity(SIGN).expect("sign");
        let sign = block_entity
            .downcast_ref::<SignBlockEntity>()
            .expect("sign block entity");
        assert_eq!(sign.get_player_who_may_edit(), None, "no editor opens");
    });
}

#[test]
fn op_features_save_only_when_allowed() {
    with_world("sign_op_features_saved", |_server, world| {
        place_sign(world, true, true, "/say hi");
        let block_entity = world.get_block_entity(SIGN).expect("sign");
        let mut nbt = NbtCompound::new();
        block_entity.save_additional(&mut nbt);
        assert_eq!(nbt.byte("allow_op_features"), Some(1));

        assert!(world.set_block(
            SIGN,
            vanilla_blocks::AIR.default_state(),
            UpdateFlags::UPDATE_ALL
        ));
        place_sign(world, true, false, "/say hi");
        let block_entity = world.get_block_entity(SIGN).expect("sign");
        let mut nbt = NbtCompound::new();
        block_entity.save_additional(&mut nbt);
        assert_eq!(nbt.byte("allow_op_features"), None);
    });
}
