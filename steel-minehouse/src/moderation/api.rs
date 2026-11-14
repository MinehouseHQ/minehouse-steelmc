//! The ban, warn and whitelist calls of the site API (minehouse-landing `mod/README.md`).

use serde_json::{Value, json};
use uuid::Uuid;

use super::wipe::SiteWipe;
use crate::site::{SiteClient, SiteError};

/// Who runs a command: the player's UUID (none for the console) and the name to show.
#[derive(Clone, Debug)]
pub struct Actor {
    /// The operator, `None` for the console.
    pub uuid: Option<Uuid>,
    /// Shown as the author on the site.
    pub name: String,
}

/// `POST /api/mc/ban` for a known nickname.
#[derive(Debug, PartialEq, Eq)]
pub struct Banned {
    /// The nickname in its pinned case.
    pub nickname: String,
    /// ISO time, `None` for a permanent ban.
    pub expires_at: Option<String>,
    /// Every linked nickname of the account, to kick whoever is online.
    pub uuids: Vec<Uuid>,
    /// Disconnect text for the banned player.
    pub message: Option<String>,
}

/// `POST /api/mc/unban`.
#[derive(Debug, PartialEq, Eq)]
pub enum Unban {
    /// The ban is lifted; with every nickname of the account, for `--wipe`.
    Done(String, Vec<Uuid>),
    /// The account had no active ban.
    NotBanned(String),
    /// The site knows no such nickname.
    NotFound,
}

/// One line of `GET /api/mc/bans`.
#[derive(Debug, PartialEq, Eq)]
pub struct BanEntry {
    /// Row id; `None` on an old site that does not send it.
    pub id: Option<i64>,
    /// The banned nickname.
    pub nickname: String,
    /// Why.
    pub reason: Option<String>,
    /// ISO time, `None` for a permanent ban.
    pub expires_at: Option<String>,
    /// Who banned.
    pub by: Option<String>,
    /// Where the ban was made: `site`, `game` or `discord`.
    pub source: Option<String>,
    /// ISO time the ban was made.
    pub created_at: Option<String>,
}

/// One player of `POST /api/mc/bans/online` to kick.
#[derive(Debug, PartialEq, Eq)]
pub struct OnlineBan {
    /// The online player.
    pub uuid: Uuid,
    /// Disconnect text.
    pub message: Option<String>,
}

/// `POST /api/mc/warn` for a known nickname.
#[derive(Debug, PartialEq, Eq)]
pub struct Warned {
    /// The nickname in its pinned case.
    pub nickname: String,
    /// Active warns after this one, 0 when they turned into a ban.
    pub count: u32,
    /// The warn reached the limit and the account is banned now.
    pub banned: bool,
    /// How many warns there were when the ban fired.
    pub banned_at: u32,
    /// Reason of that ban.
    pub ban_reason: Option<String>,
    /// ISO end of that ban, `None` is forever.
    pub expires_at: Option<String>,
    /// Every linked nickname of the account, to kick whoever is online.
    pub uuids: Vec<Uuid>,
    /// Disconnect text for the banned player.
    pub message: Option<String>,
}

/// `POST /api/mc/unwarn`.
#[derive(Debug, PartialEq, Eq)]
pub enum Unwarn {
    /// The latest warn is removed; the number is how many are left.
    Done(String, u32),
    /// The account had no warns.
    NotWarned(String),
    /// The site knows no such nickname.
    NotFound,
}

/// One active warn of `POST /api/mc/warns`.
#[derive(Debug, PartialEq, Eq)]
pub struct WarnEntry {
    /// Why.
    pub reason: String,
    /// Who warned.
    pub by: Option<String>,
    /// ISO time the warn was made.
    pub created_at: Option<String>,
}

/// `POST /api/mc/whitelist`.
#[derive(Debug, PartialEq, Eq)]
pub struct Whitelisted {
    /// One of `added`, `already`, `removed`, `not_whitelisted`, `nickname_added`,
    /// `nickname_already`, `nickname_removed`, `not_found`, `unavailable`.
    pub status: String,
    /// The nickname, Discord name or ID the site resolved the target to.
    pub name: Option<String>,
}

fn parse(text: &str) -> Result<Value, SiteError> {
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(text).map_err(|error| SiteError::Answer(format!("invalid JSON: {error}")))
}

fn string(json: &Value, key: &str) -> Option<String> {
    json.get(key)?.as_str().map(str::to_owned)
}

fn number(json: &Value, key: &str) -> u32 {
    json.get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0)
}

