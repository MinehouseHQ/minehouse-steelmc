//! The login calls of the site API (minehouse-landing `mod/README.md`).

use std::fmt::Write as _;

use serde::Deserialize;
use serde_json::json;

use crate::site::{SiteClient, SiteError};

/// Answer of `POST /api/mc/check`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    /// A trusted device, or a licensed login: let the player in.
    Allow,
    /// The player has to confirm this login with a code.
    Pending {
        /// Login request to poll.
        request_id: String,
        /// Six-digit code to show in the dialog.
        code: String,
        /// Seconds until the request expires.
        expires_in: u64,
    },
    /// Not in the Discord server or not on the whitelist; the site may say why.
    NotInGuild(Option<String>),
    /// The account is banned; the site may say why and until when.
    Banned(Option<String>),
}

/// Status of a login request, from `GET /api/mc/request/{id}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestStatus {
    /// Nobody answered yet.
    Pending,
    /// The owner confirmed the code.
    Approved,
    /// The owner refused the login.
    Denied,
    /// The code expired.
    Expired,
    /// A newer login replaced this one, or it was cancelled.
    Cancelled,
}

#[derive(Deserialize)]
struct CheckAnswer {
    status: String,
    request_id: Option<String>,
    code: Option<String>,
    expires_in: Option<u64>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct StatusAnswer {
    status: RequestStatus,
}

/// Parses the answer of `/api/mc/check`.
pub fn parse_check(text: &str) -> Result<Check, SiteError> {
    let answer: CheckAnswer = serde_json::from_str(text)
        .map_err(|error| SiteError::Answer(format!("bad check answer: {error}")))?;
    let message = answer.message.filter(|message| !message.trim().is_empty());
    match answer.status.as_str() {
        "allow" => Ok(Check::Allow),
        "not_in_guild" => Ok(Check::NotInGuild(message)),
        "banned" => Ok(Check::Banned(message)),
        "pending" => match (answer.request_id, answer.code) {
            (Some(request_id), Some(code)) => Ok(Check::Pending {
                request_id,
                code,
                expires_in: answer.expires_in.unwrap_or(300),
            }),
            _ => Err(SiteError::Answer(
                "pending check without request_id or code".to_owned(),
            )),
        },
        other => Err(SiteError::Answer(format!("unknown check status: {other}"))),
    }
}

impl SiteClient {
    /// `POST /api/mc/check`: may the player join, or must they confirm a code first.
    /// `premium` says the proxy verified the login with Mojang.
    pub async fn check(
        &self,
        nickname: &str,
        ip: &str,
        server: &str,
        premium: bool,
    ) -> Result<Check, SiteError> {
        let mut body = json!({ "nickname": nickname, "ip": ip, "server": server });
        if premium {
            body["premium"] = json!(true);
        }
        parse_check(&self.post_json("/api/mc/check", body.to_string()).await?)
    }

    /// `GET /api/mc/request/{id}`: current status of a login request.
    pub async fn request_status(&self, request_id: &str) -> Result<RequestStatus, SiteError> {
        let text = self
            .get(&format!("/api/mc/request/{}", segment(request_id)))
            .await?;
        serde_json::from_str::<StatusAnswer>(&text)
            .map(|answer| answer.status)
            .map_err(|error| SiteError::Answer(format!("bad request status: {error}")))
    }

    /// `POST /api/mc/request/{id}/cancel`: the player pressed «Отмена» or disconnected.
    pub async fn cancel_request(&self, request_id: &str) -> Result<(), SiteError> {
        let path = format!("/api/mc/request/{}/cancel", segment(request_id));
        self.post_json(&path, "{}".to_owned()).await.map(|_| ())
    }
}

/// A request id as a path segment. The site makes UUIDs; anything else is percent-encoded so
/// the signed path and the requested one can't differ.
fn segment(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for byte in id.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_check_status() {
        assert_eq!(
            parse_check(r#"{"status":"allow"}"#).ok(),
            Some(Check::Allow)
        );
        assert_eq!(
            parse_check(
                r#"{"status":"pending","request_id":"r1","code":"482913","expires_in":120}"#
            )
            .ok(),
            Some(Check::Pending {
                request_id: "r1".to_owned(),
                code: "482913".to_owned(),
                expires_in: 120
            })
        );
        assert_eq!(
            parse_check(r#"{"status":"banned","message":"до завтра"}"#).ok(),
            Some(Check::Banned(Some("до завтра".to_owned())))
        );
        assert_eq!(
            parse_check(r#"{"status":"not_in_guild","message":" "}"#).ok(),
            Some(Check::NotInGuild(None))
        );
        assert!(parse_check(r#"{"status":"pending"}"#).is_err());
        assert!(parse_check(r#"{"status":"what"}"#).is_err());
    }

    #[test]
    fn request_ids_stay_one_path_segment() {
        assert_eq!(segment("3f2a-11"), "3f2a-11");
        assert_eq!(segment("a/b c"), "a%2Fb%20c");
    }
}
