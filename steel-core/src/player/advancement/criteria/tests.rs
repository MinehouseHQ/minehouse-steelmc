use super::*;
use steel_registry::init_vanilla_registry;

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
    use super::super::{TICK, init_criteria};
    use crate::behavior::init_behaviors;
    use crate::entity::Entity as _;
    use crate::entity::next_entity_id;
    use crate::inventory::container::{Container as _, SimpleContainer};
    use crate::inventory::menu::{MenuBuilder, kinds::BasicKind};
    use crate::player::Player;
    use crate::player::ResetReason;
    use crate::test_support::{TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk};
    use crate::world::World;
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
}
