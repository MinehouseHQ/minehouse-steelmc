//! Server-side login check, ported from the Fabric `minehouse-auth` mod.
//!
//! Every player the proxy forwards is held in the configuration phase while the site says
//! whether this nickname + IP is a trusted device (`POST /api/mc/check`). If not, the player
//! sees a code, confirms it on the site or with the Discord bot, and gets in once the site
//! says so. The site also turns away players who are banned, not in Discord or not on the
//! whitelist of this server.
//!
//! Gate does the first half of the Fabric mod: it checks licensed nicknames with Mojang and
//! gives the player the stable UUID from the site (`/api/mc/profile`), so that part is not
//! here. Steel must run behind the proxy for this module to be safe.

mod api;
mod dialog;
mod settings;
mod trusted;

use std::{
    net::IpAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use rustc_hash::FxHashMap;
use steel_core::server::{
    Server,
    login_gate::{LoginAdmission, LoginConnection, LoginDecision, LoginGate, PendingLogin},
};
use steel_protocol::packets::{common::CClearDialog, config::CShowDialog};
use steel_registry::game_profile::GameProfile;
use steel_utils::locks::SyncMutex;
use text_components::TextComponent;
use tokio::{sync::Notify, time::sleep};

pub use api::{Check, RequestStatus};
pub use settings::{AuthConfig, Messages};
pub use trusted::TrustedCache;

use crate::{config, site::SiteClient};

/// Property Gate adds to the forwarded profile of a login it verified with Mojang.
const MOJANG_ID_PROPERTY: &str = "minehouse:mojang_id";

/// A login that is being checked, by lower-case nickname.
struct Session {
    id: u64,
    ip: String,
    /// Woken when a new login from the same IP takes over.
    replaced: Arc<Notify>,
}

/// The login check of one server.
pub struct Auth {
    config: AuthConfig,
    site: SiteClient,
    trusted: TrustedCache,
    sessions: SyncMutex<FxHashMap<String, Session>>,
    next_session: AtomicU64,
}

impl Auth {
    /// Loads `config/minehouse/auth.toml` and installs the login gate, unless the check is
    /// turned off.
    pub fn start(server: &Arc<Server>) -> Option<Arc<Self>> {
        let config = match config::load::<AuthConfig>(&config::path("auth")) {
            Ok(config) => config.resolved(),
            Err(error) => {
                // A broken config must not open the server to everyone.
                log::error!("Bad auth config, using the defaults: {error}");
                AuthConfig::default().resolved()
            }
        };
        if !config.enabled {
            log::warn!("Login check is disabled in config/minehouse/auth.toml");
            return None;
        }
        if config.api_secret.is_empty() {
            log::error!(
                "No API secret: set api_secret in config/minehouse/auth.toml or MINEHOUSE_API_SECRET. Only players from the local cache will get in."
            );
        }
        if server.config.velocity_secret.is_none() {
            log::warn!(
                "Login check without proxy forwarding: nicknames are not verified and the stable UUIDs of the site are not used. Run Steel behind Gate."
            );
        }

        let site = match SiteClient::new(
            &config.api_url,
            &config.api_secret,
            Duration::from_millis(config.http_timeout_ms),
            &config.server_id,
        ) {
            Ok(site) => site,
            Err(error) => {
                log::error!("Login check has no HTTP client, nobody new will get in: {error}");
                return None;
            }
        };
        let trusted = TrustedCache::load(
            config::data_path("auth-trusted.json"),
            Duration::from_secs(config.offline_trust_days * 24 * 60 * 60),
        );

        let auth = Arc::new(Self {
            config,
            site,
            trusted,
            sessions: SyncMutex::new(FxHashMap::default()),
            next_session: AtomicU64::new(1),
        });
        if !server.set_login_gate(Arc::new(Gate(Arc::clone(&auth)))) {
            log::error!("Another login gate is already installed, the login check is off");
            return None;
        }
        log::info!(
            "Login check on: API {}, server {:?}",
            auth.config.api_url,
            auth.config.server_id
        );
        Some(auth)
    }

    const fn messages(&self) -> &Messages {
        &self.config.messages
    }

    fn text(&self, message: &str) -> TextComponent {
        TextComponent::plain(message.replace("{invite}", &self.config.discord_invite))
    }

    /// Registers the login under its nickname. An unfinished login with the same nickname
    /// is told to step aside; [`Auth::refuse`] let this one through only when it may.
    fn open_session(self: &Arc<Self>, nickname: &str, ip: &str) -> SessionGuard {
        let id = self.next_session.fetch_add(1, Ordering::Relaxed);
        let replaced = Arc::new(Notify::new());
        let key = nickname.to_lowercase();
        let previous = self.sessions.lock().insert(
            key.clone(),
            Session {
                id,
                ip: ip.to_owned(),
                replaced: Arc::clone(&replaced),
            },
        );
        if let Some(previous) = previous {
            previous.replaced.notify_one();
        }
        SessionGuard {
            auth: Arc::clone(self),
            key,
            id,
            replaced,
        }
    }

    /// Login is refused while someone with this nickname (any case) or UUID is already
    /// playing, so a stranger under your nickname cannot kick you off the server. A login
    /// that is still confirming a code is replaced when the new connection comes from the
    /// same IP (the player reconnected); a login from another IP blocks the nickname until
    /// its code expires.
    fn refuse(&self, server: &Server, profile: &GameProfile, ip: &str) -> Option<TextComponent> {
        let nickname = profile.name();
        let online = server.get_players().iter().any(|player| {
            player.gameprofile.id() == profile.id()
                || player.gameprofile.name().eq_ignore_ascii_case(nickname)
        });
        if online {
            return Some(
                self.text(
                    &self
                        .messages()
                        .already_online
                        .replace("{nickname}", nickname),
                ),
            );
        }

        let sessions = self.sessions.lock();
        let session = sessions.get(&nickname.to_lowercase())?;
        if session.ip == ip {
            log::info!("{nickname} reconnected from the same IP, replacing the unfinished login");
            return None;
        }
        Some(
            self.text(
                &self
                    .messages()
                    .already_authenticating
                    .replace("{nickname}", nickname),
            ),
        )
    }

    async fn admit(self: Arc<Self>, login: PendingLogin) -> LoginDecision {
        let nickname = login.profile.name().to_owned();
        let ip = login.address.to_string();
        let premium = login
            .private_properties
            .iter()
            .any(|property| property.name() == MOJANG_ID_PROPERTY);
        let mut connection = login.connection;

        let session = self.open_session(&nickname, &ip);
        let decision = tokio::select! {
            decision = self.decide(&nickname, &ip, premium, &mut connection) => decision,
            () = session.replaced.notified() => Err((
                self.messages().replaced.clone(),
                "replaced by a new login from the same IP",
            )),
        };
        drop(session);

        match decision {
            Ok(reason) => {
                log::info!("{nickname} ({ip}) allowed: {reason}");
                LoginDecision::Allow
            }
            Err((message, reason)) => {
                log::info!("{nickname} ({ip}) disconnected: {reason}");
                LoginDecision::Deny(Box::new(self.text(&message)))
            }
        }
    }

    /// `Ok` with the reason to let the player in, or `Err` with the text for the player and
    /// the reason for the log.
    async fn decide(
        &self,
        nickname: &str,
        ip: &str,
        premium: bool,
        connection: &mut LoginConnection,
    ) -> Result<&'static str, (String, &'static str)> {
        let messages = self.messages();
        let check = self
            .site
            .check(nickname, ip, &self.config.server_id, premium)
            .await;
        let (request_id, code, expires_in) = match check {
            Ok(Check::Allow) => {
                self.trusted.trust(nickname, ip);
                return Ok("trusted device");
            }
            Ok(Check::NotInGuild(message)) => {
                self.trusted.forget(nickname, ip);
                return Err((
                    message.unwrap_or_else(|| messages.not_in_guild.clone()),
                    "not in Discord or not on the whitelist",
                ));
            }
            Ok(Check::Banned(message)) => {
                self.trusted.forget(nickname, ip);
                return Err((message.unwrap_or_else(|| messages.banned.clone()), "banned"));
            }
            Ok(Check::Pending {
                request_id,
                code,
                expires_in,
            }) => (request_id, code, expires_in),
            Err(error) => {
                log::warn!("Auth API unavailable while checking {nickname} ({error})");
                return if self.trusted.is_trusted(nickname, ip) {
                    Ok("site down, trusted by local cache")
                } else {
                    Err((
                        messages.api_unavailable.clone(),
                        "site down, not in local cache",
                    ))
                };
            }
        };

        // Cancels the request on the site if the player leaves, presses «Отмена» or is
        // replaced before the site settles it.
        let mut request = PendingRequest {
            site: self.site.clone(),
            id: Some(request_id),
        };
        let request_id = request.id.clone().unwrap_or_default();
        log::info!("{nickname} must confirm login request {request_id}");
        connection.send(CShowDialog::new(dialog::code_dialog(&self.config, &code)));

        let poll = Duration::from_millis(self.config.poll_interval_ms);
        let deadline = Instant::now()
            + Duration::from_secs(expires_in)
            + Duration::from_millis(self.config.http_timeout_ms);
        let mut clicks_open = true;
        loop {
            tokio::select! {
                click = connection.next_click(), if clicks_open => match click {
                    Some(id) if id == dialog::cancel_action() => {
                        return Err((messages.cancelled.clone(), "cancelled by player"));
                    }
                    Some(_) => {}
                    None => clicks_open = false,
                },
                () = sleep(poll) => {
                    if Instant::now() > deadline {
                        return Err((messages.expired.clone(), "code expired"));
                    }
                    match self.site.request_status(&request_id).await {
                        Ok(RequestStatus::Pending) => {}
                        Ok(RequestStatus::Approved) => {
                            request.settled();
                            self.trusted.trust(nickname, ip);
                            connection.send(CClearDialog {});
                            return Ok("approved login request");
                        }
                        Ok(RequestStatus::Denied) => {
                            request.settled();
                            return Err((messages.denied.clone(), "denied by owner"));
                        }
                        Ok(RequestStatus::Expired) => {
                            request.settled();
                            return Err((messages.expired.clone(), "code expired"));
                        }
                        Ok(RequestStatus::Cancelled) => {
                            request.settled();
                            return Err((
                                messages.replaced.clone(),
                                "request replaced or cancelled",
                            ));
                        }
                        // The site may be down for a moment; keep waiting until the code
                        // expires.
                        Err(error) => log::debug!("Polling {request_id} failed: {error}"),
                    }
                }
            }
        }
    }
}

