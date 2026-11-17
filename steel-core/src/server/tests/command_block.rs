//! Command blocks running their commands through the server, as vanilla's
//! `CommandBlock.tick` runs them.

use std::io::Cursor;
use std::sync::Arc;

use simdnbt::borrow::read_compound;
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction};
use steel_registry::vanilla_game_rules::{COMMAND_BLOCKS_WORK, SEND_COMMAND_FEEDBACK};
use steel_registry::{init_vanilla_registry, vanilla_blocks};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos, Downcast as _};
use text_components::TextComponent;

use super::insert_ready_full_chunk;
use super::scoreboard_command::with_server_world;
use crate::behavior::blocks::run_command_block_tick;
use crate::behavior::{BLOCK_BEHAVIORS, init_behaviors};
use crate::block_entity::entities::CommandBlockEntity;
use crate::block_entity::{SharedBlockEntity, init_block_entities};
use crate::command::PendingCommandExecutionQueue;
use crate::command::command_block::{BaseCommandBlock, CommandBlockRunner};
use crate::entity::init_entities;
use crate::server::Server;
use crate::world::World;

const ORIGIN: BlockPos = BlockPos::new(4, 64, 4);

fn init() {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    init_block_entities();
}

/// Places a command block facing east at `pos` and returns its entity.
fn place(
    world: &Arc<World>,
    pos: BlockPos,
    block: BlockRef,
    conditional: bool,
    command: &str,
) -> SharedBlockEntity {
    let state = block
        .default_state()
        .set_value(&BlockStateProperties::FACING, Direction::East)
        .set_value(&BlockStateProperties::CONDITIONAL, conditional);
    assert!(world.set_block(pos, state, UpdateFlags::UPDATE_ALL));
    let block_entity = world
        .get_block_entity(pos)
        .expect("a command block has a block entity");
    command_block(&block_entity).set_command(command.to_owned());
    block_entity
}

fn command_block(block_entity: &SharedBlockEntity) -> &CommandBlockEntity {
    block_entity
        .downcast_ref::<CommandBlockEntity>()
        .expect("command block entity")
}

/// Powers the impulse block at `pos` and runs its queued tick.
fn pulse(server: &Arc<Server>, world: &Arc<World>, pos: BlockPos) {
    let block_entity = world.get_block_entity(pos).expect("command block");
    command_block(&block_entity).mark_condition_met(world);
    let mut pending = PendingCommandExecutionQueue::new();
    let mut runner = CommandBlockRunner {
        server,
        pending: &mut pending,
    };
    run_command_block_tick(world, pos, &mut runner);
}

fn with_world(name: &'static str, test: impl FnOnce(&Arc<Server>, &Arc<World>)) {
    init();
    with_server_world(name, |server, world| {
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        test(server, world);
    });
}

#[test]
fn an_impulse_block_runs_its_command_and_keeps_the_output() {
    with_world("command_block_impulse", |server, world| {
        let block_entity = place(
            world,
            ORIGIN,
            &vanilla_blocks::COMMAND_BLOCK,
            false,
            "/setblock ~ ~2 ~ stone",
        );
        pulse(server, world, ORIGIN);

        assert_eq!(
            world.get_block_state(ORIGIN.above().above()).get_block(),
            &vanilla_blocks::STONE,
            "the command runs at the block's center"
        );
        let entity = command_block(&block_entity);
        assert_eq!(entity.command_block().success_count(), 1);
        let output = entity.command_block().last_output().to_string();
        assert!(
            output.starts_with('[') && output.len() > "[00:00:00] ".len(),
            "the output is stamped with the time: {output}"
        );
        let behavior = BLOCK_BEHAVIORS.get_behavior(&vanilla_blocks::COMMAND_BLOCK);
        assert_eq!(
            behavior.get_analog_output_signal(
                world.get_block_state(ORIGIN),
                world,
                ORIGIN,
                Direction::West
            ),
            1,
            "comparators read the success count"
        );
    });
}

#[test]
fn a_command_block_runs_at_most_once_per_game_tick() {
    with_world("command_block_once_per_tick", |server, world| {
        let block_entity = place(
            world,
            ORIGIN,
            &vanilla_blocks::COMMAND_BLOCK,
            false,
            "/say hi",
        );
        let entity = command_block(&block_entity);
        let mut pending = PendingCommandExecutionQueue::new();
        let mut runner = CommandBlockRunner {
            server,
            pending: &mut pending,
        };
        let base = entity.command_block();
        assert!(base.perform_command(&mut runner, world, entity.origin()));
        assert!(!base.perform_command(&mut runner, world, entity.origin()));
    });
}

