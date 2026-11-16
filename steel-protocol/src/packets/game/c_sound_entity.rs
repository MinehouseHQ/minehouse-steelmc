use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_SOUND_ENTITY;
use steel_registry::sound_event::SoundEventHolder;

use super::SoundSource;

/// Vanilla `ClientboundSoundEntityPacket`: a sound that follows an entity.
#[derive(WriteTo, ClientPacket, Clone, Debug)]
#[packet_id(Play = C_SOUND_ENTITY)]
pub struct CSoundEntity {
    /// The holder-encoded sound event.
    pub sound: SoundEventHolder,
    /// The sound source category.
    pub source: SoundSource,
    /// The entity the sound follows.
    #[write(as = VarInt)]
    pub entity_id: i32,
    /// Volume (1.0 = normal).
    pub volume: f32,
    /// Pitch (1.0 = normal).
    pub pitch: f32,
    /// Random seed for sound variations.
    pub seed: i64,
}
