use std::io::Cursor;

use simdnbt::borrow::read_compound;
use steel_registry::items::item::BlockHitResult;
use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_items};
use steel_utils::types::{GameType, UpdateFlags};
use steel_utils::{ChunkPos, Direction, Downcast as _};
use text_components::TextComponent;

use super::*;
use crate::behavior::items::CushionItem;
use crate::behavior::{ItemBehavior, UseOnContext, init_behaviors};
use crate::entity::entities::ItemEntity;
use crate::entity::{init_entities, next_entity_id};
use crate::player::ResetReason;
use crate::test_support::{
    TestPlayerBuilder, TestWorld, dropped_items, fresh_test_world, insert_ready_full_chunk,
};

struct Fixture {
    fixture: TestWorld,
    player: Arc<Player>,
}

impl Fixture {
    fn new(key: &'static str) -> Self {
        init_vanilla_registry();
        init_behaviors();
        init_entities();
        let fixture = fresh_test_world(key);
        insert_ready_full_chunk(&fixture.world, ChunkPos::new(0, 0));
        let player =
            TestPlayerBuilder::new(Arc::clone(&fixture.world), "Sitter", next_entity_id()).build();
        assert!(
            fixture
                .world
                .add_player(Arc::clone(&player), ResetReason::InitialJoin)
        );
        Self { fixture, player }
    }

    fn world(&self) -> &Arc<World> {
        &self.fixture.world
    }

    fn set_block(&self, pos: BlockPos, block: BlockRef) {
        assert!(
            self.world()
                .set_block(pos, block.default_state(), UpdateFlags::UPDATE_ALL)
        );
    }

    /// Uses `stack` on `face` of `pos`, clicking at `location`.
    fn use_on(
        &self,
        stack: ItemStack,
        pos: BlockPos,
        face: Direction,
        location: DVec3,
    ) -> InteractionResult {
        self.player
            .inventory
            .lock()
            .set_item_in_hand(InteractionHand::MainHand, stack);
        let mut context = UseOnContext::new(
            &self.player,
            InteractionHand::MainHand,
            BlockHitResult {
                location,
                direction: face,
                block_pos: pos,
                miss: false,
                inside: false,
                world_border_hit: false,
            },
            self.world(),
            Arc::clone(&self.player.inventory),
        );
        CushionItem.use_on(&mut context)
    }

    /// Clicks the middle of the top face of `pos`.
    fn place_on_top(&self, stack: ItemStack, pos: BlockPos) -> InteractionResult {
        self.use_on(
            stack,
            pos,
            Direction::Up,
            pos.0.as_dvec3() + DVec3::new(0.5, 1.0, 0.5),
        )
    }

    fn cushions_near(&self, pos: BlockPos) -> Vec<SharedEntity> {
        self.world().get_entities_in_aabb_matching(
            &WorldAabb::of_size(pos.0.as_dvec3(), 5.0, 5.0, 5.0),
            |entity| entity.downcast_ref::<CushionEntity>().is_some(),
        )
    }

    fn held_count(&self) -> i32 {
        self.player
            .inventory
            .lock()
            .get_item_in_hand(InteractionHand::MainHand)
            .count()
    }

    fn drops_near(&self, pos: DVec3) -> Vec<SharedEntity> {
        dropped_items(self.world(), pos)
            .into_iter()
            .filter(|entity| entity.downcast_ref::<ItemEntity>().is_some())
            .collect()
    }
}

fn cushion(entity: &SharedEntity) -> &CushionEntity {
    entity.downcast_ref::<CushionEntity>().expect("a cushion")
}

#[test]
fn a_cushion_goes_on_top_faces_only_once_per_spot_and_keeps_its_color() {
    let fx = Fixture::new("cushion_place");
    let ground = BlockPos::new(3, 64, 3);
    fx.set_block(ground, &vanilla_blocks::STONE);

    assert_eq!(
        fx.use_on(
            ItemStack::with_count(&vanilla_items::ORANGE_CUSHION, 2),
            ground,
            Direction::North,
            ground.0.as_dvec3() + DVec3::new(0.5, 0.5, 0.0),
        ),
        InteractionResult::Fail
    );
    assert_eq!(
        fx.place_on_top(
            ItemStack::with_count(&vanilla_items::ORANGE_CUSHION, 2),
            ground
        ),
        InteractionResult::Success
    );
    assert_eq!(fx.held_count(), 1);
    let cushions = fx.cushions_near(ground);
    assert_eq!(cushions.len(), 1);
    let placed = cushion(&cushions[0]);
    assert_eq!(placed.position(), DVec3::new(3.5, 65.0, 3.5));
    assert_eq!(placed.color(), DyeColor::Orange);
    assert_eq!(placed.block_attached_entity_base.pos(), ground.above());
    assert!(placed.survives());

    // A second cushion does not fit on the same spot.
    assert_eq!(
        fx.place_on_top(ItemStack::new(&vanilla_items::ORANGE_CUSHION), ground),
        InteractionResult::Fail
    );
    assert_eq!(fx.cushions_near(ground).len(), 1);
}

