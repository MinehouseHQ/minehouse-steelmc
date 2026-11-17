//! Vanilla `KineticWeapon.damageEntities` for players: a held spear stabs what
//! it runs into while the player charges with it.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::data_components::components::{KineticWeapon, KineticWeaponCondition};
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_attributes;
use steel_utils::Downcast as _;
use steel_utils::entity_events::EntityStatus;
use steel_utils::types::InteractionHand;

use crate::behavior::ITEM_BEHAVIORS;
use crate::entity::entities::EnderDragonPartEntity;
use crate::entity::{Entity, LivingEntity as _, SharedEntity};
use crate::player::Player;

/// Vanilla `KineticWeapon.getMotion`: the speed, in blocks per second, an
/// entity charges with. A mob passenger moves with its root vehicle.
pub(crate) fn kinetic_motion(entity: &dyn Entity) -> DVec3 {
    if entity.as_player().is_none()
        && entity.is_passenger()
        && let Some(root) = entity.root_vehicle()
    {
        return root.known_speed() * 20.0;
    }
    entity.known_speed() * 20.0
}

/// Vanilla `KineticWeapon.Condition.test`.
fn condition_holds(
    condition: Option<&KineticWeaponCondition>,
    ticks_used: i32,
    attacker_speed: f64,
    relative_speed: f64,
    entity_factor: f64,
) -> bool {
    condition.is_some_and(|condition| {
        ticks_used <= condition.max_duration_ticks()
            && attacker_speed >= f64::from(condition.min_speed()) * entity_factor
            && relative_speed >= f64::from(condition.min_relative_speed()) * entity_factor
    })
}

/// Vanilla `EnderDragonPart.parentMob`: a stab on a dragon part lands on the
/// dragon.
fn stab_target(entity: SharedEntity) -> SharedEntity {
    let parent = entity
        .downcast_ref::<EnderDragonPartEntity>()
        .and_then(|part| {
            entity
                .level()
                .and_then(|world| world.get_entity_by_id(part.parent_id()))
        });
    parent.unwrap_or(entity)
}

impl Player {
    /// Vanilla `KineticWeapon.damageEntities` for the `stack` used in `hand`.
    pub(crate) fn kinetic_damage_entities(
        self: &Arc<Self>,
        kinetic_weapon: &KineticWeapon,
        stack: &ItemStack,
        hand: InteractionHand,
        ticks_remaining: i32,
    ) {
        let use_duration = ITEM_BEHAVIORS
            .get_behavior(stack.item())
            .get_use_duration(stack, self.as_ref());
        let ticks_used = use_duration - ticks_remaining;
        if ticks_used < kinetic_weapon.delay_ticks() {
            return;
        }
        let ticks_used = ticks_used - kinetic_weapon.delay_ticks();
        let look = self.look_angle();
        let attacker_speed = look.dot(kinetic_motion(self.as_ref()));
        // Vanilla `actionFactor`: 1 for players, 0.2 for mobs.
        let action_factor = 1.0;
        let base_damage = self
            .attributes()
            .lock()
            .get_base_value(vanilla_attributes::ATTACK_DAMAGE)
            .unwrap_or_default();
        let world = self.get_world();
        let game_time = world.game_time();
        let mut affected = false;
        for target in self.piercing_hit_entities(stack, &world) {
            let target = stab_target(target);
            if self.living_base.was_recently_stabbed(
                target.id(),
                kinetic_weapon.contact_cooldown_ticks(),
                game_time,
            ) {
                continue;
            }
            self.living_base.remember_stabbed_entity(
                target.id(),
                target.as_living_entity().is_some(),
                game_time,
            );
            let target_speed = look.dot(kinetic_motion(target.as_ref()));
            let relative_speed = (attacker_speed - target_speed).max(0.0);
            let holds = |condition| {
                condition_holds(
                    condition,
                    ticks_used,
                    attacker_speed,
                    relative_speed,
                    action_factor,
                )
            };
            let dismounts = holds(kinetic_weapon.dismount_conditions());
            let knockback = holds(kinetic_weapon.knockback_conditions());
            let damage = holds(kinetic_weapon.damage_conditions());
            if dismounts || knockback || damage {
                let damage_dealt = base_damage as f32
                    + (relative_speed * f64::from(kinetic_weapon.damage_multiplier())).floor()
                        as f32;
                affected |=
                    self.stab_attack(hand, &target, damage_dealt, damage, knockback, dismounts);
            }
        }

        if affected {
            self.broadcast_entity_event(EntityStatus::KineticHit);
            self.trigger_spear_mobs(self.living_base.stabbed_living_entities());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::data_components::components::KineticWeaponCondition;
    use steel_registry::data_components::vanilla_components::KINETIC_WEAPON;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{REGISTRY, init_vanilla_registry, vanilla_entities, vanilla_items};
    use steel_utils::types::InteractionHand;
    use steel_utils::{BlockPos, ChunkPos, Identifier};

    use super::condition_holds;
    use crate::behavior::init_behaviors;
    use crate::entity::{
        Entity as _, LivingEntity, RemovalReason, SharedEntity, create_entity_instance,
        init_entities, next_entity_id,
    };
    use crate::player::advancement::criteria::init_criteria;
    use crate::player::{Player, ResetReason};
    use crate::test_support::{TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk};
    use crate::world::World;

    #[test]
    fn conditions_need_the_speed_within_their_window() {
        let condition = KineticWeaponCondition::new(10, 4.6, 0.0).expect("a valid condition");
        assert!(condition_holds(Some(&condition), 10, 5.0, 0.0, 1.0));
        assert!(!condition_holds(Some(&condition), 11, 5.0, 0.0, 1.0));
        assert!(!condition_holds(Some(&condition), 0, 4.0, 0.0, 1.0));
        assert!(condition_holds(Some(&condition), 0, 1.0, 0.0, 0.2));
        assert!(!condition_holds(None, 0, 100.0, 100.0, 1.0));
    }

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