/// Removes the session from the table when the login is over, however it ends.
struct SessionGuard {
    auth: Arc<Auth>,
    key: String,
    id: u64,
    replaced: Arc<Notify>,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let mut sessions = self.auth.sessions.lock();
        if sessions
            .get(&self.key)
            .is_some_and(|session| session.id == self.id)
        {
            sessions.remove(&self.key);
        }
    }
}

/// A login request the site still waits on.
struct PendingRequest {
    site: SiteClient,
    id: Option<String>,
}

impl PendingRequest {
    /// The site settled the request itself: nothing to cancel.
    fn settled(&mut self) {
        self.id = None;
    }
}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        let site = self.site.clone();
        tokio::spawn(async move {
            if let Err(error) = site.cancel_request(&id).await {
                log::debug!("Could not cancel request {id}: {error}");
            }
        });
    }
}

struct Gate(Arc<Auth>);

impl LoginGate for Gate {
    fn private_properties(&self) -> &[&str] {
        &[MOJANG_ID_PROPERTY]
    }

    fn refuse_login(
        &self,
        server: &Server,
        profile: &GameProfile,
        address: IpAddr,
    ) -> Option<TextComponent> {
        self.0.refuse(server, profile, &address.to_string())
    }

    fn admit(&self, login: PendingLogin) -> LoginAdmission {
        Box::pin(Arc::clone(&self.0).admit(login))
    }
}
