//! The profile clients are shown for a player: the login profile, or the one set with
//! [`Player::set_profile_properties`] (a skin changed while playing). Vanilla never changes a
//! profile online; the name and UUID always stay those of the login.

use std::{io, sync::Arc};

use steel_registry::{GameProfile, ProfileProperty};

use super::Player;

impl Player {
    /// The profile clients are shown for this player.
    #[must_use]
    pub fn shown_profile(&self) -> GameProfile {
        self.session
            .shown_profile
            .lock()
            .clone()
            .unwrap_or_else(|| self.gameprofile.clone())
    }

    /// Shows this player with `properties` (`textures` holds the skin) in place of the ones
    /// of the login, to every client at once. Call on the server tick.
    ///
    /// # Errors
    /// The properties exceed the vanilla network limits.
    pub fn set_profile_properties(
        self: &Arc<Self>,
        properties: Vec<ProfileProperty>,
    ) -> io::Result<()> {
        let profile = GameProfile::new(
            self.gameprofile.id(),
            self.gameprofile.name().to_owned(),
            properties,
        )?;
        if self.shown_profile() == profile {
            return Ok(());
        }
        *self.session.shown_profile.lock() = Some(profile);
        self.server().republish_player_profile(self);
        Ok(())
    }
}
