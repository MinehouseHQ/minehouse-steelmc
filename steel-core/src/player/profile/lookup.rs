//! Online player name-to-identity lookup.

use reqwest::{StatusCode, Url};
use serde::Deserialize;
use thiserror::Error;
use tokio::time::{Duration, sleep};
use uuid::Uuid;

use steel_registry::game_profile::{GameProfile, ProfileProperty};

use super::known_players::KnownPlayer;

const DEFAULT_PROFILE_SERVER: &str =
    "https://api.minecraftservices.com/minecraft/profile/lookup/name";
const MAX_PROFILE_LOOKUP_ATTEMPTS: usize = 3;
const PROFILE_LOOKUP_RETRY_DELAY: Duration = Duration::from_millis(750);
/// Bounds every attempt so suspended administrative commands always release their ordering barrier.
const PROFILE_LOOKUP_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Failure while resolving a player identity through the configured profile service.
#[derive(Debug, Error)]
pub enum ProfileLookupError {
    /// No profile exists for the requested name.
    #[error("Unknown player {0}")]
    UnknownPlayer(String),
    /// The configured profile lookup endpoint is invalid.
    #[error("Invalid profile server URL configured: {0}")]
    InvalidProfileServer(String),
    /// The request failed before a response was received.
    #[error("Profile lookup failed for {name}: {source}")]
    Request {
        /// Requested player name.
        name: String,
        /// Transport error.
        source: reqwest::Error,
    },
    /// The service returned an unexpected status.
    #[error("Profile lookup service returned status {status} for {name}")]
    ServiceResponse {
        /// Requested player name.
        name: String,
        /// HTTP response status.
        status: StatusCode,
    },
    /// The service returned malformed identity data.
    #[error("Invalid profile lookup response for {name}: {reason}")]
    InvalidResponse {
        /// Requested player name.
        name: String,
        /// Response validation failure.
        reason: String,
    },
}

#[derive(Deserialize)]
struct ProfileLookupResponse {
    id: String,
    name: String,
}

/// Resolves one online-mode profile through the configured service.
///
/// The caller handles local caches, offline mode, and name validation first.
pub async fn lookup_online_profile(
    client: &reqwest::Client,
    profile_server: Option<&str>,
    name: &str,
) -> Result<KnownPlayer, ProfileLookupError> {
    let lookup_name = name.to_ascii_lowercase();
    let url = profile_lookup_url(profile_server, &lookup_name)?;
    for attempt in 1..=MAX_PROFILE_LOOKUP_ATTEMPTS {
        let result =
            lookup_online_profile_once(client, url.as_str(), name, PROFILE_LOOKUP_REQUEST_TIMEOUT)
                .await;
        match result {
            Ok(profile) => return Ok(profile),
            Err(error @ ProfileLookupError::UnknownPlayer(_)) => return Err(error),
            Err(error) if attempt == MAX_PROFILE_LOOKUP_ATTEMPTS => return Err(error),
            Err(_) => sleep(PROFILE_LOOKUP_RETRY_DELAY).await,
        }
    }
    unreachable!("the profile lookup attempt range is non-empty")
}

async fn lookup_online_profile_once(
    client: &reqwest::Client,
    url: &str,
    name: &str,
    request_timeout: Duration,
) -> Result<KnownPlayer, ProfileLookupError> {
    let response = client
        .get(url)
        .timeout(request_timeout)
        .send()
        .await
        .map_err(|source| ProfileLookupError::Request {
            name: name.to_owned(),
            source,
        })?;

    match response.status() {
        StatusCode::OK => parse_profile_response(response, name).await,
        StatusCode::NO_CONTENT | StatusCode::NOT_FOUND => {
            Err(ProfileLookupError::UnknownPlayer(name.to_owned()))
        }
        status => Err(ProfileLookupError::ServiceResponse {
            name: name.to_owned(),
            status,
        }),
    }
}

