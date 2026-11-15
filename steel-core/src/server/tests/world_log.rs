use std::sync::Arc;

use glam::DVec3;
use steel_protocol::packets::game::{PlayerAction, SPlayerAction};
use steel_registry::{
    blocks::{block_state_ext::BlockStateExt as _, properties::Direction},
    init_vanilla_registry, vanilla_blocks,
};
use steel_utils::{
    BlockPos, BlockStateId, ChunkPos,
    locks::SyncMutex,
    types::{GameType, UpdateFlags},
};
use tokio::runtime::Builder;

use crate::{
    behavior::init_behaviors, entity::Entity as _, inventory::menu::Menu,
    permission::PermissionSubjectIndex, player::Player, server::world_log::WorldLog, world::World,
};

use super::{fresh_test_world, insert_ready_full_chunk, test_player, test_server};

/// Writes down what it hears; swallows clicks on blocks at `x == 9`.
#[derive(Default)]
struct Recorder {
    broken: SyncMutex<Vec<(BlockPos, BlockStateId)>>,
    clicked: SyncMutex<Vec<(BlockPos, bool)>>,
}

impl WorldLog for Recorder {
    fn block_broken(&self, _: &Player, _: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        self.broken.lock().push((pos, state));
    }

    fn block_placed(
        &self,
        _: &Player,
        _: &Arc<World>,
        _: BlockPos,
        _: BlockStateId,
        _: BlockStateId,
    ) {
    }

    fn block_clicked(&self, _: &Arc<Player>, _: &Arc<World>, pos: BlockPos, hit: bool) -> bool {
        self.clicked.lock().push((pos, hit));
        pos.x() == 9
    }

    fn menu_opened(&self, _: &Player, _: &Menu) {}

    fn menu_closed(&self, _: &Player, _: &Menu) {}
}

#[test]
fn the_world_log_hears_a_break_and_can_swallow_a_click() {
    init_vanilla_registry();
    init_behaviors();
    let world_fixture = fresh_test_world("world_log_break");
    let world = &world_fixture.world;
    let runtime = Builder::new_current_thread().enable_all().build();
    let Ok(runtime) = runtime else {
        panic!("test runtime should initialize");
    };
    runtime.block_on(async {
        let server = test_server(Arc::clone(world), PermissionSubjectIndex::new()).await;
        let Ok(server) = server else {
            panic!("test server should initialize");
        };
        let recorder = Arc::new(Recorder::default());
        assert!(server.set_world_log(Arc::clone(&recorder) as Arc<dyn WorldLog>));
        assert!(!server.set_world_log(Arc::clone(&recorder) as Arc<dyn WorldLog>));

        let broken = BlockPos::new(8, 64, 8);
        let kept = BlockPos::new(9, 64, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(broken));
        let stone = vanilla_blocks::STONE.default_state();
        for pos in [broken, kept] {
            assert!(world.set_block(pos, stone, UpdateFlags::UPDATE_ALL));
        }

        let player = test_player(&server, Arc::clone(world));
        player.set_client_loaded(true);
        player.base().set_position_local(DVec3::new(8.5, 65.0, 8.5));
        player.restore_game_modes(GameType::Creative, None);
        player
            .abilities
            .lock()
            .update_for_game_mode(GameType::Creative);

        for pos in [broken, kept] {
            player.handle_player_action(SPlayerAction {
                action: PlayerAction::StartDestroyBlock,
                pos,
                direction: Direction::Up,
                sequence: 1,
            });
        }

        assert_eq!(*recorder.clicked.lock(), [(broken, true), (kept, true)]);
        assert_eq!(*recorder.broken.lock(), [(broken, stone)]);
        assert!(world.get_block_state(broken).is_air());
        assert_eq!(world.get_block_state(kept), stone);
    });
}
