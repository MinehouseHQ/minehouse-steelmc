//! Discord role → permission groups, and the `/api/mc/roles` answer it is applied to.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::site::{SiteClient, SiteError};

/// One Discord role of a player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Role {
    /// Discord's id of the role.
    pub id: String,
    /// Its name on the Discord server.
    pub name: String,
}

/// One player of `POST /api/mc/roles`. Players the site knows nothing about (no linked
/// account, Discord did not answer) are left out of the answer, so their groups stay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerRoles {
    /// The player.
    pub uuid: Uuid,
    /// `false` when the player left the Discord server: every synced group is removed.
    pub in_guild: bool,
    /// The roles the player has now.
    pub roles: Vec<Role>,
}

/// Parses `POST /api/mc/roles`.
pub fn parse_roles(text: &str) -> Result<Vec<PlayerRoles>, SiteError> {
    let json: Value = serde_json::from_str(text)
        .map_err(|error| SiteError::Answer(format!("invalid JSON: {error}")))?;
    let text_of = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let array = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };

    Ok(array(&json, "players")
        .iter()
        .filter_map(|player| {
            Some(PlayerRoles {
                uuid: Uuid::parse_str(player.get("uuid")?.as_str()?).ok()?,
                in_guild: player.get("in_guild").and_then(Value::as_bool) != Some(false),
                roles: array(player, "roles")
                    .iter()
                    .filter(|role| role.is_object())
                    .map(|role| Role {
                        id: text_of(role, "id"),
                        name: text_of(role, "name"),
                    })
                    .collect(),
            })
        })
        .collect())
}

impl SiteClient {
    /// Discord roles of these players.
    pub async fn roles(&self, players: &[Uuid]) -> Result<Vec<PlayerRoles>, SiteError> {
        let body = json!({ "uuids": players });
        parse_roles(&self.post_json("/api/mc/roles", body.to_string()).await?)
    }
}

/// Which groups follow which Discord roles. A key of the config is a role name
/// (case-insensitive) or a role ID; a value is one group or several separated by commas.
/// Only groups that appear here are managed: the rest are never touched.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoleMapping {
    by_role: BTreeMap<String, Vec<String>>,
    managed: Vec<String>,
}

impl RoleMapping {
    /// Builds the mapping from the `groups` table of the config.
    #[must_use]
    pub fn new(config: &BTreeMap<String, String>) -> Self {
        let mut mapping = Self::default();
        for (role, groups) in config {
            let role = key(role);
            if role.is_empty() {
                continue;
            }
            for group in groups.split(',').map(key).filter(|group| !group.is_empty()) {
                let mapped = mapping.by_role.entry(role.clone()).or_default();
                if !mapped.contains(&group) {
                    mapped.push(group.clone());
                }
                if !mapping.managed.contains(&group) {
                    mapping.managed.push(group);
                }
            }
        }
        mapping
    }

    /// Whether the sync has nothing to do.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.managed.is_empty()
    }

    /// Every group the sync adds and removes.
    #[must_use]
    pub fn managed_groups(&self) -> &[String] {
        &self.managed
    }

    /// Groups the player should be in, out of [`Self::managed_groups`].
    #[must_use]
    pub fn groups_for(&self, player: &PlayerRoles) -> Vec<String> {
        let mut groups = Vec::new();
        if !player.in_guild {
            return groups;
        }
        for role in &player.roles {
            for name in [&role.id, &role.name] {
                for group in self.by_role.get(&key(name)).into_iter().flatten() {
                    if !groups.contains(group) {
                        groups.push(group.clone());
                    }
                }
            }
        }
        groups
    }

    /// The player's groups after the sync: managed groups follow the roles, the rest stay.
    /// `None` when nothing changes. `known` tells which managed groups exist on the server.
    #[must_use]
    pub fn updated_groups(
        &self,
        player: &PlayerRoles,
        current: &[String],
        known: impl Fn(&str) -> bool,
    ) -> Option<Vec<String>> {
        let wanted = self.groups_for(player);
        let mut groups: Vec<String> = current
            .iter()
            .filter(|group| {
                let managed = self.managed.iter().any(|name| name == &key(group));
                !managed || !known(group) || wanted.contains(&key(group))
            })
            .cloned()
            .collect();
        for group in wanted {
            if known(&group) && !groups.iter().any(|current| key(current) == group) {
                groups.push(group);
            }
        }
        (groups != current).then_some(groups)
    }
}

fn key(name: &str) -> String {
    name.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping() -> RoleMapping {
        RoleMapping::new(
            &[
                ("ADMIN", "admin"),
                ("Модератор", "moderator, helper"),
                ("1565106687122083942", "player"),
                (" ", "ghost"),
            ]
            .into_iter()
            .map(|(role, groups)| (role.to_owned(), groups.to_owned()))
            .collect(),
        )
    }

    fn player(in_guild: bool, roles: &[(&str, &str)]) -> PlayerRoles {
        PlayerRoles {
            uuid: Uuid::nil(),
            in_guild,
            roles: roles
                .iter()
                .map(|(id, name)| Role {
                    id: (*id).to_owned(),
                    name: (*name).to_owned(),
                })
                .collect(),
        }
    }

    #[test]
    fn roles_match_by_name_or_id() {
        let mapping = mapping();
        assert_eq!(
            mapping.managed_groups(),
            ["player", "admin", "moderator", "helper"]
        );
        let groups = mapping.groups_for(&player(
            true,
            &[("1", "модератор"), ("1565106687122083942", "Игроки")],
        ));
        assert_eq!(groups, ["moderator", "helper", "player"]);
        assert_eq!(
            mapping.groups_for(&player(false, &[("1", "ADMIN")])),
            Vec::<String>::new()
        );
    }

    #[test]
    fn only_managed_groups_that_exist_change() {
        let mapping = mapping();
        let known = |group: &str| group != "helper";
        let current = ["op".to_owned(), "admin".to_owned()];

        // Lost ADMIN, got Модератор: `op` is not managed and stays, `helper` does not exist.
        assert_eq!(
            mapping.updated_groups(&player(true, &[("1", "Модератор")]), &current, known),
            Some(vec!["op".to_owned(), "moderator".to_owned()])
        );
        // Nothing to change.
        assert_eq!(
            mapping.updated_groups(&player(true, &[("2", "Admin")]), &current, known),
            None
        );
        // Left the Discord server.
        assert_eq!(
            mapping.updated_groups(&player(false, &[("2", "Admin")]), &current, known),
            Some(vec!["op".to_owned()])
        );
    }

    #[test]
    fn answers_skip_broken_players() {
        let players = parse_roles(
            r#"{"players":[{"uuid":"0f6f2d5c-0c0b-4d5e-9d0a-1b2c3d4e5f60","in_guild":false,"roles":[{"id":"1","name":"ADMIN"},7]},{"uuid":"x"},{"uuid":"00000000-0000-0000-0000-000000000000"}]}"#,
        );
        assert!(matches!(
            players,
            Ok(ref players)
                if players.len() == 2 && !players[0].in_guild && players[0].roles.len() == 1
                    && players[1].in_guild
        ));
        assert!(matches!(parse_roles("{}"), Ok(players) if players.is_empty()));
        assert!(parse_roles("oops").is_err());
    }
}
