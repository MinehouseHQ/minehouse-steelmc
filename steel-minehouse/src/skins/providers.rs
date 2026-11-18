//! Where skins come from: the licensed account with a nickname (Mojang), Ely.by, or a link
//! to a PNG that `MineSkin` signs.
//!
//! Vanilla clients show only textures with a valid signature, so every skin here comes
//! signed: Mojang and Ely.by sign their own, `MineSkin` uploads a picture to a Minecraft
//! account and returns Mojang's signed textures of it.

use std::{sync::LazyLock, time::Duration};

use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::store::Textures;
use crate::config;

const MOJANG_PROFILE_BY_NAME: &str = "https://api.mojang.com/users/profiles/minecraft/";
const MOJANG_SESSION_PROFILE: &str = "https://sessionserver.mojang.com/session/minecraft/profile/";
const ELY_BY_SIGNED_TEXTURES: &str = "https://skinsystem.ely.by/textures/signed/";
const MINESKIN_GENERATE: &str = "https://api.mineskin.org/v2/generate";

/// Why a skin was not found, in words for the player.
pub(crate) type Failure = String;

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .user_agent("MineHouse-Steel")
        .build()
        .unwrap_or_default()
});

#[derive(Deserialize)]
struct NameAndId {
    id: String,
}

#[derive(Deserialize)]
struct ProfileWithProperties {
    #[serde(default)]
    properties: Vec<Textures>,
}

impl ProfileWithProperties {
    fn textures(self) -> Option<Textures> {
        self.properties
            .into_iter()
            .find(|property| property.name == "textures")
    }
}

/// Whether `name` can be a Minecraft nickname, so it may go into a URL.
fn is_nickname(name: &str) -> bool {
    (1..=16).contains(&name.len())
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// The signed textures of the licensed account `name`; `Ok(None)` when there is none.
pub(crate) async fn mojang(name: &str) -> Result<Option<Textures>, Failure> {
    if !is_nickname(name) {
        return Ok(None);
    }
    let failed = |error: reqwest::Error| format!("Mojang не отвечает: {error}");
    let response = HTTP
        .get(format!("{MOJANG_PROFILE_BY_NAME}{name}"))
        .send()
        .await
        .map_err(failed)?;
    match response.status() {
        StatusCode::OK => {}
        StatusCode::NO_CONTENT | StatusCode::NOT_FOUND => return Ok(None),
        status => return Err(format!("Mojang ответил {status}")),
    }
    let id = response.json::<NameAndId>().await.map_err(failed)?.id;
    let response = HTTP
        .get(format!("{MOJANG_SESSION_PROFILE}{id}?unsigned=false"))
        .send()
        .await
        .map_err(failed)?;
    match response.status() {
        StatusCode::OK => {}
        StatusCode::NO_CONTENT | StatusCode::NOT_FOUND => return Ok(None),
        status => return Err(format!("Mojang ответил {status}")),
    }
    Ok(response
        .json::<ProfileWithProperties>()
        .await
        .map_err(failed)?
        .textures())
}

/// The signed textures of the Ely.by account `name`; `Ok(None)` when there is none.
pub(crate) async fn ely_by(name: &str) -> Result<Option<Textures>, Failure> {
    if !is_nickname(name) {
        return Ok(None);
    }
    let failed = |error: reqwest::Error| format!("Ely.by не отвечает: {error}");
    let response = HTTP
        .get(format!("{ELY_BY_SIGNED_TEXTURES}{name}"))
        .send()
        .await
        .map_err(failed)?;
    match response.status() {
        StatusCode::OK => {}
        StatusCode::NO_CONTENT | StatusCode::NOT_FOUND => return Ok(None),
        status => return Err(format!("Ely.by ответил {status}")),
    }
    Ok(response
        .json::<ProfileWithProperties>()
        .await
        .map_err(failed)?
        .textures())
}

/// What `MineSkin` answered, as textures or as words for the player.
fn mineskin_answer(status: u16, body: &str) -> Result<Textures, Failure> {
    let answer: Value =
        serde_json::from_str(body).map_err(|_| format!("MineSkin ответил {status}"))?;
    let data = answer
        .get("skin")
        .and_then(|skin| skin.get("texture"))
        .and_then(|texture| texture.get("data"));
    let text = |key: &str| data?.get(key)?.as_str().map(str::to_owned);
    if let (Some(value), Some(signature)) = (text("value"), text("signature")) {
        return Ok(Textures {
            name: "textures".to_owned(),
            value,
            signature: Some(signature),
        });
    }
    let why = answer
        .get("errors")
        .and_then(|errors| errors.get(0))
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map_or_else(|| format!("код {status}"), str::to_owned);
    Err(format!("MineSkin не принял скин: {why}"))
}

/// Uploads the PNG at `url` through `MineSkin`. `slim` is the variant with thin arms.
/// `MINEHOUSE_MINESKIN_KEY` raises `MineSkin`'s limits; it works without one.
pub(crate) async fn mineskin(url: &str, slim: bool, name: &str) -> Result<Textures, Failure> {
    let failed = |error: reqwest::Error| format!("MineSkin не отвечает: {error}");
    let body = json!({
        "url": url,
        "variant": if slim { "slim" } else { "classic" },
        "visibility": "unlisted",
        "name": name,
    });
    let mut request = HTTP
        .post(MINESKIN_GENERATE)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .body(body.to_string());
    if let Some(key) = config::env_value("MINEHOUSE_MINESKIN_KEY") {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.map_err(failed)?;
    let status = response.status().as_u16();
    let body = response.text().await.map_err(failed)?;
    mineskin_answer(status, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mineskin_answers_become_textures_or_words() {
        let ok = r#"{"skin":{"texture":{"data":{"value":"v","signature":"s"}}}}"#;
        assert_eq!(
            mineskin_answer(200, ok).map(|textures| textures.value),
            Ok("v".to_owned())
        );
        let refused =
            r#"{"success":false,"errors":[{"code":"invalid_image","message":"Invalid image"}]}"#;
        assert_eq!(
            mineskin_answer(400, refused),
            Err("MineSkin не принял скин: Invalid image".to_owned())
        );
        assert_eq!(
            mineskin_answer(500, "{}"),
            Err("MineSkin не принял скин: код 500".to_owned())
        );
        assert_eq!(
            mineskin_answer(502, "<html>"),
            Err("MineSkin ответил 502".to_owned())
        );
    }

    #[test]
    fn only_nicknames_go_into_urls() {
        assert!(is_nickname("Timur_42"));
        assert!(!is_nickname("../etc"));
        assert!(!is_nickname(""));
    }
}
