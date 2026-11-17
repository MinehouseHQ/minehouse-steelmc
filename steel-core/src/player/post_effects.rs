//! Vanilla `ServerPlayer.postEffects`: the post-processing shaders a player's
//! client applies, set by `/posteffect`.

use steel_protocol::packets::common::CPostEffects;
use steel_utils::Identifier;

use super::Player;

/// The applied post effects and whether the client still needs them.
#[derive(Debug, Clone)]
pub(crate) struct PostEffects {
    /// Vanilla `postEffects`, in the order they were added.
    effects: Vec<Identifier>,
    /// Vanilla `postEffectsDirty`; starts set so a new player sends its list.
    dirty: bool,
}

impl Default for PostEffects {
    fn default() -> Self {
        Self {
            effects: Vec::new(),
            dirty: true,
        }
    }
}

impl PostEffects {
    /// Vanilla `addPostEffect`: false when the effect is already applied.
    pub(crate) fn add(&mut self, effect: Identifier) -> bool {
        if self.effects.contains(&effect) {
            return false;
        }
        self.effects.push(effect);
        self.dirty = true;
        true
    }

    /// Vanilla `removePostEffect`: false when the effect was not applied.
    pub(crate) fn remove(&mut self, effect: &Identifier) -> bool {
        let Some(index) = self.effects.iter().position(|applied| applied == effect) else {
            return false;
        };
        self.effects.remove(index);
        self.dirty = true;
        true
    }

    /// Vanilla `clearPostEffects`: false when nothing was applied.
    pub(crate) fn clear(&mut self) -> bool {
        if self.effects.is_empty() {
            return false;
        }
        self.effects.clear();
        self.dirty = true;
        true
    }

    /// Vanilla `getPostEffects`.
    pub(crate) fn effects(&self) -> &[Identifier] {
        &self.effects
    }

    /// Vanilla `sendPostEffects`: clears the dirty flag and returns the packet.
    fn take_packet(&mut self) -> CPostEffects {
        self.dirty = false;
        CPostEffects {
            post_effects: self.effects.clone(),
        }
    }
}

impl Player {
    /// Vanilla `ServerPlayer.addPostEffect`.
    pub fn add_post_effect(&self, effect: Identifier) -> bool {
        self.post_effects.lock().add(effect)
    }

    /// Vanilla `ServerPlayer.removePostEffect`.
    pub fn remove_post_effect(&self, effect: &Identifier) -> bool {
        self.post_effects.lock().remove(effect)
    }

    /// Vanilla `ServerPlayer.clearPostEffects`.
    pub fn clear_post_effects(&self) -> bool {
        self.post_effects.lock().clear()
    }

    /// Vanilla `ServerPlayer.getPostEffects`, as a copy.
    #[must_use]
    pub fn post_effects(&self) -> Vec<Identifier> {
        self.post_effects.lock().effects().to_vec()
    }

    /// Replaces the applied post effects with saved ones, keeping vanilla's
    /// add order and duplicate rule.
    pub(crate) fn restore_post_effects(&self, effects: impl IntoIterator<Item = Identifier>) {
        let mut post_effects = self.post_effects.lock();
        post_effects.clear();
        for effect in effects {
            post_effects.add(effect);
        }
    }

    /// Vanilla `ServerPlayer.sendPostEffects`.
    pub(crate) fn send_post_effects(&self) {
        let packet = self.post_effects.lock().take_packet();
        self.send_packet(packet);
    }

    /// The end of vanilla `ServerPlayer.tick`: resends changed post effects.
    pub(crate) fn send_dirty_post_effects(&self) {
        let packet = {
            let mut post_effects = self.post_effects.lock();
            if !post_effects.dirty {
                return;
            }
            post_effects.take_packet()
        };
        self.send_packet(packet);
    }
}

#[cfg(test)]
mod tests {
    use steel_utils::Identifier;

    use super::PostEffects;

    #[test]
    fn post_effects_follow_vanilla_add_remove_and_clear_rules() {
        let creeper = Identifier::vanilla_static("creeper");
        let spider = Identifier::vanilla_static("spider");
        let mut effects = PostEffects::default();
        assert!(effects.dirty);
        let _ = effects.take_packet();
        assert!(!effects.dirty);

        assert!(!effects.clear());
        assert!(!effects.remove(&creeper));
        assert!(!effects.dirty);

        assert!(effects.add(creeper.clone()));
        assert!(effects.add(spider.clone()));
        assert!(!effects.add(creeper.clone()));
        assert_eq!(effects.effects(), [creeper.clone(), spider.clone()]);
        assert!(effects.dirty);

        let packet = effects.take_packet();
        assert_eq!(packet.post_effects, vec![creeper.clone(), spider.clone()]);
        assert!(!effects.dirty);

        assert!(effects.remove(&creeper));
        assert_eq!(effects.effects(), [spider]);
        assert!(effects.clear());
        assert_eq!(effects.effects(), []);
        assert!(effects.dirty);
    }
}
