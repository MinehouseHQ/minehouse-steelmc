use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::config::C_POST_EFFECTS;
use steel_registry::packets::play::C_POST_EFFECTS as PLAY_C_POST_EFFECTS;
use steel_utils::Identifier;

/// Vanilla `ClientboundPostEffectsPacket`: the post-processing shaders the
/// client applies, replacing any it applied before.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Config = C_POST_EFFECTS, Play = PLAY_C_POST_EFFECTS)]
pub struct CPostEffects {
    /// The applied post effects, in order.
    #[write(as = Prefixed(VarInt))]
    pub post_effects: Vec<Identifier>,
}
