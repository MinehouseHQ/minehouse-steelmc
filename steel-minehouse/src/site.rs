//! Signed calls to the site's `/api/mc/*`, the same scheme as the Fabric mods and Gate
//! (minehouse-landing `mod/README.md`):
//! `hex(HMAC-SHA256(secret, timestamp \n nonce \n METHOD \n path \n hex(SHA-256(body))))`.

use std::{
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use hmac::{Hmac, KeyInit, Mac};
use reqwest::redirect::Policy;
use sha2::{Digest, Sha256};

/// A failed call to the site.
#[derive(Debug)]
pub enum SiteError {
    /// Network error, timeout or unreadable answer.
    Http(reqwest::Error),
    /// The site answered with a non-2xx status.
    Status {
        /// HTTP status code.
        code: u16,
        /// Start of the answer body.
        body: String,
    },
}

impl fmt::Display for SiteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(error) => write!(f, "{error}"),
            Self::Status { code, body } => write!(f, "HTTP {code}: {body}"),
        }
    }
}

impl From<reqwest::Error> for SiteError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error)
    }
}

/// Client of the site for one secret and one server id.
pub struct SiteClient {
    http: reqwest::Client,
    base_url: String,
    secret: Vec<u8>,
    server_id: String,
}

impl SiteClient {
    /// `base_url` without the path (`https://minehouse.pro`); `server_id` goes into
    /// `X-MH-Server` so the site checks the signature with this server's own secret.
    pub fn new(
        base_url: &str,
        secret: &str,
        timeout: Duration,
        server_id: &str,
    ) -> Result<Self, reqwest::Error> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .redirect(Policy::none())
            // A connection that died during an outage must not be reused forever.
            .pool_idle_timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
            secret: secret.as_bytes().to_vec(),
            server_id: server_id.to_owned(),
        })
    }

    /// `POST <path>` with a JSON body. Returns the answer body of a 2xx answer.
    pub async fn post_json(&self, path: &str, body: String) -> Result<String, SiteError> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();
        let nonce = hex::encode(rand::random::<[u8; 16]>());
        let signature = sign(&self.secret, &timestamp, &nonce, "POST", path, &body);

        let response = self
            .http
            .post(format!("{}{path}", self.base_url))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("X-MH-Server", &self.server_id)
            .header("X-MH-Timestamp", timestamp)
            .header("X-MH-Nonce", nonce)
            .header("X-MH-Signature", signature)
            .body(body)
            .send()
            .await?;

        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            return Err(SiteError::Status {
                code: status.as_u16(),
                body: text.chars().take(200).collect(),
            });
        }
        Ok(text)
    }
}

/// Signature of one request.
#[must_use]
pub fn sign(
    secret: &[u8],
    timestamp: &str,
    nonce: &str,
    method: &str,
    path: &str,
    body: &str,
) -> String {
    let body_hash = hex::encode(Sha256::digest(body.as_bytes()));
    let payload = format!("{timestamp}\n{nonce}\n{method}\n{path}\n{body_hash}");
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        unreachable!("HMAC accepts keys of any length");
    };
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::sign;

    /// Same value as the site's `lib/mc-api.ts` and the Fabric mods compute for this request.
    #[test]
    fn signature_matches_the_site() {
        assert_eq!(
            sign(
                b"s3cret",
                "1700000000",
                "abcd",
                "POST",
                "/api/mc/console/poll",
                "{}"
            ),
            "0bd9777510d3768ef338221d2758a808e4827d339c481d4ee0ec9dbca90e4710"
        );
    }
}