#[test]
fn cushions_cover_cauldrons_but_not_spots_under_a_solid_block() {
    let fx = Fixture::new("cushion_cauldron");
    let cauldron = BlockPos::new(6, 64, 6);
    fx.set_block(cauldron, &vanilla_blocks::CAULDRON);
    fx.player
        .try_set_position(DVec3::new(6.5, 66.0, 4.5))
        .expect("move player");
    assert_eq!(
        fx.place_on_top(ItemStack::new(&vanilla_items::WHITE_CUSHION), cauldron),
        InteractionResult::Success
    );
    let cushions = fx.cushions_near(cauldron);
    assert_eq!(cushions.len(), 1);
    // The click is recast against the cauldron's collision shape; a ray that
    // stops just past the open top misses it, so the cushion covers the top.
    assert_eq!(cushions[0].position(), DVec3::new(6.5, 65.0, 6.5));

    // Stone right above the clicked block leaves no room to sit.
    let ground = BlockPos::new(9, 64, 9);
    fx.set_block(ground, &vanilla_blocks::STONE);
    fx.set_block(ground.above(), &vanilla_blocks::STONE);
    assert_eq!(
        fx.place_on_top(ItemStack::new(&vanilla_items::WHITE_CUSHION), ground),
        InteractionResult::Fail
    );
}

#[test]
fn players_sit_on_cushions_unless_sneaking_or_taken() {
    let fx = Fixture::new("cushion_sit");
    let ground = BlockPos::new(3, 64, 3);
    fx.set_block(ground, &vanilla_blocks::STONE);
    assert_eq!(
        fx.place_on_top(ItemStack::new(&vanilla_items::BLUE_CUSHION), ground),
        InteractionResult::Success
    );
    let seat = Arc::clone(&fx.cushions_near(ground)[0]);

    fx.player.set_crouching(true);
    assert_eq!(
        seat.interact(&fx.player, InteractionHand::MainHand, seat.position()),
        InteractionResult::Pass
    );
    fx.player.set_crouching(false);
    assert_eq!(
        seat.interact(&fx.player, InteractionHand::MainHand, seat.position()),
        InteractionResult::Success
    );
    assert!(seat.has_passenger(fx.player.as_ref()));
    assert_eq!(
        seat.interact(&fx.player, InteractionHand::MainHand, seat.position()),
        InteractionResult::Pass
    );
}

#[test]
fn broken_cushions_drop_their_item_unless_a_creative_player_broke_them() {
    let fx = Fixture::new("cushion_drops");
    let source = DamageSource::environment(&vanilla_damage_types::PLAYER_ATTACK)
        .with_causing_entity(Arc::clone(&fx.player) as SharedEntity);
    let place = |ground: BlockPos| {
        fx.set_block(ground, &vanilla_blocks::STONE);
        let mut stack = ItemStack::new(&vanilla_items::LIME_CUSHION);
        stack.set(CUSTOM_NAME, TextComponent::plain("Comfy"));
        assert_eq!(fx.place_on_top(stack, ground), InteractionResult::Success);
        Arc::clone(&fx.cushions_near(ground)[0])
    };

    let seat = place(BlockPos::new(2, 64, 2));
    assert!(seat.hurt(fx.world(), &source, 1.0));
    assert!(seat.is_removed());
    let drops = fx.drops_near(seat.position());
    assert_eq!(drops.len(), 1);
    let item = drops[0]
        .downcast_ref::<ItemEntity>()
        .expect("an item")
        .get_item();
    assert!(item.is(&vanilla_items::LIME_CUSHION));
    assert_eq!(item.get(CUSTOM_NAME), Some(&TextComponent::plain("Comfy")));

    fx.player.restore_game_modes(GameType::Creative, None);
    fx.player
        .abilities
        .lock()
        .update_for_game_mode(GameType::Creative);
    let seat = place(BlockPos::new(10, 64, 10));
    assert!(seat.hurt(fx.world(), &source, 1.0));
    assert!(seat.is_removed());
    assert!(fx.drops_near(seat.position()).is_empty());

    // Adventure players may not build, so they cannot break cushions.
    fx.player.restore_game_modes(GameType::Survival, None);
    let seat = place(BlockPos::new(6, 64, 12));
    fx.player.abilities.lock().may_build = false;
    assert!(!seat.hurt(fx.world(), &source, 1.0));
    assert!(!seat.is_removed());
}

