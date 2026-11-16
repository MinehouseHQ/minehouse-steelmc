//! Vanilla `DragonFireball`: the Ender Dragon's fireball, which leaves a
//! lingering cloud of dragon's breath where it lands.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::particle_type::{ParticleData, PowerParticleOption};
use steel_registry::vanilla_entity_data::DragonFireballEntityData;
use steel_registry::{
    MobEffectInstance, level_events, vanilla_entities, vanilla_mob_effects, vanilla_particle_types,
};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, DowncastType, DowncastTypeKey};

use crate::entity::damage::DamageSource;
use crate::entity::entities::AreaEffectCloudEntity;
use crate::entity::{
    AbstractHurtingProjectile, Entity, EntityBase, EntityBaseLoad, EntitySyncedData,
    HurtingProjectileBase, Projectile, ProjectileBase, ProjectileHit, RemovalReason, SharedEntity,
    next_entity_id,
};
use crate::world::World;

/// Vanilla `SPLASH_RANGE`: the cloud moves onto a living entity this close.
const SPLASH_RANGE: f64 = 4.0;
const CLOUD_RADIUS: f32 = 3.0;
/// The radius the cloud grows to over its duration.
const CLOUD_FINAL_RADIUS: f32 = 7.0;
const CLOUD_DURATION: i32 = 600;
const CLOUD_POTION_DURATION_SCALE: f32 = 0.25;

/// A dragon fireball.
#[entity_behavior(class = "DragonFireball")]
pub struct DragonFireballEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<DragonFireballEntityData>,
    projectile_base: ProjectileBase,
    hurting_base: HurtingProjectileBase,
}

// SAFETY: This key is owned by Steel and uniquely identifies `DragonFireballEntity`.
unsafe impl DowncastType for DragonFireballEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/dragon_fireball");
}

impl DragonFireballEntity {
    /// Creates a dragon fireball at rest.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved dragon fireball.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        Self {
            base,
            entity_type,
            entity_data: SyncMutex::new(DragonFireballEntityData::new()),
            projectile_base: ProjectileBase::new(),
            hurting_base: HurtingProjectileBase::new(),
        }
    }

    /// The cloud half of vanilla `DragonFireball.onHit`.
    fn spawn_breath_cloud(&self, world: &Arc<World>) {
        let search = self
            .bounding_box()
            .inflate_xyz(SPLASH_RANGE, 2.0, SPLASH_RANGE);
        let mut position = self.position();
        // Vanilla moves the cloud onto the first living entity in range.
        if let Some(entity) = world
            .get_entities_in_aabb(&search)
            .into_iter()
            .filter(|entity| entity.as_living_entity().is_some())
            .find(|entity| self.distance_to_sqr(entity.position()) < SPLASH_RANGE * SPLASH_RANGE)
        {
            position = entity.position();
        }

        let cloud = Arc::new(AreaEffectCloudEntity::new(
            &vanilla_entities::AREA_EFFECT_CLOUD,
            next_entity_id(),
            position,
            Arc::downgrade(world),
        ));
        cloud.set_owner(self.get_owner().as_ref());
        cloud.set_custom_particle(ParticleData::new(
            &vanilla_particle_types::DRAGON_BREATH,
            PowerParticleOption::new(1.0),
        ));
        cloud.set_radius(CLOUD_RADIUS);
        cloud.set_duration(CLOUD_DURATION);
        #[expect(
            clippy::cast_precision_loss,
            reason = "vanilla divides by the duration as a float"
        )]
        cloud.set_radius_per_tick((CLOUD_FINAL_RADIUS - cloud.radius()) / cloud.duration() as f32);
        cloud.set_potion_duration_scale(CLOUD_POTION_DURATION_SCALE);
        cloud.add_effect(MobEffectInstance::new(
            vanilla_mob_effects::INSTANT_DAMAGE,
            1,
            1,
            false,
            true,
            true,
            None,
        ));

        world.level_event(
            level_events::PARTICLES_DRAGON_FIREBALL_SPLASH,
            BlockPos::from(self.position()),
            if self.is_silent() { -1 } else { 1 },
            None,
        );
        let cloud: SharedEntity = cloud;
        if let Err(error) = world.try_add_entity(cloud) {
            log::debug!("world rejected a dragon breath cloud: {error}");
        }
    }
}

