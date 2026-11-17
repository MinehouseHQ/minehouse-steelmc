//! `force-gamemode`: vanilla `MinecraftServer.enforceGameTypeForPlayers`
//! from `/defaultgamemode`, and `ServerPlayer.calculateGameModeForNewPlayer`
//! for returning players.

use std::slice;
use std::sync::Arc;

use steel_registry::init_vanilla_registry;
use steel_utils::types::GameType;
use tokio::runtime::Builder;

use super::scoreboard_command::run;
use super::{
    fresh_test_world, test_player_with_packets, test_runtime_config_with_max_players,
    test_server_with_worlds_and_config,
};
use crate::behavior::init_behaviors;
use crate::config::{ResolvedDomainConfig, RuntimeConfig};
use crate::entity::next_entity_id;
use crate::permission::PermissionSubjectIndex;
use crate::player::{Player, ResetReason};
use crate::server::Server;
use crate::world::World;

fn with_joined_player(
    name: &'static str,
    force_gamemode: bool,
    test: impl FnOnce(&Arc<Server>, &Arc<World>, &Arc<Player>),
) {
    init_vanilla_registry();
    init_behaviors();
    let fixture = fresh_test_world(name);
    let world = &fixture.world;
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    runtime.block_on(async {
        let config = RuntimeConfig {
            force_gamemode,
            ..(*test_runtime_config_with_max_players(1)).clone()
        };
        let domain = ResolvedDomainConfig {
            name: world.domain().to_owned(),
            default_world: world.key.clone(),
            worlds: vec![world.key.clone()],
        };
        let server = test_server_with_worlds_and_config(
            domain.name.clone(),
            slice::from_ref(&domain),
            slice::from_ref(world),
            PermissionSubjectIndex::new(),
            Arc::new(config),
        )
        .await
        .expect("test server");
        let player =
            test_player_with_packets(&server, Arc::clone(world), "Player", next_entity_id()).0;
        assert!(server.online_players.insert(Arc::clone(&player)));
        assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));
        let _ = player.mark_joined_world();
        test(&server, world, &player);
    });
}

#[test]
fn defaultgamemode_moves_online_players_when_forced() {
    with_joined_player("force_gamemode_on", true, |server, _world, player| {
        assert_eq!(player.game_mode(), GameType::Survival);
        assert_eq!(run(server, "defaultgamemode creative"), Some(1));
        assert_eq!(player.game_mode(), GameType::Creative);
        assert_eq!(
            run(server, "defaultgamemode creative"),
            Some(0),
            "players already in the mode are not counted"
        );
    });
}

#[test]
fn defaultgamemode_leaves_players_alone_when_not_forced() {
    with_joined_player("force_gamemode_off", false, |server, _world, player| {
        assert_eq!(run(server, "defaultgamemode creative"), Some(0));
        assert_eq!(player.game_mode(), GameType::Survival);
    });
}

#[test]
fn a_returning_player_takes_the_forced_default_mode() {
    with_joined_player("force_gamemode_join", true, |_server, world, player| {
        player.restore_game_modes(GameType::Survival, Some(GameType::Spectator));
        world.set_default_gamemode(GameType::Adventure);
        Server::force_default_game_mode(player, world);
        assert_eq!(player.game_mode(), GameType::Adventure);
        assert_eq!(player.previous_game_mode(), Some(GameType::Spectator));
    });
}
