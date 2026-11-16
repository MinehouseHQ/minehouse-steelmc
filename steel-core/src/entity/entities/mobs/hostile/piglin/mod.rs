//! Piglins and piglin brutes: the gold-loving Nether hunters, with the
//! anger, hunting and zombification logic they share (vanilla
//! `AbstractPiglin` and the shared half of `PiglinAi`).

mod ai;
mod anger;
mod behaviors;
mod brute;
mod piglin_mob;
mod sensor;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicI32, Ordering};

use steel_registry::sound_event::SoundEventRef;
use steel_registry::{vanilla_entities, vanilla_mob_effects};
use steel_utils::types::Difficulty;

pub use anger::anger_nearby_piglins;
pub use brute::PiglinBruteEntity;
pub use piglin_mob::PiglinEntity;

use crate::entity::ai::brain::activity::Activity;
use crate::entity::conversion::ConversionParams;
use crate::entity::{LivingEntity, Mob, MobEffectInstance};

/// Vanilla `AbstractPiglin.CONVERSION_TIME`.
const CONVERSION_TIME: i32 = 300;
/// Ticks of nausea a fresh zombified piglin gets.
const CONVERSION_NAUSEA_TICKS: i32 = 200;

/// Vanilla `AbstractPiglin`: what piglins and piglin brutes share.
pub trait AbstractPiglin: Mob {
    /// Vanilla `canHunt`.
    fn can_hunt(&self) -> bool;

    /// Whether `activity` is running in this piglin's brain.
    fn is_activity_active(&self, activity: Activity) -> bool;

    /// Vanilla `isImmuneToZombification`.
    fn is_immune_to_zombification(&self) -> bool;

    /// Vanilla `setImmuneToZombification`.
    fn set_immune_to_zombification(&self, immune: bool);

    /// Vanilla `timeInOverworld`.
    fn time_in_overworld(&self) -> &AtomicI32;

    /// Vanilla `playConvertedSound`.
    fn converted_sound(&self) -> SoundEventRef;

    /// Vanilla `finishConversion`: turns into a zombified piglin.
    fn finish_conversion(&self);

    /// Vanilla `isAdult`.
    fn is_adult(&self) -> bool {
        !LivingEntity::is_baby(self)
    }

    /// Vanilla `isConverting`: zombifying outside the Nether.
    fn is_converting(&self) -> bool {
        !self.is_immune_to_zombification()
            && !self.is_no_ai()
            && self
                .level()
                .is_some_and(|world| world.dimension_type.piglins_zombify)
    }

    /// The zombification half of vanilla `AbstractPiglin.customServerAiStep`.
    fn tick_zombification(&self) {
        let time = if self.is_converting() {
            self.time_in_overworld().fetch_add(1, Ordering::Relaxed) + 1
        } else {
            self.time_in_overworld().store(0, Ordering::Relaxed);
            0
        };
        if time <= CONVERSION_TIME {
            return;
        }
        if self
            .level()
            .is_some_and(|world| world.difficulty() != Difficulty::Peaceful)
        {
            self.make_sound(Some(self.converted_sound()));
        }
        self.finish_conversion();
    }
}

/// The base vanilla `AbstractPiglin.finishConversion`: a zombified piglin
/// that keeps the equipment and gets nausea.
fn convert_to_zombified_piglin(piglin: &(impl AbstractPiglin + Sized)) {
    let _ = piglin.convert_to(
        &vanilla_entities::ZOMBIFIED_PIGLIN,
        ConversionParams::single(true, true),
        Some(&|zombified: &dyn Mob| {
            zombified.add_mob_effect(MobEffectInstance::with_duration(
                vanilla_mob_effects::NAUSEA,
                CONVERSION_NAUSEA_TICKS,
                0,
            ));
        }),
    );
}
