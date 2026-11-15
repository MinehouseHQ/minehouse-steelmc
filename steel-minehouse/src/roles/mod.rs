//! Discord roles → permission groups: a port of `roleSync` of the Fabric mod `minehouse-auth`,
//! where the groups were `LuckPerms` ones. Discord wins: a group from the mapping follows its
//! role, every other group of the player is left alone.

use std::{
    collections::BTreeMap,
    convert::Infallible,
    sync::Arc,
    time::{Duration, Instant},
};

use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};
use steel_core::{permission::PermissionSubjectState, server::Server};
use steel_utils::locks::SyncMutex;
use tokio::{task::JoinHandle, time::sleep};
use uuid::Uuid;

use crate::{config, site::SiteClient};

pub mod mapping;

use mapping::{PlayerRoles, RoleMapping};

/// How often to look for players who just joined: they are synced at once.
const JOIN_CHECK: Duration = Duration::from_secs(3);

/// `config/minehouse/roles.toml`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RolesConfig {
    /// Turns the sync off.
    pub enabled: bool,
    /// How often to re-check the players online; they are also checked right after joining.
    pub sync_seconds: u64,
    /// Discord role (name, case-insensitive, or ID) → group of `config/groups.toml`, or
    /// several separated by commas. Groups not listed here are never touched. Empty = the
    /// sync does nothing.
    pub groups: BTreeMap<String, String>,
}

impl Default for RolesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sync_seconds: 60,
            groups: BTreeMap::new(),
        }
    }
}

impl RolesConfig {
    fn load() -> Self {
        config::load(&config::path("roles")).unwrap_or_else(|error| {
            log::warn!("Bad roles config, role sync is off: {error}");
            Self {
                enabled: false,
                ..Self::default()
            }
        })
    }
}

/// The running role sync of one server.
pub struct Roles {
    task: SyncMutex<Option<JoinHandle<()>>>,
}

impl Roles {
    /// Starts the sync, unless the config turns it off or maps nothing.
    #[must_use]
    pub fn start(server: &Arc<Server>, site: SiteClient) -> Option<Self> {
        let config = RolesConfig::load();
        let mapping = RoleMapping::new(&config.groups);
        if !config.enabled || mapping.is_empty() {
            log::info!("Discord role sync is off: no groups in config/minehouse/roles.toml");
            return None;
        }

        log::info!("Discord role sync on: {:?}", config.groups);
        let task = tokio::spawn(run(
            Arc::clone(server),
            site,
            mapping,
            Duration::from_secs(config.sync_seconds.max(10)),
        ));
        Some(Self {
            task: SyncMutex::new(Some(task)),
        })
    }

    /// Stops the sync.
    pub fn stop(&self) {
        let task = self.task.lock().take();
        if let Some(task) = task {
            task.abort();
        }
    }
}

async fn run(server: Arc<Server>, site: SiteClient, mapping: RoleMapping, every: Duration) {
    // Players already synced since they joined.
    let mut synced: FxHashSet<Uuid> = FxHashSet::default();
    let mut reported_missing: FxHashSet<String> = FxHashSet::default();
    let mut last_full = Instant::now();

    loop {
        sleep(JOIN_CHECK).await;

        let online: Vec<Uuid> = server
            .get_players()
            .iter()
            .map(|player| player.gameprofile.id())
            .collect();
        synced.retain(|uuid| online.contains(uuid));

        let full = last_full.elapsed() >= every;
        if full {
            last_full = Instant::now();
        }
        let targets: Vec<Uuid> = online
            .into_iter()
            .filter(|uuid| full || !synced.contains(uuid))
            .collect();
        if targets.is_empty() {
            continue;
        }
        // Also when the site fails: the next try is the full sync, not every few seconds.
        synced.extend(&targets);

        match site.roles(&targets).await {
            Ok(players) => {
                for player in &players {
                    apply(&server, &mapping, player, &mut reported_missing).await;
                }
            }
            Err(error) => log::debug!("Role sync failed: {error}"),
        }
    }
}

async fn apply(
    server: &Arc<Server>,
    mapping: &RoleMapping,
    player: &PlayerRoles,
    reported_missing: &mut FxHashSet<String>,
) {
    for group in mapping.managed_groups() {
        if !server.permission_groups.contains_group(group) && reported_missing.insert(group.clone())
        {
            log::warn!("Role sync: group '{group}' is not in config/groups.toml, it is skipped");
        }
    }

    // Plain data, not the server: the update below must stay `Send` without the compiler
    // having to prove it through every field of `Server`.
    let existing: Vec<&String> = mapping
        .managed_groups()
        .iter()
        .filter(|group| server.permission_groups.contains_group(group))
        .collect();
    let known = |group: &str| existing.iter().any(|name| name.as_str() == group);
    let current = server
        .player_permission_state(player.uuid)
        .unwrap_or_default();
    if mapping
        .updated_groups(player, current.groups(), known)
        .is_none()
    {
        return;
    }

    // Computed again inside the update: the state may have changed since the look above.
    let result = server
        .try_update_player_permissions(player.uuid, |state| {
            let before = state.groups().to_vec();
            let groups = mapping
                .updated_groups(player, &before, known)
                .unwrap_or_else(|| before.clone());
            let (_, overrides, metadata) = state.into_parts();
            Ok::<_, Infallible>((
                PermissionSubjectState::new_with_metadata(groups, overrides, metadata),
                before,
            ))
        })
        .await;

    match result {
        Ok((state, before)) => log::info!(
            "Role sync: {} groups {:?} -> {:?}",
            player.uuid,
            before,
            state.groups()
        ),
        Err(error) => log::warn!("Role sync failed for {}: {error}", player.uuid),
    }
}