fn profile_lookup_url(
    profile_server: Option<&str>,
    normalized_name: &str,
) -> Result<Url, ProfileLookupError> {
    let server = profile_server.unwrap_or(DEFAULT_PROFILE_SERVER);
    let endpoint = format!("{}/{normalized_name}", server.trim_end_matches('/'));
    Url::parse(&endpoint).map_err(|_| ProfileLookupError::InvalidProfileServer(endpoint))
}

async fn parse_profile_response(
    response: reqwest::Response,
    requested_name: &str,
) -> Result<KnownPlayer, ProfileLookupError> {
    let profile = response
        .json::<ProfileLookupResponse>()
        .await
        .map_err(|source| ProfileLookupError::InvalidResponse {
            name: requested_name.to_owned(),
            reason: source.to_string(),
        })?;
    let uuid =
        Uuid::parse_str(&profile.id).map_err(|source| ProfileLookupError::InvalidResponse {
            name: requested_name.to_owned(),
            reason: source.to_string(),
        })?;
    Ok(KnownPlayer::new(uuid, profile.name))
}

/// Vanilla `YggdrasilMinecraftSessionService` profile endpoint.
const SESSION_PROFILE_SERVER: &str = "https://sessionserver.mojang.com/session/minecraft/profile";

#[derive(Deserialize)]
struct SessionProfileResponse {
    id: String,
    name: String,
    #[serde(default)]
    properties: Vec<SessionProfileProperty>,
}

#[derive(Deserialize)]
struct SessionProfileProperty {
    name: String,
    value: String,
    signature: Option<String>,
}

/// Vanilla `SessionService.fetchProfile(id, true)`: the full profile with
/// signed textures, or `None` when it is unknown or the service fails.
pub async fn fetch_session_profile(client: &reqwest::Client, id: Uuid) -> Option<GameProfile> {
    let url = session_profile_url(id);
    let response = client
        .get(url)
        .timeout(PROFILE_LOOKUP_REQUEST_TIMEOUT)
        .send()
        .await
        .inspect_err(|error| log::warn!("Profile fetch for {id} failed: {error}"))
        .ok()?;
    if response.status() != StatusCode::OK {
        return None;
    }
    let profile = response
        .json::<SessionProfileResponse>()
        .await
        .inspect_err(|error| log::warn!("Invalid profile response for {id}: {error}"))
        .ok()?;
    let properties = profile
        .properties
        .into_iter()
        .filter_map(|property| {
            ProfileProperty::new(property.name, property.value, property.signature).ok()
        })
        .collect();
    let id = Uuid::parse_str(&profile.id).ok()?;
    GameProfile::new(id, profile.name, properties).ok()
}

fn session_profile_url(id: Uuid) -> String {
    format!("{SESSION_PROFILE_SERVER}/{}?unsigned=false", id.simple())
}

#[cfg(test)]
mod tests {
    use super::{profile_lookup_url, session_profile_url};

    #[test]
    fn session_profile_url_asks_for_signed_textures() {
        let id = uuid::Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
        assert_eq!(
            session_profile_url(id),
            "https://sessionserver.mojang.com/session/minecraft/profile/\
             0123456789abcdef0123456789abcdef?unsigned=false"
        );
    }

    #[test]
    fn profile_lookup_url_uses_mojangs_default_endpoint() {
        let url = profile_lookup_url(None, "steve");
        let Ok(url) = url else {
            panic!("default profile lookup URL should build");
        };
        assert_eq!(
            url.as_str(),
            "https://api.minecraftservices.com/minecraft/profile/lookup/name/steve"
        );
    }

    #[test]
    fn profile_lookup_url_uses_the_configured_endpoint() {
        let url = profile_lookup_url(Some("https://profiles.example.com/lookup/"), "steve");
        let Ok(url) = url else {
            panic!("configured profile lookup URL should build");
        };
        assert_eq!(url.as_str(), "https://profiles.example.com/lookup/steve");
    }
}
