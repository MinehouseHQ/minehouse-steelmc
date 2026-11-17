use super::*;
use steel_registry::init_vanilla_registry;

#[test]
fn distance_fields_decode_as_predicates_or_bounds() {
    let predicate: Conditions = serde_json::from_str(r#"{"distance": {"y": {"min": 50.0}}}"#)
        .expect("a distance predicate decodes");
    assert!(matches!(
        predicate.distance,
        Some(DistanceField::Distance(_))
    ));
    let bounds: Conditions =
        serde_json::from_str(r#"{"distance": {"min": 2.0}}"#).expect("ender eye bounds decode");
    assert!(matches!(bounds.distance, Some(DistanceField::Bounds(_))));
}

#[test]
fn every_vanilla_criterion_decodes() {
    init_vanilla_registry();
    let mut failures = Vec::new();
    for &advancement in REGISTRY.advancements.advancements() {
        for (criterion, data) in advancement.criteria() {
            if let Err(error) = serde_json::from_str::<Conditions>(data.conditions) {
                failures.push(format!("{} {criterion}: {error}", advancement.key()));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

mod triggers {
    use std::sync::Arc;

    use steel_registry::item_stack::ItemStack;
    use steel_registry::{vanilla_items, vanilla_menu_types};
    use steel_utils::Identifier;
    use steel_utils::locks::IntoShared as _;

    use super::super::predicates::LocationPredicate;
    use super::super::{AVOID_VIBRATION, TICK, init_criteria};
    use crate::behavior::init_behaviors;
    use crate::entity::entities::{ItemEntity, LightningBoltEntity};
    use crate::entity::init_entities;
    use crate::entity::next_entity_id;
    use crate::entity::{Entity, SharedEntity};
    use crate::inventory::container::{Container as _, SimpleContainer};
    use crate::inventory::menu::{MenuBuilder, kinds::BasicKind};
    use crate::player::Player;
    use crate::player::ResetReason;
    use crate::test_support::{
        TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk,
    };
    use crate::world::World;
    use steel_registry::entity_type::EntityTypeRef;
    use steel_registry::vanilla_blocks;

    fn setup() {
        steel_registry::init_vanilla_registry();
        init_behaviors();
        init_criteria();
    }

    fn criterion_done(player: &Player, advancement: &str, criterion: &str) -> bool {
        let key: Identifier = advancement.parse().expect("advancement id parses");
        let advancement = REGISTRY
            .advancements
            .value_by_key(&key)
            .expect("vanilla advancement exists");
        player
            .advancements
            .lock()
            .progress
            .get_progress(advancement)
            .is_some_and(|progress| progress.is_criterion_done(criterion))
    }

    use super::REGISTRY;

    #[test]
    fn inventory_changed_grants_on_join_and_on_change() {
        setup();
        let fixture = fresh_test_world("criteria_inventory");
        let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "Crafter", 1).build();

        player
            .inventory
            .lock()
            .set_item(0, ItemStack::new(&vanilla_items::DIRT));
        player.init_inventory_menu();
        assert!(!criterion_done(
            &player,
            "minecraft:story/root",
            "crafting_table"
        ));

        player
            .inventory
            .lock()
            .set_item(1, ItemStack::new(&vanilla_items::CRAFTING_TABLE));
        player.broadcast_inventory_changes_and_trigger();
        assert!(criterion_done(
            &player,
            "minecraft:story/root",
            "crafting_table"
        ));
    }

    #[test]
    fn inventory_changed_on_join_reports_items_already_held() {
        setup();
        let fixture = fresh_test_world("criteria_inventory_join");
        let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "Holder", 1).build();
        player
            .inventory
            .lock()
            .set_item(5, ItemStack::new(&vanilla_items::CRAFTING_TABLE));

        player.init_inventory_menu();

        assert!(criterion_done(
            &player,
            "minecraft:story/root",
            "crafting_table"
        ));
    }

    #[test]
    fn items_outside_the_player_inventory_do_not_count() {
        setup();
        let fixture = fresh_test_world("criteria_inventory_chest");
        let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "Looker", 1).build();
        player.init_inventory_menu();

        let chest = SimpleContainer::new(9).into_shared();
        chest
            .lock()
            .set_item(0, ItemStack::new(&vanilla_items::CRAFTING_TABLE));
        player.open_menu("Chest", move |context| {
            let mut builder =
                MenuBuilder::new(&vanilla_menu_types::GENERIC_9X1, context.container_id);
            builder.section(chest, 9);
            builder.player_inventory(&context.player.inventory);
            builder.build(BasicKind {})
        });

        assert!(!criterion_done(
            &player,
            "minecraft:story/root",
            "crafting_table"
        ));

        player
            .inventory
            .lock()
            .set_item(0, ItemStack::new(&vanilla_items::CRAFTING_TABLE));
        player.broadcast_inventory_changes_and_trigger();
        assert!(criterion_done(
            &player,
            "minecraft:story/root",
            "crafting_table"
        ));
    }

    #[test]
    fn tick_trigger_grants_unconditional_criteria() {
        setup();
        let fixture = fresh_test_world("criteria_tick");
        let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "Ticker", 1).build();
        assert!(!criterion_done(
            &player,
            "minecraft:recipes/decorations/crafting_table",
            "unlock_right_away"
        ));

        player.trigger_player_criteria(&TICK);

        assert!(criterion_done(
            &player,
            "minecraft:recipes/decorations/crafting_table",
            "unlock_right_away"
        ));
    }

    #[test]
    fn location_predicate_checks_the_dimension() {
        setup();
        let fixture = fresh_test_world("criteria_location");
        let world = &fixture.world;
        let player = TestPlayerBuilder::new(Arc::clone(world), "Walker", 1).build();
        let pos = player.position();

        let same: LocationPredicate =
            serde_json::from_value(serde_json::json!({ "dimension": world.key.to_string() }))
                .expect("predicate decodes");
        let other_key = if world.key.to_string() == "minecraft:the_end" {
            "minecraft:overworld"
        } else {
            "minecraft:the_end"
        };
        let other: LocationPredicate =
            serde_json::from_value(serde_json::json!({ "dimension": other_key }))
                .expect("predicate decodes");

        assert!(same.matches(world, pos.x, pos.y, pos.z));
        assert!(!other.matches(world, pos.x, pos.y, pos.z));
    }

    fn player_in(world: &Arc<World>, name: &str) -> Arc<Player> {
        let player =
            TestPlayerBuilder::new(Arc::clone(world), name.to_owned(), next_entity_id()).build();
        assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));
        player
    }

    #[test]
    fn eating_and_enchanting_count_their_items() {
        setup();
        let fixture = fresh_test_world("criteria_consume_enchant");
        let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "Eater", 1).build();

        player.trigger_consume_item(&ItemStack::new(&vanilla_items::APPLE));
        player.trigger_enchanted_item(&ItemStack::new(&vanilla_items::DIAMOND_SWORD), 1);
        player.trigger_shot_crossbow(&ItemStack::new(&vanilla_items::CROSSBOW));

        assert!(criterion_done(
            &player,
            "minecraft:husbandry/balanced_diet",
            "apple"
        ));
        assert!(!criterion_done(
            &player,
            "minecraft:husbandry/balanced_diet",
            "beef"
        ));
        assert!(criterion_done(
            &player,
            "minecraft:story/enchant_item",
            "enchanted_item"
        ));
        assert!(criterion_done(
            &player,
            "minecraft:adventure/ol_betsy",
            "shot_crossbow"
        ));
    }

    #[test]
    fn changing_dimension_and_entering_a_gateway_are_reported() {
        setup();
        let fixture = fresh_test_world("criteria_dimension_gateway");
        let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "Traveler", 1).build();

        player.trigger_changed_dimension(
            &Identifier::vanilla_static("the_nether"),
            &Identifier::vanilla_static("overworld"),
        );
        assert!(!criterion_done(
            &player,
            "minecraft:story/enter_the_nether",
            "entered_nether"
        ));
        player.trigger_changed_dimension(
            &Identifier::vanilla_static("overworld"),
            &Identifier::vanilla_static("the_nether"),
        );
        assert!(criterion_done(
            &player,
            "minecraft:story/enter_the_nether",
            "entered_nether"
        ));

        player.trigger_enter_block(vanilla_blocks::STONE.default_state());
        assert!(!criterion_done(
            &player,
            "minecraft:end/enter_end_gateway",
            "entered_end_gateway"
        ));
        player.trigger_enter_block(vanilla_blocks::END_GATEWAY.default_state());
        assert!(criterion_done(
            &player,
            "minecraft:end/enter_end_gateway",
            "entered_end_gateway"
        ));
    }

    #[test]
    fn an_effect_from_an_axolotl_counts_its_source() {
        use crate::entity::LivingEntity as _;
        use crate::entity::MobEffectInstance;
        use crate::entity::entities::AxolotlEntity;
        use steel_registry::{vanilla_entities, vanilla_mob_effects};

        setup();
        let fixture = fresh_test_world("criteria_effects_source");
        let world = &fixture.world;
        let player = player_in(world, "Diver");
        let axolotl = AxolotlEntity::new(
            &vanilla_entities::AXOLOTL,
            next_entity_id(),
            player.position(),
            Arc::downgrade(world),
        );

        player.add_mob_effect(MobEffectInstance::new(vanilla_mob_effects::SPEED, 0));
        assert!(!criterion_done(
            &player,
            "minecraft:husbandry/kill_axolotl_target",
            "kill_axolotl_target"
        ));
        player.add_mob_effect_with_source(
            MobEffectInstance::new(vanilla_mob_effects::REGENERATION, 0),
            Some(&axolotl),
        );
        assert!(criterion_done(
            &player,
            "minecraft:husbandry/kill_axolotl_target",
            "kill_axolotl_target"
        ));
    }

    #[test]
    fn killing_a_mob_credits_the_player_who_hurt_it() {
        use crate::entity::damage::DamageSource;
        use crate::entity::entities::ZombieEntity;
        use crate::entity::{LivingEntity as _, SharedEntity};
        use glam::DVec3;
        use steel_registry::{vanilla_damage_types, vanilla_entities};
        use steel_utils::ChunkPos;

        setup();
        let fixture = fresh_test_world("criteria_kill_credit");
        let world = &fixture.world;
        let position = DVec3::new(8.5, 64.0, 8.5);
        insert_ready_full_chunk(world, ChunkPos::from_entity_pos(position));
        let player = player_in(world, "Hunter");
        let zombie = Arc::new(ZombieEntity::new(
            &vanilla_entities::ZOMBIE,
            next_entity_id(),
            position,
            Arc::downgrade(world),
        ));
        world
            .try_add_entity(Arc::clone(&zombie) as SharedEntity)
            .expect("zombie should attach to the loaded chunk");
        let source = DamageSource::environment(&vanilla_damage_types::PLAYER_ATTACK)
            .with_causing_entity(Arc::clone(&player) as SharedEntity)
            .with_direct_entity(Arc::clone(&player) as SharedEntity);

        assert!(zombie.hurt(world, &source, 100.0));

        assert!(zombie.is_dead_or_dying());
        assert!(criterion_done(
            &player,
            "minecraft:adventure/kill_a_mob",
            "minecraft:zombie"
        ));
    }

    #[test]
    fn totem_beacon_trade_and_loot_triggers_grant_their_advancements() {
        use super::super::USED_TOTEM;

        setup();
        let fixture = fresh_test_world("criteria_interactions");
        let player = player_in(&fixture.world, "Explorer");

        player.trigger_item_criteria(&USED_TOTEM, &ItemStack::new(&vanilla_items::DIRT));
        assert!(!criterion_done(
            &player,
            "minecraft:adventure/totem_of_undying",
            "used_totem"
        ));
        player.trigger_item_criteria(
            &USED_TOTEM,
            &ItemStack::new(&vanilla_items::TOTEM_OF_UNDYING),
        );
        assert!(criterion_done(
            &player,
            "minecraft:adventure/totem_of_undying",
            "used_totem"
        ));

        player.trigger_construct_beacon(1);
        assert!(criterion_done(
            &player,
            "minecraft:nether/create_beacon",
            "beacon"
        ));
        assert!(!criterion_done(
            &player,
            "minecraft:nether/create_full_beacon",
            "beacon"
        ));
        player.trigger_construct_beacon(4);
        assert!(criterion_done(
            &player,
            "minecraft:nether/create_full_beacon",
            "beacon"
        ));

        player.trigger_trade(player.as_ref(), &ItemStack::new(&vanilla_items::EMERALD));
        assert!(criterion_done(
            &player,
            "minecraft:adventure/trade",
            "traded"
        ));

        let desert_well: Identifier = "minecraft:archaeology/desert_well"
            .parse()
            .expect("loot table id parses");
        player.trigger_generate_loot(&desert_well);
        assert!(criterion_done(
            &player,
            "minecraft:adventure/salvage_sherd",
            "desert_well"
        ));
    }

    #[test]
    fn mounting_a_boat_with_a_goat_grants_started_riding() {
        use crate::entity::{
            SharedEntity, create_entity_instance, init_entities, start_riding_entities,
            start_riding_entities_with,
        };
        use glam::DVec3;
        use steel_registry::vanilla_entities;
        use steel_utils::ChunkPos;

        setup();
        init_entities();
        let fixture = fresh_test_world("criteria_started_riding");
        let world = &fixture.world;
        let position = DVec3::new(8.5, 64.0, 8.5);
        insert_ready_full_chunk(world, ChunkPos::from_entity_pos(position));
        let player = player_in(world, "Rider");
        let player_entity: SharedEntity = Arc::clone(&player) as SharedEntity;
        let spawn = |entity_type| {
            let entity = create_entity_instance(world, entity_type, position)
                .expect("the entity can be created");
            world
                .try_add_entity(Arc::clone(&entity))
                .expect("the entity should attach to the loaded chunk");
            entity
        };
        let boat = spawn(&vanilla_entities::OAK_BOAT);
        let goat = spawn(&vanilla_entities::GOAT);
        assert!(start_riding_entities_with(&goat, &boat, true, false));

        // A quiet mount, as after a conversion, grants nothing.
        assert!(start_riding_entities_with(
            &player_entity,
            &boat,
            true,
            false
        ));
        let criterion = "ride_a_boat_with_a_goat";
        let advancement = "minecraft:husbandry/ride_a_boat_with_a_goat";
        assert!(!criterion_done(&player, advancement, criterion));
        player_entity.stop_riding();

        assert!(start_riding_entities(&player_entity, &boat, true));
        assert!(criterion_done(&player, advancement, criterion));
    }

    #[test]
    fn wearing_a_fungus_on_a_stick_on_a_strider_grants_ride_strider() {
        use crate::entity::{
            SharedEntity, create_entity_instance, init_entities, start_riding_entities,
        };
        use glam::DVec3;
        use steel_registry::vanilla_entities;
        use steel_utils::ChunkPos;
        use steel_utils::types::InteractionHand;

        setup();
        init_entities();
        let fixture = fresh_test_world("criteria_ride_strider");
        let world = &fixture.world;
        let position = DVec3::new(8.5, 64.0, 8.5);
        insert_ready_full_chunk(world, ChunkPos::from_entity_pos(position));
        let player = player_in(world, "StriderRider");
        let player_entity: SharedEntity = Arc::clone(&player) as SharedEntity;
        player.inventory.lock().set_item_in_hand(
            InteractionHand::MainHand,
            ItemStack::new(&vanilla_items::WARPED_FUNGUS_ON_A_STICK),
        );
        let criterion = "used_warped_fungus_on_a_stick";
        let advancement = "minecraft:nether/ride_strider";

        // Off a strider the wear does not count.
        player.hurt_item_in_hand(InteractionHand::MainHand, 1);
        assert!(!criterion_done(&player, advancement, criterion));

        let strider = create_entity_instance(world, &vanilla_entities::STRIDER, position)
            .expect("the strider can be created");
        world
            .try_add_entity(Arc::clone(&strider))
            .expect("the strider should attach to the loaded chunk");
        assert!(start_riding_entities(&player_entity, &strider, true));
        player.hurt_item_in_hand(InteractionHand::MainHand, 1);
        assert!(criterion_done(&player, advancement, criterion));
        assert_eq!(
            player
                .inventory
                .lock()
                .get_item_in_hand(InteractionHand::MainHand)
                .get_damage_value(),
            2
        );
    }

    fn movement_fixture(name: &'static str) -> (TestWorld, Arc<Player>) {
        use glam::DVec3;
        use steel_utils::ChunkPos;

        setup();
        init_entities();
        let fixture = fresh_test_world(name);
        insert_ready_full_chunk(
            &fixture.world,
            ChunkPos::from_entity_pos(DVec3::new(8.5, 64.0, 8.5)),
        );
        let player = player_in(&fixture.world, "Mover");
        (fixture, player)
    }

    #[test]
    fn levitating_fifty_blocks_grants_levitate() {
        use glam::DVec3;
        use steel_registry::vanilla_mob_effects;

        use crate::entity::{LivingEntity as _, MobEffectInstance};

        let (_fixture, player) = movement_fixture("criteria_levitate");
        player
            .teleport_to(DVec3::new(8.5, 64.0, 8.5))
            .expect("the player can move");
        assert!(player.add_mob_effect(MobEffectInstance::with_duration(
            vanilla_mob_effects::LEVITATION,
            200,
            0
        )));
        player
            .teleport_to(DVec3::new(8.5, 110.0, 8.5))
            .expect("the player can move");
        player.tick_movement_criteria();
        assert!(!criterion_done(
            &player,
            "minecraft:end/levitate",
            "levitated"
        ));

        player
            .teleport_to(DVec3::new(8.5, 114.5, 8.5))
            .expect("the player can move");
        player.tick_movement_criteria();
        assert!(criterion_done(
            &player,
            "minecraft:end/levitate",
            "levitated"
        ));
    }

    #[test]
    fn falling_from_build_height_to_bedrock_grants_the_fall() {
        use glam::DVec3;

        let (_fixture, player) = movement_fixture("criteria_fall_from_height");
        let advancement = "minecraft:adventure/fall_from_world_height";
        player
            .teleport_to(DVec3::new(8.5, 330.0, 8.5))
            .expect("the player can move");
        player.set_fall_distance(1.0);
        player.tick_movement_criteria();
        player
            .teleport_to(DVec3::new(8.5, -60.0, 8.5))
            .expect("the player can move");
        player.reset_fall_distance();
        assert!(criterion_done(
            &player,
            advancement,
            "fall_from_world_height"
        ));
    }

    #[test]
    fn a_wind_charge_launch_grants_who_needs_rockets() {
        use glam::DVec3;
        use steel_registry::vanilla_entities;

        use crate::entity::create_entity_instance;

        let (fixture, player) = movement_fixture("criteria_who_needs_rockets");
        let advancement = "minecraft:adventure/who_needs_rockets";
        let charge = create_entity_instance(
            &fixture.world,
            &vanilla_entities::WIND_CHARGE,
            DVec3::new(8.5, 64.0, 8.5),
        )
        .expect("the wind charge can be created");
        fixture
            .world
            .try_add_entity(Arc::clone(&charge))
            .expect("the wind charge should attach to the loaded chunk");
        player
            .teleport_to(DVec3::new(8.5, 64.0, 8.5))
            .expect("the player can move");
        player.on_explosion_hit(Some(charge.as_ref()));
        player
            .teleport_to(DVec3::new(8.5, 72.0, 8.5))
            .expect("the player can move");
        player.set_fall_distance(0.5);
        player.tick_movement_criteria();
        assert!(criterion_done(&player, advancement, "who_needs_rockets"));
    }

    #[test]
    fn traveling_through_the_nether_grants_fast_travel() {
        use glam::DVec3;

        let (_fixture, player) = movement_fixture("criteria_fast_travel");
        let overworld = Identifier::vanilla_static("overworld");
        let nether = Identifier::vanilla_static("the_nether");
        player
            .teleport_to(DVec3::new(8.5, 64.0, 8.5))
            .expect("the player can move");
        player.note_dimension_leaving(&overworld, &nether);
        player.trigger_nether_travel(&overworld, &nether);
        player
            .teleport_to(DVec3::new(7020.5, 64.0, 8.5))
            .expect("the player can move");
        player.trigger_nether_travel(&nether, &overworld);
        assert!(criterion_done(
            &player,
            "minecraft:nether/fast_travel",
            "travelled"
        ));
        assert!(player.movement_criteria.lock().entered_nether().is_none());
    }

    fn spawn_in(world: &Arc<World>, entity_type: EntityTypeRef) -> SharedEntity {
        use glam::DVec3;

        use crate::entity::create_entity_instance;

        let entity = create_entity_instance(world, entity_type, DVec3::new(8.5, 64.0, 8.5))
            .expect("the entity can be created");
        world
            .try_add_entity(Arc::clone(&entity))
            .expect("the entity should attach to the loaded chunk");
        entity
    }

    fn thrown_item(world: &Arc<World>, thrower: &dyn Entity, stack: ItemStack) -> ItemEntity {
        use glam::DVec3;
        use steel_registry::vanilla_entities;

        let item = ItemEntity::new(
            &vanilla_entities::ITEM,
            next_entity_id(),
            DVec3::new(8.5, 64.0, 8.5),
            Arc::downgrade(world),
        );
        item.set_item(stack);
        item.set_thrower(thrower.uuid());
        item.set_no_pickup_delay();
        item
    }

    #[test]
    fn picking_up_what_an_allay_threw_grants_its_delivery() {
        use steel_registry::{vanilla_entities, vanilla_items};

        let (fixture, player) = movement_fixture("criteria_allay_delivery");
        let world = &fixture.world;
        let allay = spawn_in(world, &vanilla_entities::ALLAY);
        let item = thrown_item(
            world,
            allay.as_ref(),
            ItemStack::new(&vanilla_items::COOKIE),
        );

        assert!(item.try_pickup(&player));
        assert!(criterion_done(
            &player,
            "minecraft:husbandry/allay_deliver_item_to_player",
            "allay_deliver_item_to_player"
        ));
    }

    #[test]
    fn a_piglin_taking_thrown_gold_grants_distract_piglin() {
        use steel_registry::{vanilla_entities, vanilla_items};

        let (fixture, player) = movement_fixture("criteria_distract_piglin");
        let world = &fixture.world;
        let piglin = spawn_in(world, &vanilla_entities::PIGLIN);
        let item = thrown_item(
            world,
            player.as_ref(),
            ItemStack::new(&vanilla_items::GOLD_INGOT),
        );

        piglin
            .as_mob()
            .expect("a piglin is a mob")
            .pick_up_item(world, &item);
        assert!(criterion_done(
            &player,
            "minecraft:nether/distract_piglin",
            "distract_piglin"
        ));
    }

    #[test]
    fn shearing_wolf_armor_grants_shear_brilliance() {
        use steel_registry::{vanilla_entities, vanilla_items};

        let (fixture, player) = movement_fixture("criteria_wolf_armor");
        let wolf = spawn_in(&fixture.world, &vanilla_entities::WOLF);
        let cat = spawn_in(&fixture.world, &vanilla_entities::CAT);
        let armor = ItemStack::new(&vanilla_items::WOLF_ARMOR);
        let advancement = "minecraft:husbandry/remove_wolf_armor";

        player.trigger_sheared_equipment(&armor, cat.as_ref());
        assert!(!criterion_done(&player, advancement, "remove_wolf_armor"));
        player.trigger_sheared_equipment(&armor, wolf.as_ref());
        assert!(criterion_done(&player, advancement, "remove_wolf_armor"));
    }

    #[test]
    fn sliding_down_honey_grants_sticky_situation() {
        use steel_registry::vanilla_blocks;

        let (_fixture, player) = movement_fixture("criteria_honey_slide");
        let advancement = "minecraft:adventure/honey_block_slide";
        player.trigger_slide_down_block(
            REGISTRY
                .blocks
                .get_default_state_id(&vanilla_blocks::SLIME_BLOCK),
        );
        assert!(!criterion_done(&player, advancement, "honey_block_slide"));
        player.trigger_slide_down_block(
            REGISTRY
                .blocks
                .get_default_state_id(&vanilla_blocks::HONEY_BLOCK),
        );
        assert!(criterion_done(&player, advancement, "honey_block_slide"));
    }

    #[test]
    fn killing_near_a_catalyst_credits_the_player_who_hurt_the_mob() {
        use steel_registry::{vanilla_damage_types, vanilla_entities};

        use crate::block_entity::entities::trigger_kill_near_catalyst;
        use crate::entity::damage::DamageSource;

        let (fixture, player) = movement_fixture("criteria_catalyst");
        let world = &fixture.world;
        let zombie = spawn_in(world, &vanilla_entities::ZOMBIE);
        let zombie = zombie.as_living_entity().expect("a zombie is alive");
        let advancement = "minecraft:adventure/kill_mob_near_sculk_catalyst";

        trigger_kill_near_catalyst(zombie);
        assert!(!criterion_done(
            &player,
            advancement,
            "kill_mob_near_sculk_catalyst"
        ));
        let source = DamageSource::environment(&vanilla_damage_types::PLAYER_ATTACK)
            .with_causing_entity(Arc::clone(&player) as SharedEntity)
            .with_direct_entity(Arc::clone(&player) as SharedEntity);
        assert!(zombie.hurt(world, &source, 1.0));
        trigger_kill_near_catalyst(zombie);
        assert!(criterion_done(
            &player,
            advancement,
            "kill_mob_near_sculk_catalyst"
        ));
    }

    #[test]
    fn curing_and_sneaking_grant_their_advancements() {
        use steel_registry::vanilla_entities;

        let (fixture, player) = movement_fixture("criteria_cure_sneak");
        let zombie = spawn_in(&fixture.world, &vanilla_entities::ZOMBIE_VILLAGER);
        let villager = spawn_in(&fixture.world, &vanilla_entities::VILLAGER);

        player.trigger_cured_zombie_villager(zombie.as_ref(), villager.as_ref());
        assert!(criterion_done(
            &player,
            "minecraft:story/cure_zombie_villager",
            "cured_zombie"
        ));
        player.trigger_player_criteria(&AVOID_VIBRATION);
        assert!(criterion_done(
            &player,
            "minecraft:adventure/avoid_vibration",
            "avoid_vibration"
        ));
    }

    fn bolt_at(world: &Arc<World>, position: glam::DVec3) -> Arc<LightningBoltEntity> {
        use steel_registry::vanilla_entities;

        let bolt = Arc::new(LightningBoltEntity::new(
            &vanilla_entities::LIGHTNING_BOLT,
            next_entity_id(),
            position,
            Arc::downgrade(world),
        ));
        world
            .try_add_entity(Arc::clone(&bolt) as SharedEntity)
            .expect("the bolt should attach to the loaded chunk");
        bolt
    }

    #[test]
    fn a_channeled_bolt_striking_a_villager_grants_very_very_frightening() {
        use glam::DVec3;
        use steel_registry::vanilla_entities;

        let (fixture, player) = movement_fixture("criteria_channeled");
        let world = &fixture.world;
        let _villager = spawn_in(world, &vanilla_entities::VILLAGER);
        let bolt = bolt_at(world, DVec3::new(8.5, 64.0, 8.5));
        let advancement = "minecraft:adventure/very_very_frightening";

        Arc::clone(&bolt).tick();
        assert!(!criterion_done(&player, advancement, "struck_villager"));

        let bolt = bolt_at(world, DVec3::new(8.5, 64.0, 8.5));
        let _villager = spawn_in(world, &vanilla_entities::VILLAGER);
        bolt.set_cause(Some(Arc::clone(&player)));
        Arc::clone(&bolt).tick();
        assert!(criterion_done(&player, advancement, "struck_villager"));
    }

    #[test]
    fn a_fireless_bolt_beside_a_villager_grants_surge_protector() {
        use glam::DVec3;
        use steel_registry::vanilla_entities;

        use crate::entity::create_entity_instance;

        let (fixture, player) = movement_fixture("criteria_surge");
        let world = &fixture.world;
        player
            .teleport_to(DVec3::new(8.5, 100.0, 8.5))
            .expect("the player can move");
        let villager = create_entity_instance(
            world,
            &vanilla_entities::VILLAGER,
            DVec3::new(8.5, 100.0, 1.5),
        )
        .expect("a villager can be created");
        world
            .try_add_entity(Arc::clone(&villager))
            .expect("the villager should attach to the loaded chunk");
        // In the air the bolt has nothing to set on fire.
        let bolt = bolt_at(world, DVec3::new(8.5, 100.0, 14.5));
        let advancement = "minecraft:adventure/lightning_rod_with_villager_no_fire";

        for _ in 0..64 {
            if bolt.is_removed() {
                break;
            }
            Arc::clone(&bolt).tick();
        }
        assert!(bolt.is_removed());
        assert_eq!(bolt.blocks_set_on_fire(), 0);
        assert!(villager.is_alive());
        assert!(criterion_done(
            &player,
            advancement,
            "lightning_rod_with_villager_no_fire"
        ));
    }
}
