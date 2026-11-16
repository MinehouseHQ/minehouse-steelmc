//! The explosive archetype: priming, the fuse and the explosion.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::vanilla_game_rules::{MOB_GRIEFING, TNT_EXPLODES};
use steel_registry::{sound_events, vanilla_game_events};

use super::SulfurCubeEntity;
use crate::entity::entities::PrimedTntEntity;
use crate::entity::entities::objects::explosives::use_portal_damage_calculator;
use crate::entity::leash::Leashable as _;
use crate::entity::{Entity, LivingEntity, RemovalReason, SharedEntity};
use crate::world::{ExplosionInteraction, ExplosionOptions, SignalGetter as _};

/// Vanilla `getY(0.0625)`: the explosion center sits a sixteenth up the cube.
const EXPLOSION_HEIGHT_FRACTION: f64 = 0.0625;

impl SulfurCubeEntity {
    /// Vanilla `getFuse`: ticks left, or -1 when not primed.
    #[must_use]
    pub fn fuse(&self) -> i32 {
        self.state.lock().fuse
    }

    /// Vanilla `isPrimed`.
    #[must_use]
    pub fn is_primed(&self) -> bool {
        self.fuse() >= 0
    }

    /// Vanilla `canExplode`: holds an explosive block, alive and not yet lit.
    #[must_use]
    pub fn can_explode(&self) -> bool {
        self.state.lock().explosion.is_some() && LivingEntity::is_alive(self) && !self.is_primed()
    }

    /// Vanilla `primeTime`: lights the fuse, shortened when `imminent`.
    pub fn prime_time(&self, imminent: bool) -> bool {
        let Some(explosion) = self.state.lock().explosion else {
            return false;
        };
        let Some(world) = self.level() else {
            return false;
        };
        if !LivingEntity::is_alive(self) || !world.get_game_rule(&TNT_EXPLODES) || self.is_primed()
        {
            return false;
        }
        let fuse = if imminent {
            PrimedTntEntity::get_random_short_fuse(explosion.fuse)
        } else {
            explosion.fuse
        };
        self.base.set_invulnerable(true);
        self.state.lock().fuse = fuse;
        self.entity_data.lock().max_fuse.set(fuse);
        self.make_sound(Some(&sound_events::ENTITY_TNT_PRIMED));
        self.game_event(&vanilla_game_events::PRIME_FUSE);
        true
    }

    /// Vanilla `primeWhenOnPoweredPosition`.
    pub(super) fn prime_when_on_powered_position(&self) {
        let Some(world) = self.level() else {
            return;
        };
        if self.can_explode() && world.get_best_own_or_neighbour_signal(self.block_position()) != 0
        {
            self.prime_time(false);
        }
    }

    /// Vanilla `tickFuse`: counts down and explodes at zero.
    pub(super) fn tick_fuse(self: &Arc<Self>) {
        let (fuse, explosion) = {
            let mut state = self.state.lock();
            if state.fuse > 0 {
                state.fuse -= 1;
            }
            (state.fuse, state.explosion)
        };
        let Some(explosion) = explosion else {
            return;
        };
        if fuse != 0 {
            return;
        }
        self.drop_leash();
        self.living_base.mark_death_processed();
        if let Some(world) = self.level() {
            if world.get_game_rule(&TNT_EXPLODES) {
                let interaction = if world.get_game_rule(&MOB_GRIEFING) {
                    ExplosionInteraction::Tnt
                } else {
                    ExplosionInteraction::None
                };
                let position = self.position();
                let center = DVec3::new(
                    position.x,
                    position.y
                        + f64::from(self.base.dimensions().height) * EXPLOSION_HEIGHT_FRACTION,
                    position.z,
                );
                let mut options =
                    ExplosionOptions::new(center, explosion.power as f32, interaction);
                options.source = Some(Arc::clone(self) as SharedEntity);
                options.fire = explosion.causes_fire;
                if self.portal_cooldown() > 0 {
                    use_portal_damage_calculator(&mut options);
                }
                world.explode(options);
            }
            self.trigger_on_death_mob_effects(&world, RemovalReason::Killed);
        }
        self.set_removed(RemovalReason::Discarded);
    }
}
