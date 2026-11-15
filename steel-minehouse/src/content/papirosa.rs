//! «Магическая папироса»: lit with a flint and steel in the other hand and smoked for
//! eight seconds, with smoke and crackles. Two more hearts and a short slowness; more than
//! three within five minutes make the smoker sick. A port of the Fabric `PapirosaItem`.

use std::{collections::VecDeque, sync::Arc};

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_core::{
    behavior::{ITEM_BEHAVIORS, InteractionResult, ItemBehavior, UseItemContext},
    entity::{LivingEntity, LivingEntityRef, MobEffectInstance},
    world::World,
};
use steel_protocol::packets::game::SoundSource;
use steel_registry::{
    item_stack::ItemStack, particle_type::ParticleData, sound_event::SoundEventRef,
    sound_events as sounds, vanilla_items, vanilla_mob_effects, vanilla_particle_types,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use text_components::{Modifier as _, TextComponent, format::Color};
use uuid::Uuid;

use super::PAPIROSA;

/// How long one papirosa is smoked.
pub const SMOKE_SECONDS: f32 = 8.0;
const SMOKE_TICKS: i32 = 160;
/// A puff of smoke and a crackle this often while smoking.
const PUFF_TICKS: i32 = 25;
/// This many within [`WINDOW_TICKS`] do no harm.
const SAFE_IN_A_ROW: usize = 3;
const WINDOW_TICKS: i64 = 5 * 60 * 20;
const ABSORPTION_TICKS: i32 = 2 * 60 * 20;
const SLOWNESS_TICKS: i32 = 20 * 20;
const SICK_TICKS: i32 = 15 * 20;

/// The game times of each smoker's recent papirosy.
static SMOKED: SyncMutex<Option<FxHashMap<Uuid, VecDeque<i64>>>> = SyncMutex::new(None);

const fn other(hand: InteractionHand) -> InteractionHand {
    match hand {
        InteractionHand::MainHand => InteractionHand::OffHand,
        InteractionHand::OffHand => InteractionHand::MainHand,
    }
}

/// Counts a papirosa smoked at `now` and returns how many that makes within the window.
fn count_smoked(times: &mut VecDeque<i64>, now: i64) -> usize {
    while times
        .front()
        .is_some_and(|first| now - first >= WINDOW_TICKS)
    {
        times.pop_front();
    }
    times.push_back(now);
    times.len()
}

fn eye(entity: &dyn LivingEntity) -> DVec3 {
    let position = entity.position();
    DVec3::new(position.x, entity.get_eye_y(), position.z)
}

fn sound(world: &World, entity: &dyn LivingEntity, sound: SoundEventRef, volume: f32, pitch: f32) {
    world.play_sound_at(
        sound,
        SoundSource::Players,
        eye(entity),
        volume,
        pitch,
        None,
    );
}

/// Smoke just in front of the face.
fn puff(world: &World, entity: &dyn LivingEntity, amount: i32) {
    let at = eye(entity) - DVec3::new(0.0, 0.15, 0.0) + entity.look_angle() * 0.35;
    world.send_particles(
        ParticleData::simple(&vanilla_particle_types::SMOKE),
        at,
        amount * 2,
        DVec3::splat(0.05),
        0.01,
    );
    world.send_particles(
        ParticleData::simple(&vanilla_particle_types::WHITE_SMOKE),
        at + DVec3::new(0.0, 0.1, 0.0),
        amount,
        DVec3::splat(0.08),
        0.005,
    );
}

/// The last drag: the effects, and a big violet-white exhale.
fn smoked(world: &World, entity: &dyn LivingEntity) {
    let count = count_smoked(
        SMOKED
            .lock()
            .get_or_insert_default()
            .entry(entity.uuid())
            .or_default(),
        world.game_time(),
    );
    let mut effects = vec![
        (&vanilla_mob_effects::ABSORPTION, ABSORPTION_TICKS),
        (&vanilla_mob_effects::SLOWNESS, SLOWNESS_TICKS),
    ];
    if count > SAFE_IN_A_ROW {
        effects.push((&vanilla_mob_effects::NAUSEA, SICK_TICKS));
        effects.push((&vanilla_mob_effects::DARKNESS, SICK_TICKS));
    }
    for (effect, ticks) in effects {
        let _ = entity.add_mob_effect(MobEffectInstance::with_duration(effect, ticks, 0));
    }
    puff(world, entity, 6);
    world.send_particles(
        ParticleData::simple(&vanilla_particle_types::WITCH),
        eye(entity),
        6,
        DVec3::new(0.3, 0.2, 0.3),
        0.0,
    );
    sound(world, entity, &sounds::BLOCK_FIRE_EXTINGUISH, 0.3, 1.6);
}

/// How the papirosa behaves.
pub(super) struct Papirosa;

impl ItemBehavior for Papirosa {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        let lighter = other(context.hand);
        let has_lighter = context.inv.with_inventory(|inventory| {
            inventory
                .get_item_in_hand(lighter)
                .is(&vanilla_items::FLINT_AND_STEEL)
        });
        if !has_lighter {
            let text = "Возьми огниво во вторую руку, чтобы прикурить";
            context
                .player
                .send_overlay_message(&TextComponent::plain(text).color(Color::Yellow));
            return InteractionResult::Fail;
        }
        let player: &dyn LivingEntity = context.player.as_ref();
        sound(
            context.world,
            player,
            &sounds::ITEM_FLINTANDSTEEL_USE,
            0.8,
            1.2,
        );
        ITEM_BEHAVIORS.get_behavior(PAPIROSA.base).use_item(context)
    }

    fn on_use_tick(
        &self,
        world: &Arc<World>,
        user: LivingEntityRef<'_>,
        _stack: &mut ItemStack,
        ticks_remaining: i32,
    ) {
        let smoked_for = SMOKE_TICKS - ticks_remaining;
        if smoked_for > 0 && smoked_for % PUFF_TICKS == 0 {
            let entity = user.living();
            puff(world, entity, 2);
            let pitch = 1.4 + rand::random::<f32>() * 0.3;
            sound(world, entity, &sounds::BLOCK_CAMPFIRE_CRACKLE, 0.35, pitch);
        }
    }

    fn finish_using(
        &self,
        stack: &mut ItemStack,
        world: &Arc<World>,
        user: LivingEntityRef<'_>,
    ) -> ItemStack {
        let entity = user.living();
        smoked(world, entity);
        // Lighting it wore the flint and steel a little.
        if let Some(player) = entity.as_player()
            && let Some(hand) = player.active_item_use_hand()
        {
            let infinite = player.has_infinite_materials();
            let mut inventory = player.inventory.lock();
            let lighter = inventory.get_item_in_hand_mut(other(hand));
            if lighter.is(&vanilla_items::FLINT_AND_STEEL) {
                lighter.hurt_and_break(1, infinite);
            }
        }
        ITEM_BEHAVIORS
            .get_behavior(PAPIROSA.base)
            .finish_using(stack, world, user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn papirosy_are_counted_within_five_minutes() {
        let mut times = VecDeque::new();
        assert_eq!(count_smoked(&mut times, 0), 1);
        assert_eq!(count_smoked(&mut times, 200), 2);
        assert_eq!(count_smoked(&mut times, 400), 3);
        assert_eq!(count_smoked(&mut times, 600), 4);
        // The first one is five minutes old now and no longer counts.
        assert_eq!(count_smoked(&mut times, WINDOW_TICKS), 4);
        assert_eq!(count_smoked(&mut times, WINDOW_TICKS * 3), 1);
    }
}
