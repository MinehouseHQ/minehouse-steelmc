//! The banker's skin (the Fabric `BankerSkin`): a signed `textures` property or the nick of a
//! player.
//!
//! Vanilla clients only show skins from textures.minecraft.net with Mojang's signature, so a
//! skin from any other link goes through `MineSkin` (api.mineskin.org, no key needed), which
//! uploads it and returns the signed property.

use std::{sync::LazyLock, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use steel_registry::{
    game_profile::ProfileProperty,
    resolvable_profile::{PartialProfile, PlayerSkinPatch, ResolvableProfile},
};

/// The skin Timur picked for the banker.
const DEFAULT: &str = include_str!("skin.json");
const MINESKIN: &str = "https://api.mineskin.org/v2/generate";

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

/// What `MineSkin` answered, as a skin or as words for the admin.
fn answer(status: u16, body: &str) -> Result<Skin, String> {
    let answer: Value =
        serde_json::from_str(body).map_err(|_| format!("MineSkin ответил {status}"))?;
    let data = answer
        .get("skin")
        .and_then(|skin| skin.get("texture"))
        .and_then(|texture| texture.get("data"));
    let text = |key: &str| data?.get(key)?.as_str().map(str::to_owned);
    if let (Some(value), Some(signature)) = (text("value"), text("signature")) {
        return Ok(Skin::Texture { value, signature });
    }
    let why = answer
        .get("errors")
        .and_then(|errors| errors.get(0))
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map_or_else(|| format!("код {status}"), str::to_owned);
    Err(format!("MineSkin не принял скин: {why}"))
}

/// Uploads the PNG at `url` through `MineSkin`; fails with words for the admin.
pub(super) async fn from_url(url: String) -> Result<Skin, String> {
    let failed = |error: reqwest::Error| format!("MineSkin не отвечает: {error}");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(failed)?;
    let body = json!({"url": url, "visibility": "unlisted", "name": "Banker"});
    let response = client
        .post(MINESKIN)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("User-Agent", "minecraft-server")
        .body(body.to_string())
        .send()
        .await
        .map_err(failed)?;
    let status = response.status().as_u16();
    let body = response.text().await.map_err(failed)?;
    answer(status, &body)
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

    #[test]
    fn the_answer_of_mineskin_is_a_skin_or_a_reason() {
        let good = r#"{"skin":{"texture":{"data":{"value":"v","signature":"s"}}}}"#;
        assert_eq!(
            answer(200, good),
            Ok(Skin::Texture {
                value: "v".to_owned(),
                signature: "s".to_owned()
            })
        );
        let refused = r#"{"errors":[{"message":"invalid image"}]}"#;
        assert_eq!(
            answer(400, refused),
            Err("MineSkin не принял скин: invalid image".to_owned())
        );
        assert_eq!(
            answer(500, "{}"),
            Err("MineSkin не принял скин: код 500".to_owned())
        );
        assert_eq!(
            answer(502, "<html>"),
            Err("MineSkin ответил 502".to_owned())
        );
    }
}
