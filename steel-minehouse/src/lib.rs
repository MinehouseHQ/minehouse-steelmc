//! `MineHouse` server features for Steel: our survival mods, ported from the Fabric server.
//!
//! Everything `MineHouse`-specific lives in this crate so it stays out of the way of the vanilla
//! work in `steel-core`. The modules talk to the site (`/api/mc/*`) with the same signed
//! protocol as the Fabric mods, so the site, the Discord bot and Gate don't change.

// Auto-trait (`Send`/`Sync`) proofs through the World/chunk type graph exceed the default depth of 128.
#![recursion_limit = "256"]

use std::{net::IpAddr, sync::Arc};

use steel_core::{
    command::CommandRegistry,
    player::GameProfile,
    server::{
        Server,
        login_gate::{LoginAdmission, LoginDecision, LoginGate, PendingLogin},
    },
};
use text_components::TextComponent;

pub mod auth;
pub mod bank;
pub mod blocklog;
pub mod chat;
pub mod config;
pub mod console;
pub mod content;
pub mod downtime;
pub mod events;
pub mod guides;
pub mod migrate;
pub mod moderation;
pub mod nitro;
pub mod roles;
pub mod site;
pub mod stats;
pub mod tab;
pub mod villagers;
pub mod voice;

use auth::{Auth, AuthSetup};
use blocklog::BlockLog;
use chat::Chat;
use console::Console;
use downtime::Downtime;
use moderation::Moderation;
use roles::Roles;

/// What is prepared before the server exists: settings and the commands.
pub struct Setup {
    auth: Option<AuthSetup>,
    moderation: Option<Arc<Moderation>>,
    downtime: Option<Arc<Downtime>>,
    chat: Option<Arc<Chat>>,
    blocklog: Option<Arc<BlockLog>>,
}

/// The running `MineHouse` modules of one server.
pub struct Minehouse {
    console: Option<Arc<Console>>,
    /// Kept for the commands and syncs that will use it.
    _auth: Option<Arc<Auth>>,
    moderation: Option<Arc<Moderation>>,
    downtime: Option<Arc<Downtime>>,
    roles: Option<Roles>,
    stats: Option<stats::Stats>,
    voice: Option<Arc<voice::Voice>>,
    blocklog: Option<Arc<BlockLog>>,
}

impl Minehouse {
    /// Loads the settings and adds the `MineHouse` commands to `registry`. Call before the
    /// server is built: its command tree is fixed at startup.
    #[must_use]
    pub fn prepare(registry: &mut CommandRegistry) -> Setup {
        let auth = Auth::load();
        // The bans and the whitelist live on the same site as the login check.
        let moderation = auth.as_ref().and_then(|auth| {
            Moderation::register(
                registry,
                auth.site.clone(),
                auth.config.server_id.clone(),
                auth.config.messages.banned.clone(),
            )
        });
        Setup {
            auth,
            moderation,
            downtime: Downtime::register(registry),
            chat: Chat::register(registry),
            blocklog: {
                if let Err(error) = content::register(registry) {
                    log::error!("MineHouse items: /mhgive is off: {error}");
                }
                BlockLog::register(registry)
            },
        }
    }

    /// Starts every enabled module. Call once the server is ready for players.
    #[must_use]
    pub fn start(server: &Arc<Server>, setup: Setup) -> Self {
        content::start();
        content::start_jobs(server);
        if let Some(moderation) = &setup.moderation {
            moderation.start(server);
        }
        // Roles come from the same site as the login check.
        let roles = setup
            .auth
            .as_ref()
            .and_then(|auth| Roles::start(server, auth.site.clone()));
        // And who has Nitro (coloured nicknames in the tab list and chat).
        if let Some(auth) = &setup.auth {
            nitro::start(auth.site.clone(), auth.config.server_id.clone());
        }
        // So do the statistics of the players go to it.
        let stats = setup
            .auth
            .as_ref()
            .and_then(|auth| stats::Stats::start(server, auth.site.clone()));
        if let Some(downtime) = &setup.downtime {
            downtime.start(server);
        }
        if let Some(chat) = &setup.chat {
            chat.start(server);
        }
        tab::Tab::start(server);
        villagers::start();
        events::start(server, setup.downtime.clone(), setup.blocklog.clone());
        if let Some(blocklog) = &setup.blocklog {
            blocklog.start(server);
        }
        let voice = voice::Voice::start(server);
        // Lives in a static: the ATM and the terminals find it from block callbacks.
        let _bank = bank::Bank::start(server);
        let auth = setup.auth.map(|auth| Auth::start(server, auth));
        // One gate serves both: maintenance refuses first, then the login check asks the site.
        let gate = MinehouseGate {
            downtime: setup.downtime.clone(),
            auth: auth.clone(),
        };
        if !server.set_login_gate(Arc::new(gate)) {
            log::error!("Another login gate is already installed: no login check, no maintenance");
        }
        Self {
            console: Console::start(server),
            _auth: auth,
            moderation: setup.moderation,
            downtime: setup.downtime,
            roles,
            stats,
            voice,
            blocklog: setup.blocklog,
        }
    }

    /// Lets the modules report their last state while the server is still up.
    pub async fn shutdown(&self) {
        if let Some(moderation) = &self.moderation {
            moderation.stop();
        }
        if let Some(downtime) = &self.downtime {
            downtime.stop();
        }
        if let Some(roles) = &self.roles {
            roles.stop();
        }
        if let Some(stats) = &self.stats {
            stats.stop().await;
        }
        if let Some(voice) = &self.voice {
            voice.stop();
        }
        if let Some(console) = &self.console {
            console.stop().await;
        }
        // Last: whatever the other modules still did is in the log.
        if let Some(blocklog) = &self.blocklog {
            blocklog.stop();
        }
    }
}

/// The login gate of the server: maintenance mode first, then the login check.
struct MinehouseGate {
    downtime: Option<Arc<Downtime>>,
    auth: Option<Arc<Auth>>,
}

impl LoginGate for MinehouseGate {
    fn private_properties(&self) -> &[&str] {
        if self.auth.is_some() {
            Auth::PRIVATE_PROPERTIES
        } else {
            &[]
        }
    }

    fn refuse_login(
        &self,
        server: &Server,
        profile: &GameProfile,
        address: IpAddr,
    ) -> Option<TextComponent> {
        self.downtime
            .as_ref()
            .and_then(|downtime| downtime.refusal(server, profile.id(), profile.name()))
            .or_else(|| {
                self.auth
                    .as_ref()
                    .and_then(|auth| auth.refuse(server, profile, &address.to_string()))
            })
    }

    fn admit(&self, login: PendingLogin) -> LoginAdmission {
        match &self.auth {
            Some(auth) => Box::pin(Arc::clone(auth).admit(login)),
            None => Box::pin(async { LoginDecision::Allow }),
        }
    }
}