#[test]
fn cushions_fall_off_without_ground_and_burn_in_fire() {
    let fx = Fixture::new("cushion_survival");
    let ground = BlockPos::new(3, 64, 3);
    fx.set_block(ground, &vanilla_blocks::STONE);
    assert_eq!(
        fx.place_on_top(ItemStack::new(&vanilla_items::RED_CUSHION), ground),
        InteractionResult::Success
    );
    let seat = Arc::clone(&fx.cushions_near(ground)[0]);
    fx.set_block(ground, &vanilla_blocks::AIR);
    for _ in 0..100 {
        Arc::clone(&seat).tick();
        assert!(!seat.is_removed());
    }
    Arc::clone(&seat).tick();
    assert!(seat.is_removed());
    assert_eq!(fx.drops_near(seat.position()).len(), 1);

    let ground = BlockPos::new(9, 64, 9);
    fx.set_block(ground, &vanilla_blocks::NETHERRACK);
    assert_eq!(
        fx.place_on_top(ItemStack::new(&vanilla_items::RED_CUSHION), ground),
        InteractionResult::Success
    );
    let seat = Arc::clone(&fx.cushions_near(ground)[0]);
    fx.set_block(ground.above(), &vanilla_blocks::FIRE);
    cushion(&seat).tick_at_check_interval();
    assert!(seat.is_removed());
}

#[test]
fn lightning_breaks_cushions_and_shields_the_dropped_item_briefly() {
    let fx = Fixture::new("cushion_lightning");
    let ground = BlockPos::new(3, 64, 3);
    fx.set_block(ground, &vanilla_blocks::STONE);
    assert_eq!(
        fx.place_on_top(ItemStack::new(&vanilla_items::CYAN_CUSHION), ground),
        InteractionResult::Success
    );
    let seat = Arc::clone(&fx.cushions_near(ground)[0]);
    let lightning = LightningBoltEntity::new(
        &vanilla_entities::LIGHTNING_BOLT,
        next_entity_id(),
        seat.position(),
        Arc::downgrade(fx.world()),
    );
    seat.thunder_hit(fx.world(), &lightning);
    assert!(seat.is_removed());
    let drops = fx.drops_near(seat.position());
    assert_eq!(drops.len(), 1);
    let item = &drops[0];
    assert_eq!(item.invulnerable_time(), LIGHTNING_DROP_INVULNERABLE_TICKS);
    assert!(!item.hurt(
        fx.world(),
        &DamageSource::environment(&vanilla_damage_types::LIGHTNING_BOLT),
        5.0
    ));
    for _ in 0..LIGHTNING_DROP_INVULNERABLE_TICKS {
        item.advance_tick_count();
    }
    assert!(!item.is_invulnerable());
}

#[test]
fn cushions_round_trip_their_color_and_block_through_nbt() {
    let fx = Fixture::new("cushion_nbt");
    let ground = BlockPos::new(3, 64, 3);
    fx.set_block(ground, &vanilla_blocks::STONE);
    assert_eq!(
        fx.place_on_top(ItemStack::new(&vanilla_items::PURPLE_CUSHION), ground),
        InteractionResult::Success
    );
    let seat = Arc::clone(&fx.cushions_near(ground)[0]);
    let mut nbt = NbtCompound::new();
    seat.save_additional(&mut nbt);
    assert_eq!(
        nbt.string("color").map(|color| color.to_str().into_owned()),
        Some("purple".to_owned())
    );
    let mut bytes = Vec::new();
    nbt.write(&mut bytes);
    let borrowed = read_compound(&mut Cursor::new(&bytes)).expect("nbt");
    let loaded = CushionEntity::new(
        &vanilla_entities::CUSHION,
        next_entity_id(),
        seat.position(),
        Arc::downgrade(fx.world()),
    );
    loaded.load_additional((&borrowed).into());
    assert_eq!(loaded.color(), DyeColor::Purple);
    assert_eq!(loaded.block_attached_entity_base.pos(), ground.above());
}
