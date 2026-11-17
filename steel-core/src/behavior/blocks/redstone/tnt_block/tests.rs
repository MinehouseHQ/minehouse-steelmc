use steel_registry::data_component_predicate::DataComponentMatchers;
use steel_registry::data_components::vanilla_components::CAN_BREAK;
use steel_registry::data_components::{AdventureModePredicate, BlockPredicate};
use steel_registry::item_stack::ItemStack;
use steel_registry::{RegistryHolderSet, init_vanilla_registry};
use steel_utils::{ChunkPos, Direction, Downcast as _, WorldAabb};

use super::*;
use crate::behavior::init_behaviors;
use crate::behavior::items::projectile_weapon::create_arrow;
use crate::entity::SharedEntity;
use crate::player::ResetReason;
use crate::test_support::{TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk};
use crate::world::{ExplosionInteraction, ExplosionOptions};

#[test]
fn adventure_owner_primes_only_when_the_igniting_stack_can_break_tnt() {
    init_vanilla_registry();
    init_behaviors();
    let fixture = fresh_test_world("tnt_adventure_priming");
    let world = &fixture.world;
    let pos = BlockPos::new(8, 64, 8);
    insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
    let owner =
        TestPlayerBuilder::new(Arc::clone(world), "AdventureOwner", next_entity_id()).build();
    owner.restore_game_modes(GameType::Adventure, None);

    assert!(!TntBlock::prime_with_source(
        world,
        pos,
        Some(&*owner),
        || false
    ));
    assert!(TntBlock::prime_with_source(
        world,
        pos,
        Some(&*owner),
        || true
    ));
}

#[test]
fn explosion_replaces_tnt_with_a_short_fuse_entity_owned_by_the_igniter() {
    init_vanilla_registry();
    init_behaviors();
    let fixture = fresh_test_world("tnt_chain_reaction");
    let world = &fixture.world;
    let pos = BlockPos::new(8, 64, 8);
    insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
    assert!(world.set_block(
        pos,
        vanilla_blocks::TNT.default_state(),
        UpdateFlags::UPDATE_NONE,
    ));
    let owner = TestPlayerBuilder::new(Arc::clone(world), "ChainOwner", next_entity_id()).build();
    let (x, y, z) = pos.offset(4, 0, 0).get_bottom_center();
    owner.base().set_position_local(DVec3::new(x, y, z));
    assert!(world.add_player(Arc::clone(&owner), ResetReason::InitialJoin));
    let (x, y, z) = pos.get_center();
    let mut options = ExplosionOptions::new(DVec3::new(x, y, z), 4.0, ExplosionInteraction::Tnt);
    options.source = Some(Arc::clone(&owner) as SharedEntity);

    world.explode(options);

    assert!(world.get_block_state(pos).is_air());
    let (x, y, z) = (f64::from(pos.x()), f64::from(pos.y()), f64::from(pos.z()));
    let primed = world
        .get_entities_in_aabb(&WorldAabb::new(x, y, z, x + 1.0, y + 1.0, z + 1.0))
        .into_iter()
        .filter(|entity| entity.downcast_ref::<PrimedTntEntity>().is_some())
        .collect::<Vec<_>>();
    let [primed] = primed.as_slice() else {
        panic!("expected exactly one primed TNT, found {}", primed.len());
    };
    let fuse = primed
        .downcast_ref::<PrimedTntEntity>()
        .map(PrimedTntEntity::fuse)
        .expect("spawned entity should remain primed TNT");
    // `getRandomShortFuse(80)` is `nextInt(20) + 10`.
    assert!((10..30).contains(&fuse), "fuse={fuse}");
    assert_eq!(
        primed.explosion_indirect_source().map(|source| source.id()),
        Some(owner.id())
    );
}

fn burning_arrow_hits_tnt(world: &Arc<World>, owner: &SharedEntity, ammo: &ItemStack) -> bool {
    let pos = BlockPos::new(8, 64, 8);
    world.set_block(
        pos,
        vanilla_blocks::TNT.default_state(),
        UpdateFlags::UPDATE_NONE,
    );
    assert_eq!(
        world.get_block_state(pos),
        vanilla_blocks::TNT.default_state()
    );
    let arrow = create_arrow(world, ammo, owner, None);
    arrow.set_remaining_fire_ticks(100);
    let Some(arrow) = as_abstract_arrow_entity(&*arrow) else {
        panic!("created arrow should be an abstract arrow");
    };
    let (x, y, z) = pos.get_center();
    TntBlock::new(&vanilla_blocks::TNT).on_projectile_hit(
        vanilla_blocks::TNT.default_state(),
        world,
        &ClipHitResult {
            location: DVec3::new(x, y, z),
            direction: Direction::Up,
            block_pos: pos,
            miss: false,
            inside: false,
            world_border_hit: false,
        },
        arrow,
    );
    world.get_block_state(pos).is_air()
}

#[test]
fn burning_arrow_primes_tnt_for_adventure_owner_only_with_can_break_ammo() {
    init_vanilla_registry();
    init_behaviors();
    let fixture = fresh_test_world("tnt_adventure_arrow");
    let world = &fixture.world;
    insert_ready_full_chunk(world, ChunkPos::from_block_pos(BlockPos::new(8, 64, 8)));
    let owner = TestPlayerBuilder::new(Arc::clone(world), "ArrowOwner", next_entity_id()).build();
    owner.restore_game_modes(GameType::Adventure, None);
    let owner = Arc::clone(&owner) as SharedEntity;

    assert!(!burning_arrow_hits_tnt(
        world,
        &owner,
        &ItemStack::new(&vanilla_items::ARROW)
    ));

    let predicate = BlockPredicate::new(
        Some(RegistryHolderSet::Direct(vec![&vanilla_blocks::TNT])),
        None,
        None,
        DataComponentMatchers::ANY,
    );
    let mut ammo = ItemStack::new(&vanilla_items::ARROW);
    ammo.set(
        CAN_BREAK,
        AdventureModePredicate::new(vec![predicate]).expect("one block predicate is valid"),
    );
    assert!(burning_arrow_hits_tnt(world, &owner, &ammo));
}