#[test]
fn a_conditional_chain_block_follows_the_block_before_it() {
    with_world("command_block_chain", |server, world| {
        let chain_pos = ORIGIN.relative(Direction::East);
        let target = BlockPos::new(4, 70, 4);
        place(
            world,
            ORIGIN,
            &vanilla_blocks::COMMAND_BLOCK,
            false,
            "/setblock 4 70 4 stone",
        );
        let chain = place(
            world,
            chain_pos,
            &vanilla_blocks::CHAIN_COMMAND_BLOCK,
            true,
            "/setblock 4 71 4 gold_block",
        );
        assert!(
            command_block(&chain).is_automatic(),
            "chain blocks start always active"
        );

        pulse(server, world, ORIGIN);
        assert_eq!(
            world.get_block_state(target.above()).get_block(),
            &vanilla_blocks::GOLD_BLOCK,
            "the condition holds after the first block succeeded"
        );

        // The same `setblock` now fails, so the conditional block is skipped.
        world.set_block(
            target.above(),
            vanilla_blocks::AIR.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        world.level_data.write().advance_game_time();
        pulse(server, world, ORIGIN);
        assert_eq!(
            world.get_block_state(target.above()).get_block(),
            &vanilla_blocks::AIR
        );
        assert_eq!(command_block(&chain).command_block().success_count(), 0);
    });
}

#[test]
fn disabled_command_blocks_do_nothing() {
    with_world("command_block_disabled", |server, world| {
        world.set_game_rule(&COMMAND_BLOCKS_WORK, false);
        let block_entity = place(
            world,
            ORIGIN,
            &vanilla_blocks::COMMAND_BLOCK,
            false,
            "/setblock ~ ~2 ~ stone",
        );
        pulse(server, world, ORIGIN);
        assert_eq!(
            world.get_block_state(ORIGIN.above().above()).get_block(),
            &vanilla_blocks::AIR
        );
        assert_eq!(
            command_block(&block_entity).command_block().success_count(),
            0
        );
    });
}

#[test]
fn an_untracked_block_keeps_no_output() {
    with_world("command_block_untracked", |server, world| {
        world.set_game_rule(&SEND_COMMAND_FEEDBACK, true);
        let block_entity = place(
            world,
            ORIGIN,
            &vanilla_blocks::COMMAND_BLOCK,
            false,
            "/setblock ~ ~2 ~ stone",
        );
        command_block(&block_entity)
            .command_block()
            .set_track_output(false);
        pulse(server, world, ORIGIN);
        assert_eq!(
            command_block(&block_entity)
                .command_block()
                .last_output()
                .to_string(),
            ""
        );
    });
}

fn round_trip(block: &BaseCommandBlock) -> (NbtCompound, BaseCommandBlock) {
    let mut nbt = NbtCompound::new();
    block.save(&mut nbt);
    let mut bytes = Vec::new();
    nbt.write(&mut bytes);
    let borrowed = read_compound(&mut Cursor::new(bytes.as_slice())).expect("nbt");
    let loaded = BaseCommandBlock::new();
    loaded.load(&(&borrowed).into());
    (nbt, loaded)
}

#[test]
fn the_command_block_state_saves_under_vanilla_keys() {
    init_vanilla_registry();
    let block = BaseCommandBlock::new();
    block.set_command("say hi".to_owned());
    block.set_success_count(3);
    block.set_custom_name(Some(TextComponent::plain("Door")));
    block.set_last_output(Some(TextComponent::plain("[12:00:00] hi")));

    let (nbt, loaded) = round_trip(&block);
    assert_eq!(
        nbt.string("Command").map(ToString::to_string).as_deref(),
        Some("say hi")
    );
    assert_eq!(nbt.int("SuccessCount"), Some(3));
    assert_eq!(nbt.byte("TrackOutput"), Some(1));
    assert_eq!(nbt.byte("UpdateLastExecution"), Some(1));
    assert!(nbt.get("LastExecution").is_none(), "it never ran");
    assert_eq!(loaded.command(), "say hi");
    assert_eq!(loaded.success_count(), 3);
    assert_eq!(loaded.name(), TextComponent::plain("Door"));
    assert_eq!(loaded.last_output(), TextComponent::plain("[12:00:00] hi"));

    block.set_track_output(false);
    let (nbt, loaded) = round_trip(&block);
    assert!(
        nbt.get("LastOutput").is_none(),
        "untracked output is not saved"
    );
    assert_eq!(loaded.last_output(), TextComponent::plain(""));
    assert_eq!(BaseCommandBlock::new().name(), TextComponent::plain("@"));
}
