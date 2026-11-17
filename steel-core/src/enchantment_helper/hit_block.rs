//! Vanilla `minecraft:hit_block` enchantment effects (Channeling's lightning on
//! a lightning rod), the `summon_entity` effect, and the `weather_check` and
//! `location_check` conditions they need.

use glam::{DVec3, IVec3};
use rand::seq::IndexedRandom as _;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::enchantment_effect::{EnchantmentBlockSet, EnchantmentEntityTypeSet};
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::{REGISTRY, RegistryExt as _, TaggedRegistryExt as _};
use steel_utils::{BlockPos, Downcast as _};

use super::item_in_use::EnchantedItemInUse;
use super::{apply_entity_effect, entity_requirements_state_at};
use crate::entity::entities::LightningBoltEntity;
use crate::entity::{
    Entity, EntitySpawnPlacement, EntitySpawnReason, EntitySpawnRequest, LivingEntity,
    SharedEntity, spawn_entity,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::world::World;

/// Vanilla `EnchantmentHelper.onHitBlock` for a weapon held by no slot: runs
/// every enchantment's `minecraft:hit_block` effects at `hit_location`.
/// Returns whether an effect broke the weapon.
pub(crate) fn on_hit_block(
    world: &World,
    weapon: &mut ItemStack,
    owner: Option<&SharedEntity>,
    entity: &dyn Entity,
    hit_location: DVec3,
) -> bool {
    let Some(enchantments) = weapon.get_enchantments().cloned() else {
        return false;
    };
    let mut broke = false;
    for (key, level) in enchantments.iter() {
        if *level == 0 {
            continue;
        }
        let Some(enchantment) = REGISTRY.enchantments.by_key(key) else {
            continue;
        };
        let level = *level as i32;
        for effect in enchantment.effects.hit_block {
            let matches = effect.requirements.is_none_or(|requirements| {
                entity_requirements_state_at(requirements, entity, hit_location, level, None)
                    == Some(true)
            });
            if matches {
                broke |= apply_entity_effect(
                    world,
                    &effect.effect,
                    level,
                    entity,
                    hit_location,
                    owner,
                    &mut EnchantedItemInUse::Stack(weapon),
                );
            }
        }
    }
    broke
}

/// Vanilla `EnchantmentHelper.onHitBlock` for the item `owner` holds in
/// `slot`: runs every enchantment's `minecraft:hit_block` effects at
/// `hit_location`, a broken item going through the owner's equipment break.
pub(crate) fn on_hit_block_equipped(
    world: &World,
    owner: &dyn LivingEntity,
    slot: EquipmentSlot,
    hit_location: DVec3,
) {
    let mut enchantments = None;
    owner.with_equipment_slot(slot, &mut |item| {
        enchantments = item.get_enchantments().cloned();
    });
    let Some(enchantments) = enchantments else {
        return;
    };
    let owner_shared = world.get_entity_by_id(owner.id());
    for (key, level) in enchantments.iter() {
        if *level == 0 {
            continue;
        }
        let Some(enchantment) = REGISTRY.enchantments.by_key(key) else {
            continue;
        };
        let level = *level as i32;
        for effect in enchantment.effects.hit_block {
            let matches = effect.requirements.is_none_or(|requirements| {
                entity_requirements_state_at(requirements, owner, hit_location, level, None)
                    == Some(true)
            });
            if matches {
                apply_entity_effect(
                    world,
                    &effect.effect,
                    level,
                    owner,
                    hit_location,
                    owner_shared.as_ref(),
                    &mut EnchantedItemInUse::Equipped { owner, slot },
                );
            }
        }
    }
}

/// Vanilla `WeatherCheck.test`.
pub(super) fn weather_matches(
    world: &World,
    raining: Option<bool>,
    thundering: Option<bool>,
) -> bool {
    raining.is_none_or(|raining| raining == world.is_raining())
        && thundering.is_none_or(|thundering| thundering == world.is_thundering())
}

/// Vanilla `LocationCheck.test` with a `LocationPredicate` holding only
/// `block.blocks` and `can_see_sky`.
pub(super) fn location_check_matches(
    world: &World,
    origin: DVec3,
    offset: IVec3,
    blocks: Option<&EnchantmentBlockSet>,
    can_see_sky: Option<bool>,
) -> bool {
    let pos = BlockPos::containing(
        origin.x + f64::from(offset.x),
        origin.y + f64::from(offset.y),
        origin.z + f64::from(offset.z),
    );
    let state = world.loaded_block_state(pos);
    blocks.is_none_or(|blocks| state.is_some_and(|state| blocks.contains(state.get_block())))
        && can_see_sky.is_none_or(|expected| sky_visible(world, pos) == expected)
}

/// Vanilla `LocationPredicate`'s `can_see_sky`: a loaded position under the
/// open sky.
pub(super) fn sky_visible(world: &World, pos: BlockPos) -> bool {
    world.loaded_block_state(pos).is_some() && world.can_see_sky(pos)
}

/// Vanilla `SummonEntityEffect.apply`: spawns one of the entity types at
/// `position`; a lightning bolt remembers the player who owns the item.
/// Steel has no scoreboard teams, so `join_team` has no team to join.
pub(super) fn summon_entity(
    entity_types: &EnchantmentEntityTypeSet,
    entity: &dyn Entity,
    position: DVec3,
    owner: Option<&SharedEntity>,
) {
    let Some(world) = entity.level() else {
        return;
    };
    let pos = BlockPos::containing(position.x, position.y, position.z);
    if !World::is_in_spawnable_bounds(pos) {
        return;
    }
    let Some(entity_type) = random_entity_type(entity_types) else {
        return;
    };
    let request = EntitySpawnRequest {
        entity_type,
        placement: EntitySpawnPlacement::Block {
            pos,
            try_move_down: false,
            moved_up: false,
        },
        reason: EntitySpawnReason::Triggered,
        finalize_spawn: true,
        play_ambient_sound: true,
        item_stack: None,
        user_is_operator: false,
    };
    let Ok(spawned) = spawn_entity(&world, request) else {
        return;
    };
    if let Some(bolt) = spawned.downcast_ref::<LightningBoltEntity>()
        && let Some(owner) = owner
        && owner.as_player().is_some()
    {
        bolt.set_cause(world.players.get_by_entity_id(owner.id()));
    }
    let (yaw, pitch) = spawned.rotation();
    spawned.snap_to(position, yaw, pitch);
}

/// Vanilla `HolderSet.getRandomElement`.
fn random_entity_type(entity_types: &EnchantmentEntityTypeSet) -> Option<EntityTypeRef> {
    match entity_types {
        EnchantmentEntityTypeSet::Direct(keys) => keys
            .choose(&mut rand::rng())
            .and_then(|key| REGISTRY.entity_types.by_key(key)),
        EnchantmentEntityTypeSet::Tag(tag) => {
            let types: Vec<EntityTypeRef> = REGISTRY.entity_types.iter_tag(tag).collect();
            // Vanilla picks uniformly from the tag's entries.
            types.choose(&mut rand::rng()).copied()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::enchantment_effect::EnchantmentEntityEffect;
    use steel_registry::{
        init_vanilla_registry, vanilla_blocks, vanilla_enchantments, vanilla_entities,
        vanilla_items,
    };
    use steel_utils::types::UpdateFlags;
    use steel_utils::{BlockPos, ChunkPos, Downcast as _, Identifier, WorldAabb};

    use super::{on_hit_block, on_hit_block_equipped};
    use crate::behavior::init_behaviors;
    use crate::entity::entities::LightningBoltEntity;
    use crate::entity::{SharedEntity, create_entity_instance, init_entities, next_entity_id};
    use crate::inventory::equipment::{EntityEquipment as _, EquipmentSlot};
    use crate::player::ResetReason;
    use crate::test_support::{
        TestPlayerBuilder, enchanted_item, fresh_test_world, insert_ready_full_chunk,
    };
    use crate::world::World;

    const ROD: BlockPos = BlockPos::new(8, 65, 8);

    #[test]
    fn channeling_decodes_its_lightning_effects() {
        init_vanilla_registry();
        let effects = &vanilla_enchantments::CHANNELING.effects;
        assert_eq!(effects.hit_block.len(), 1);
        assert!(effects.hit_block[0].requirements.is_some());
        for effect in [&effects.hit_block[0].effect, &effects.post_attack[0].effect] {
            let EnchantmentEntityEffect::AllOf(parts) = effect else {
                panic!("channeling runs several effects");
            };
            assert!(matches!(
                parts[0],
                EnchantmentEntityEffect::SummonEntity { .. }
            ));
        }
    }

    fn bolts(world: &Arc<World>) -> Vec<SharedEntity> {
        let bounds = WorldAabb::from_min_max(DVec3::splat(0.0), DVec3::new(16.0, 128.0, 16.0));
        world.get_entities_in_aabb_matching(&bounds, |entity| {
            entity.entity_type() == &vanilla_entities::LIGHTNING_BOLT
        })
    }

    #[test]
    fn a_channeling_trident_on_a_rod_in_a_storm_calls_credited_lightning() {
        init_vanilla_registry();
        init_behaviors();
        init_entities();
        let fixture = fresh_test_world("channeling_rod");
        let world = &fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        assert!(world.set_block(
            ROD,
            vanilla_blocks::LIGHTNING_ROD.default_state(),
            UpdateFlags::UPDATE_ALL,
        ));
        world.chunk_map.broadcast_changed_chunks();
        let player =
            TestPlayerBuilder::new(Arc::clone(world), "Thrower".to_owned(), next_entity_id())
                .build();
        assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));
        let owner: SharedEntity = Arc::clone(&player) as SharedEntity;
        let trident = create_entity_instance(
            world,
            &vanilla_entities::TRIDENT,
            DVec3::new(8.5, 66.0, 8.5),
        )
        .expect("a trident can be created");
        let mut weapon = enchanted_item(
            &vanilla_items::TRIDENT,
            Identifier::vanilla_static("channeling"),
            1,
        );
        let hit = DVec3::new(8.5, 65.9, 8.5);

        assert!(!on_hit_block(
            world,
            &mut weapon,
            Some(&owner),
            trident.as_ref(),
            hit
        ));
        assert!(bolts(world).is_empty(), "clear skies call no lightning");

        {
            let mut weather = world.weather.write();
            weather.rain_level = 1.0;
            weather.thunder_level = 1.0;
        }
        let _ = on_hit_block(world, &mut weapon, Some(&owner), trident.as_ref(), hit);
        let bolts = bolts(world);
        assert_eq!(bolts.len(), 1);
        let bolt = bolts[0]
            .downcast_ref::<LightningBoltEntity>()
            .expect("the summoned entity is a bolt");
        assert_eq!(bolts[0].position(), hit);
        assert!(
            bolt.cause()
                .is_some_and(|cause| Arc::ptr_eq(&cause, &player))
        );
    }

    #[test]
    fn a_player_punching_a_rod_with_a_channeling_trident_calls_no_lightning() {
        init_vanilla_registry();
        init_behaviors();
        init_entities();
        let fixture = fresh_test_world("channeling_punch");
        let world = &fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        assert!(world.set_block(
            ROD,
            vanilla_blocks::LIGHTNING_ROD.default_state(),
            UpdateFlags::UPDATE_ALL,
        ));
        let player =
            TestPlayerBuilder::new(Arc::clone(world), "Puncher".to_owned(), next_entity_id())
                .build();
        assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));
        player.inventory.lock().set(
            EquipmentSlot::MainHand,
            enchanted_item(
                &vanilla_items::TRIDENT,
                Identifier::vanilla_static("channeling"),
                1,
            ),
        );
        {
            let mut weather = world.weather.write();
            weather.rain_level = 1.0;
            weather.thunder_level = 1.0;
        }

        on_hit_block_equipped(
            world,
            player.as_ref(),
            EquipmentSlot::MainHand,
            DVec3::new(8.5, 65.5, 8.5),
        );

        assert!(
            bolts(world).is_empty(),
            "channeling's hit_block needs the trident entity itself"
        );
    }
}