fn uuids(json: &Value, key: &str) -> Vec<Uuid> {
    list(json, key)
        .iter()
        .filter_map(|value| Uuid::parse_str(value.as_str()?).ok())
        .collect()
}

fn list<'json>(json: &'json Value, key: &str) -> &'json [Value] {
    json.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn is_ok(json: &Value) -> bool {
    json.get("status").and_then(Value::as_str) == Some("ok")
}

fn nickname(json: &Value) -> String {
    string(json, "nickname").unwrap_or_else(|| "?".to_owned())
}

/// Parses `POST /api/mc/ban`; `None` when the site knows no such nickname.
pub fn parse_ban(text: &str) -> Result<Option<Banned>, SiteError> {
    let json = parse(text)?;
    Ok(is_ok(&json).then(|| Banned {
        nickname: nickname(&json),
        expires_at: string(&json, "expires_at"),
        uuids: uuids(&json, "uuids"),
        message: string(&json, "message"),
    }))
}

/// Parses `POST /api/mc/unban`.
pub fn parse_unban(text: &str) -> Result<Unban, SiteError> {
    let json = parse(text)?;
    match json.get("status").and_then(Value::as_str) {
        Some("ok") => Ok(Unban::Done(nickname(&json), uuids(&json, "uuids"))),
        Some("not_banned") => Ok(Unban::NotBanned(nickname(&json))),
        Some("not_found") => Ok(Unban::NotFound),
        other => Err(SiteError::Answer(format!(
            "unknown unban status: {other:?}"
        ))),
    }
}

/// Parses `POST /api/mc/wipes`.
pub fn parse_wipes(text: &str) -> Result<Vec<SiteWipe>, SiteError> {
    let json = parse(text)?;
    Ok(list(&json, "wipes")
        .iter()
        .filter_map(|wipe| {
            Some(SiteWipe {
                id: wipe.get("id")?.as_i64()?,
                nickname: nickname(wipe),
                uuids: uuids(wipe, "uuids"),
            })
        })
        .collect())
}

/// Parses `GET /api/mc/bans`.
pub fn parse_bans(text: &str) -> Result<Vec<BanEntry>, SiteError> {
    let json = parse(text)?;
    Ok(list(&json, "bans")
        .iter()
        .filter(|ban| ban.is_object())
        .map(|ban| BanEntry {
            id: ban.get("id").and_then(Value::as_i64),
            nickname: nickname(ban),
            reason: string(ban, "reason"),
            expires_at: string(ban, "expires_at"),
            by: string(ban, "by"),
            source: string(ban, "source"),
            created_at: string(ban, "created_at"),
        })
        .collect())
}

/// Parses `POST /api/mc/bans/online`.
pub fn parse_online_bans(text: &str) -> Result<Vec<OnlineBan>, SiteError> {
    let json = parse(text)?;
    Ok(list(&json, "banned")
        .iter()
        .filter_map(|ban| {
            Some(OnlineBan {
                uuid: Uuid::parse_str(ban.get("uuid")?.as_str()?).ok()?,
                message: string(ban, "message"),
            })
        })
        .collect())
}

/// Parses `POST /api/mc/warn`; `None` when the site knows no such nickname.
pub fn parse_warn(text: &str) -> Result<Option<Warned>, SiteError> {
    let json = parse(text)?;
    Ok(is_ok(&json).then(|| Warned {
        nickname: nickname(&json),
        count: number(&json, "count"),
        banned: json.get("banned").and_then(Value::as_bool) == Some(true),
        banned_at: number(&json, "banned_at"),
        ban_reason: string(&json, "reason"),
        expires_at: string(&json, "expires_at"),
        uuids: uuids(&json, "uuids"),
        message: string(&json, "message"),
    }))
}

/// Parses `POST /api/mc/unwarn`.
pub fn parse_unwarn(text: &str) -> Result<Unwarn, SiteError> {
    let json = parse(text)?;
    match json.get("status").and_then(Value::as_str) {
        Some("ok") => Ok(Unwarn::Done(nickname(&json), number(&json, "count"))),
        Some("not_warned") => Ok(Unwarn::NotWarned(nickname(&json))),
        Some("not_found") => Ok(Unwarn::NotFound),
        other => Err(SiteError::Answer(format!(
            "unknown unwarn status: {other:?}"
        ))),
    }
}

/// Parses `POST /api/mc/warns`; `None` when the site knows no such nickname.
pub fn parse_warns(text: &str) -> Result<Option<(String, Vec<WarnEntry>)>, SiteError> {
    let json = parse(text)?;
    Ok(is_ok(&json).then(|| {
        let warns = list(&json, "warns")
            .iter()
            .filter(|warn| warn.is_object())
            .map(|warn| WarnEntry {
                reason: string(warn, "reason").unwrap_or_default(),
                by: string(warn, "by"),
                created_at: string(warn, "created_at"),
            })
            .collect();
        (nickname(&json), warns)
    }))
}

