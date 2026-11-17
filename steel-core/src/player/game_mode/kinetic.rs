//! Vanilla `KineticWeapon.damageEntities` for players: a held spear stabs what
//! it runs into while the player charges with it.

use std::sync::Arc;

use steel_registry::data_components::components::KineticWeapon;
use steel_registry::item_stack::ItemStack;
use steel_utils::types::InteractionHand;

use crate::entity::kinetic::damage_entities;
use crate::player::Player;

impl Player {
    /// Vanilla `KineticWeapon.damageEntities` for the `stack` used in `hand`,
    /// then `SPEAR_MOBS_TRIGGER` when a stab landed.
    pub(crate) fn kinetic_damage_entities(
        self: &Arc<Self>,
        kinetic_weapon: &KineticWeapon,
        stack: &ItemStack,
        hand: InteractionHand,
        ticks_remaining: i32,
    ) {
        let affected = damage_entities(
            self.as_ref(),
            kinetic_weapon,
            stack,
            ticks_remaining,
            |target, stab| {
                self.stab_attack(
                    hand,
                    target,
                    stab.damage,
                    stab.deals_damage,
                    stab.deals_knockback,
                    stab.dismounts,
                )
            },
        );
        if affected {
            self.trigger_spear_mobs(self.living_base.stabbed_living_entities());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::data_components::vanilla_components::KINETIC_WEAPON;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{REGISTRY, init_vanilla_registry, vanilla_entities, vanilla_items};
    use steel_utils::types::InteractionHand;
    use steel_utils::{BlockPos, ChunkPos, Identifier};

    use crate::behavior::init_behaviors;
    use crate::entity::{
        Entity as _, LivingEntity, RemovalReason, SharedEntity, create_entity_instance,
        init_entities, next_entity_id,
    };
    use crate::player::advancement::criteria::init_criteria;
    use crate::player::{Player, ResetReason};
    use crate::test_support::{TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk};
    use crate::world::World;

    fn zombie_at(world: &Arc<World>, position: DVec3) -> SharedEntity {
        let zombie = create_entity_instance(world, &vanilla_entities::ZOMBIE, position)
            .expect("zombie factory");
        world
            .try_add_entity(Arc::clone(&zombie))
            .expect("zombie added");
        zombie
    }

    fn health(entity: &SharedEntity) -> f32 {
        entity
            .as_living_entity()
            .map_or(0.0, LivingEntity::get_health)
    }

    fn spear_many_mobs_done(player: &Player) -> bool {
        let key = Identifier::vanilla_static("adventure/spear_many_mobs");
        let advancement = REGISTRY
            .advancements
            .value_by_key(&key)
            .expect("vanilla advancement exists");
        player
            .advancements
            .lock()
            .progress
            .get_progress(advancement)
            .is_some_and(|progress| progress.is_criterion_done("spear_many_mobs"))
    }

    #[test]
    fn a_spear_charge_stabs_each_mob_once_and_five_grant_the_advancement() {
        init_vanilla_registry();
        init_behaviors();
        init_entities();
        init_criteria();
        let fixture = fresh_test_world("kinetic_spear_charge");
        let world = &fixture.world;
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(BlockPos::new(8, 64, 8)));
        let spear = ItemStack::new(&vanilla_items::IRON_SPEAR);
        let kinetic_weapon = spear
            .get(KINETIC_WEAPON)
            .cloned()
            .expect("spears are kinetic weapons");
        let player =
            TestPlayerBuilder::new(Arc::clone(world), "Lancer".to_owned(), next_entity_id())
                .build();
        assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));
        player
            .teleport_to(DVec3::new(8.5, 64.0, 8.5))
            .expect("player teleport");
        player.inventory.lock().set_selected_item(spear.clone());
        player.start_using_item(InteractionHand::MainHand);
        // Charging north to south at a block per tick, looking south.
        player
            .movement
            .lock()
            .set_last_known_client_movement(DVec3::new(0.0, 0.0, 1.0));
        let ticks_remaining = 72000 - kinetic_weapon.delay_ticks();

        for stabbed in 1..=5 {
            let zombie = zombie_at(world, DVec3::new(8.5, 64.0, 11.5));
            let full_health = health(&zombie);
            player.kinetic_damage_entities(
                &kinetic_weapon,
                &spear,
                InteractionHand::MainHand,
                ticks_remaining,
            );
            assert!(health(&zombie) < full_health, "zombie {stabbed} is stabbed");
            let after_stab = health(&zombie);
            player.kinetic_damage_entities(
                &kinetic_weapon,
                &spear,
                InteractionHand::MainHand,
                ticks_remaining,
            );
            assert!(
                (health(&zombie) - after_stab).abs() < f32::EPSILON,
                "the contact cooldown spares a mob just stabbed"
            );
            assert_eq!(spear_many_mobs_done(&player), stabbed == 5);
            zombie.set_removed(RemovalReason::Discarded);
        }
    }
}