impl Entity for DragonFireballEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        self.hurting_projectile_tick();
    }

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    fn is_pickable(&self) -> bool {
        self.projectile_is_pickable()
    }

    fn play_entity_on_fire_extinguished_sound(&self) {}

    fn spawn_data(&self) -> i32 {
        self.get_owner().map_or(0, |owner| owner.id())
    }

    fn restore_owner_reference(&self, owner: &SharedEntity) {
        self.cache_owner_entity(owner);
    }

    fn projectile_owner_uuid(&self) -> Option<uuid::Uuid> {
        self.owner_uuid()
    }

    fn projectile_owner(&self) -> Option<SharedEntity> {
        self.get_owner()
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_projectile(nbt);
        self.save_hurting_projectile(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_projectile(nbt);
        self.load_hurting_projectile(nbt);
    }
}

impl Projectile for DragonFireballEntity {
    fn projectile_base(&self) -> &ProjectileBase {
        &self.projectile_base
    }

    fn can_hit_entity(&self, entity: &dyn Entity) -> bool {
        self.hurting_can_hit_entity(entity)
    }

    fn on_deflection(&self, by_attack: bool) {
        self.on_hurting_deflection(by_attack);
    }

    fn hurt(&self, _world: &World, _source: &DamageSource, _amount: f32) -> bool {
        false
    }

    /// Vanilla `DragonFireball.onHit`: anything but its own dragon pops it
    /// into a cloud of dragon's breath.
    fn on_hit(self: Arc<Self>, hit: &ProjectileHit) {
        Arc::clone(&self).projectile_on_hit(hit);
        if let ProjectileHit::Entity(entity_hit) = hit
            && self.owned_by(entity_hit.entity.as_ref())
        {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };
        self.spawn_breath_cloud(&world);
        self.set_removed(RemovalReason::Discarded);
    }
}

impl AbstractHurtingProjectile for DragonFireballEntity {
    fn hurting_projectile_base(&self) -> &HurtingProjectileBase {
        &self.hurting_base
    }

    fn should_burn(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::blocks::block_state_ext::BlockStateExt as _;
    use steel_registry::{init_vanilla_registry, vanilla_blocks};
    use steel_utils::types::UpdateFlags;
    use steel_utils::{ChunkPos, Downcast as _, WorldAabb};

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::entity::init_entities;
    use crate::test_support::{fresh_test_world, insert_ready_full_chunk};

    #[test]
    fn a_landed_fireball_leaves_a_growing_breath_cloud() {
        init_vanilla_registry();
        init_behaviors();
        init_entities();
        let world_fixture = fresh_test_world("dragon_fireball_cloud");
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let stone = vanilla_blocks::STONE.default_state();
        for x in 0..16 {
            for z in 0..16 {
                world.set_block(BlockPos::new(x, 64, z), stone, UpdateFlags::UPDATE_ALL);
            }
        }
        let fireball = Arc::new(DragonFireballEntity::new(
            &vanilla_entities::DRAGON_FIREBALL,
            next_entity_id(),
            DVec3::new(8.5, 68.0, 8.5),
            Arc::downgrade(world),
        ));
        let entity: SharedEntity = Arc::<DragonFireballEntity>::clone(&fireball);
        world
            .try_add_entity(Arc::clone(&entity))
            .expect("the fireball attaches to the loaded chunk");
        fireball.assign_directional_movement(DVec3::new(0.0, -1.0, 0.0));

        for _ in 0..100 {
            if fireball.is_removed() {
                break;
            }
            Arc::clone(&entity).tick();
        }
        assert!(fireball.is_removed(), "the fireball pops on the floor");
        assert!(!world.get_block_state(BlockPos::new(8, 64, 8)).is_air());
        let area = WorldAabb::new(0.0, 60.0, 0.0, 16.0, 80.0, 16.0);
        let clouds: Vec<_> = world
            .get_entities_in_aabb(&area)
            .into_iter()
            .filter(|entity| entity.entity_type().key == vanilla_entities::AREA_EFFECT_CLOUD.key)
            .collect();
        assert_eq!(clouds.len(), 1);
        let cloud = clouds[0]
            .downcast_ref::<AreaEffectCloudEntity>()
            .expect("an area effect cloud");
        assert!((cloud.radius() - CLOUD_RADIUS).abs() < f32::EPSILON);
        assert_eq!(cloud.duration(), CLOUD_DURATION);
    }
}
