//! Vanilla `ServerPlayer.requestDebugSubscriptions` / `debugSubscriptions`.

use super::Player;

impl Player {
    /// Vanilla `ServerPlayer.requestDebugSubscriptions`: replaces the requested set.
    pub fn request_debug_subscriptions(&self, subscriptions: Vec<i32>) {
        *self.requested_debug_subscriptions.lock() = subscriptions;
    }

    /// Vanilla `ServerPlayer.debugSubscriptions`: the requested debug data, which
    /// only operators receive.
    #[must_use]
    pub fn debug_subscriptions(&self) -> Vec<i32> {
        if !self.is_operator() {
            return Vec::new();
        }
        self.requested_debug_subscriptions.lock().clone()
    }

    /// Vanilla `ServerPlayer.restoreFrom` keeps the requested debug subscriptions.
    pub(crate) fn restore_debug_subscriptions_from(&self, old_player: &Self) {
        let requested = old_player.requested_debug_subscriptions.lock().clone();
        *self.requested_debug_subscriptions.lock() = requested;
    }
}