/// Parses `POST /api/mc/whitelist`.
pub fn parse_whitelist(text: &str) -> Result<Whitelisted, SiteError> {
    let json = parse(text)?;
    let status = string(&json, "status")
        .ok_or_else(|| SiteError::Answer("whitelist answer without status".to_owned()))?;
    Ok(Whitelisted {
        status,
        name: string(&json, "name"),
    })
}

impl SiteClient {
    /// Bans the Discord account behind the nickname, with all its nicknames.
    /// `duration_seconds` is `None` for a permanent ban.
    pub async fn ban(
        &self,
        target: &str,
        reason: &str,
        duration_seconds: Option<u64>,
        by: &Actor,
    ) -> Result<Option<Banned>, SiteError> {
        let body = json!({
            "target": target,
            "reason": reason,
            "duration_seconds": duration_seconds,
            "by_uuid": by.uuid,
            "by_name": by.name,
        });
        parse_ban(&self.post_json("/api/mc/ban", body.to_string()).await?)
    }

    /// Lifts the active ban of the account behind the nickname.
    pub async fn unban(&self, target: &str, reason: &str, by: &Actor) -> Result<Unban, SiteError> {
        let body = json!({
            "target": target,
            "reason": reason,
            "by_uuid": by.uuid,
            "by_name": by.name,
        });
        parse_unban(&self.post_json("/api/mc/unban", body.to_string()).await?)
    }

    /// Progress resets asked for on the site for `server` and not done yet.
    pub async fn wipes(&self, server: &str) -> Result<Vec<SiteWipe>, SiteError> {
        let body = json!({ "server": server });
        parse_wipes(&self.post_json("/api/mc/wipes", body.to_string()).await?)
    }

    /// Marks a reset done: how many nicknames had progress, and what went wrong if anything.
    pub async fn wipe_done(
        &self,
        id: i64,
        wiped: usize,
        error: Option<&str>,
    ) -> Result<(), SiteError> {
        let body = json!({ "id": id, "wiped": wiped, "error": error });
        self.post_json("/api/mc/wipes/done", body.to_string())
            .await
            .map(|_| ())
    }

    /// Active bans, newest first.
    pub async fn bans(&self) -> Result<Vec<BanEntry>, SiteError> {
        parse_bans(&self.get("/api/mc/bans").await?)
    }

    /// Which of the online players must leave: banned elsewhere or no longer whitelisted on
    /// `server`.
    pub async fn online_bans(
        &self,
        online: &[Uuid],
        server: &str,
    ) -> Result<Vec<OnlineBan>, SiteError> {
        let body = json!({ "uuids": online, "server": server });
        parse_online_bans(
            &self
                .post_json("/api/mc/bans/online", body.to_string())
                .await?,
        )
    }

    /// Warns the account behind the nickname. At `limit` active warns the site bans the
    /// account (`ban_seconds` `None` is forever) and clears the warns.
    pub async fn warn(
        &self,
        target: &str,
        reason: &str,
        by: &Actor,
        limit: u32,
        ban_seconds: Option<u64>,
        ban_reason: Option<&str>,
    ) -> Result<Option<Warned>, SiteError> {
        let body = json!({
            "target": target,
            "reason": reason,
            "by_uuid": by.uuid,
            "by_name": by.name,
            "limit": limit,
            "ban_seconds": ban_seconds,
            "ban_reason": ban_reason,
        });
        parse_warn(&self.post_json("/api/mc/warn", body.to_string()).await?)
    }

    /// Removes the latest active warn.
    pub async fn unwarn(&self, target: &str, by: &Actor) -> Result<Unwarn, SiteError> {
        let body = json!({
            "target": target,
            "by_uuid": by.uuid,
            "by_name": by.name,
        });
        parse_unwarn(&self.post_json("/api/mc/unwarn", body.to_string()).await?)
    }

    /// Active warns of the account behind the nickname, oldest first.
    pub async fn warns(&self, target: &str) -> Result<Option<(String, Vec<WarnEntry>)>, SiteError> {
        let body = json!({ "target": target });
        parse_warns(&self.post_json("/api/mc/warns", body.to_string()).await?)
    }

