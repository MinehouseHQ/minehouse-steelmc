//! The banker's skin (the Fabric `BankerSkin`): a signed `textures` property or the nick of a
//! player.
//!
//! Vanilla clients only show skins from textures.minecraft.net with Mojang's signature, so a
//! skin from any other link goes through `MineSkin` (api.mineskin.org, no key needed), which
//! uploads it and returns the signed property.

use std::sync::LazyLock;

use crate::skins::providers;

use serde::{Deserialize, Serialize};
use steel_registry::{
    game_profile::ProfileProperty,
    resolvable_profile::{PartialProfile, PlayerSkinPatch, ResolvableProfile},
};

/// The skin Timur picked for the banker.
const DEFAULT: &str = include_str!("skin.json");

/// What the banker looks like.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Skin {
    /// A signed `textures` property.
    Texture {
        /// The property itself.
        value: String,
        /// Mojang's signature of it.
        signature: String,
    },
    /// The skin of the player with this nick; the client finds it itself.
    Name {
        /// The nick.
        name: String,
    },
}

static DEFAULT_SKIN: LazyLock<Skin> = LazyLock::new(|| {
    serde_json::from_str(DEFAULT).unwrap_or_else(|error| {
        log::error!("Bank: the banker's skin can't be read: {error}");
        Skin::Name {
            name: "Steve".to_owned(),
        }
    })
});

impl Skin {
    /// The banker's own skin.
    #[must_use]
    pub fn default_skin() -> Self {
        DEFAULT_SKIN.clone()
    }

    /// The profile of the mannequin.
    #[must_use]
    pub fn profile(&self) -> ResolvableProfile {
        let patch = PlayerSkinPatch::default();
        let profile = match self {
            Self::Texture { value, signature } => ProfileProperty::new(
                "textures".to_owned(),
                value.clone(),
                Some(signature.clone()),
            )
            .and_then(|property| PartialProfile::new(None, None, vec![property]))
            .map(|profile| ResolvableProfile::static_partial(profile, patch)),
            Self::Name { name } => ResolvableProfile::dynamic_name(name.clone(), patch),
        };
        profile.unwrap_or_default()
    }
}

/// Uploads the PNG at `url` through `MineSkin`; fails with words for the admin.
pub(super) async fn from_url(url: String) -> Result<Skin, String> {
    let textures = providers::mineskin(&url, false, "Banker").await?;
    Ok(Skin::Texture {
        value: textures.value,
        signature: textures.signature.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use steel_registry::resolvable_profile::ResolvableProfileContents;

    use super::*;

    #[test]
    fn the_banker_has_a_signed_skin_of_his_own() {
        let Skin::Texture { value, signature } = Skin::default_skin() else {
            unreachable!("the skin of the banker is a texture");
        };
        assert!(!value.is_empty() && !signature.is_empty());
        let profile = Skin::default_skin().profile();
        let ResolvableProfileContents::StaticPartial(partial) = profile.contents() else {
            unreachable!("a texture is a partial profile");
        };
        assert_eq!(partial.properties().len(), 1);
    }

    #[test]
    fn a_nick_is_left_to_the_client_and_a_bad_one_falls_back() {
        let named = Skin::Name {
            name: "Steve".to_owned(),
        };
        assert_eq!(
            named.profile().contents(),
            &ResolvableProfileContents::DynamicName("Steve".to_owned())
        );
        let bad = Skin::Name {
            name: "no such nick, far too long for a player".to_owned(),
        };
        assert_eq!(bad.profile(), ResolvableProfile::default());
    }
}