    /// Adds or removes a nickname or Discord ID on the whitelist of `server`. An unlinked
    /// nickname is remembered by the site and whitelists whoever links it first.
    pub async fn whitelist(
        &self,
        target: &str,
        add: bool,
        server: &str,
        by: &Actor,
    ) -> Result<Whitelisted, SiteError> {
        let body = json!({
            "target": target,
            "mode": if add { "add" } else { "remove" },
            "server": server,
            "by_uuid": by.uuid,
            "by_name": by.name,
        });
        parse_whitelist(
            &self
                .post_json("/api/mc/whitelist", body.to_string())
                .await?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UUID: &str = "0f6f2d5c-0c0b-4d5e-9d0a-1b2c3d4e5f60";

    #[test]
    fn ban_answers() {
        let banned = parse_ban(&format!(
            r#"{{"status":"ok","nickname":"Steve","expires_at":null,"uuids":["{UUID}","junk",null],"message":"Бан"}}"#
        ));
        assert!(matches!(
            banned,
            Ok(Some(Banned { ref nickname, expires_at: None, ref uuids, .. }))
                if nickname == "Steve" && uuids.len() == 1
        ));
        assert!(matches!(parse_ban(r#"{"status":"not_found"}"#), Ok(None)));
        assert!(parse_ban("<html>").is_err());
    }

    #[test]
    fn unban_and_unwarn_statuses() {
        assert!(matches!(
            parse_unban(
                r#"{"status":"ok","nickname":"Steve","uuids":["0f6f2d5c-0c0b-4d5e-9d0a-1b2c3d4e5f60"]}"#
            ),
            Ok(Unban::Done(name, uuids)) if name == "Steve" && uuids.len() == 1
        ));
        assert!(matches!(
            parse_unban(r#"{"status":"not_banned","nickname":"Steve"}"#),
            Ok(Unban::NotBanned(_))
        ));
        assert!(matches!(
            parse_unban(r#"{"status":"not_found"}"#),
            Ok(Unban::NotFound)
        ));
        assert!(parse_unban(r#"{"status":"teapot"}"#).is_err());
        assert!(matches!(
            parse_unwarn(r#"{"status":"ok","nickname":"Steve","count":2}"#),
            Ok(Unwarn::Done(_, 2))
        ));
        assert!(parse_unwarn("{}").is_err());
    }

    #[test]
    fn wipes_keep_rows_with_an_id() {
        let wipes = parse_wipes(
            r#"{"wipes":[{"id":3,"nickname":"Steve","uuids":["0f6f2d5c-0c0b-4d5e-9d0a-1b2c3d4e5f60","x"]},{"nickname":"NoId"},4]}"#,
        );
        assert!(matches!(
            wipes,
            Ok(ref wipes) if wipes.len() == 1 && wipes[0].id == 3 && wipes[0].uuids.len() == 1
        ));
        assert!(matches!(parse_wipes(""), Ok(wipes) if wipes.is_empty()));
    }

    #[test]
    fn lists_skip_broken_rows() {
        let bans = parse_bans(
            r#"{"bans":[{"id":7,"nickname":"A","reason":"r","expires_at":null,"by":"Admin","source":"site","created_at":"2026-11-07T15:00:00Z"},5]}"#,
        );
        assert!(matches!(bans, Ok(ref bans) if bans.len() == 1 && bans[0].id == Some(7)));
        assert!(matches!(parse_bans("{}"), Ok(bans) if bans.is_empty()));

        let online = parse_online_bans(&format!(
            r#"{{"banned":[{{"uuid":"{UUID}","message":"Бан"}},{{"uuid":"x"}}]}}"#
        ));
        assert!(matches!(online, Ok(online) if online.len() == 1));
    }

    #[test]
    fn warn_answers() {
        let warned = parse_warn(&format!(
            r#"{{"status":"ok","nickname":"Steve","count":0,"banned":true,"banned_at":3,"reason":"Предупреждений: 3","expires_at":null,"uuids":["{UUID}"]}}"#
        ));
        assert!(matches!(
            warned,
            Ok(Some(Warned {
                banned: true,
                banned_at: 3,
                count: 0,
                ..
            }))
        ));
        assert!(matches!(parse_warn(r#"{"status":"not_found"}"#), Ok(None)));

        let warns = parse_warns(
            r#"{"status":"ok","nickname":"Steve","warns":[{"reason":"мат","by":"Admin","created_at":"2026-11-07T15:00:00Z"}]}"#,
        );
        assert!(matches!(warns, Ok(Some((_, warns))) if warns.len() == 1));
        assert!(parse_whitelist("{}").is_err());
        assert!(matches!(
            parse_whitelist(r#"{"status":"added","name":"Steve"}"#),
            Ok(Whitelisted { ref status, .. }) if status == "added"
        ));
    }
}
