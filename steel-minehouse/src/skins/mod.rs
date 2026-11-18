//! Skins for nicknames without a license, like the Fabric `SkinRestorer` on prod.
//!
//! On a join the player gets the skin they chose (`/skin`), looked up again by its nickname
//! when it came from Mojang or Ely.by. A player who never chose one and joined without
//! textures gets the skin of the licensed account with the same nickname, or else the one
//! from Ely.by; neither means Steve or Alex. The skins live in `skins/` of the data
//! directory ([`store`]); `/skin` changes them for everyone at once ([`commands`]).

mod commands;
pub(crate) mod providers;
pub(crate) mod store;

use std::{sync::Arc, time::Duration};

use steel_core::{command::CommandRegistry, player::GameProfile};
use steel_registry::game_profile::ProfileProperty;
use tokio::{runtime::Handle, task::JoinHandle};
use uuid::Uuid;

use store::{ELY_BY, MOJANG, SkinStore, StoredSkin, Textures};

/// How long a join waits for the skin once the login is allowed. A slower lookup still
/// saves what it found, for the next join.
pub(crate) const JOIN_WAIT: Duration = Duration::from_secs(5);

/// The skins module.
pub struct Skins {
    runtime: Handle,
    store: SkinStore,
}

impl Skins {
    /// Adds `/skin`. Call inside the Tokio runtime, before the server is built.
    #[must_use]
    pub fn register(registry: &mut CommandRegistry) -> Option<Arc<Self>> {
        let Ok(runtime) = Handle::try_current() else {
            log::error!("Skins are off: no Tokio runtime");
            return None;
        };
        let skins = Arc::new(Self {
            runtime,
            store: SkinStore::open(),
        });
        if let Err(error) = commands::register(registry, &skins) {
            log::error!("Skins: /skin is off: {error}");
        }
        Some(skins)
    }

    /// The profile a login joins with, or `None` to join with `profile` as it is. Runs the
    /// lookup on the runtime, so dropping the handle does not cancel the save.
    pub(crate) fn lookup_for_join(
        self: &Arc<Self>,
        profile: GameProfile,
    ) -> JoinHandle<Option<GameProfile>> {
        let skins = Arc::clone(self);
        self.runtime.spawn(async move {
            let has_textures = textures_of(&profile).is_some();
            let shown = skins
                .skin_at_join(profile.id(), profile.name(), has_textures)
                .await?;
            with_textures(&profile, shown.as_ref())
        })
    }

    /// The textures to show on a join: `None` keeps those of the login.
    async fn skin_at_join(
        &self,
        uuid: Uuid,
        name: &str,
        has_login_textures: bool,
    ) -> Option<Option<Textures>> {
        match self.store.load(uuid) {
            Some(stored) if stored.refreshes() => {
                let argument = stored.argument.clone().unwrap_or_default();
                match fetch(&stored.provider, &argument).await {
                    Ok(Some(fresh)) => {
                        if stored.value.as_ref() != Some(&fresh) {
                            let mut updated = stored;
                            updated.value = Some(fresh.clone());
                            self.store.save(uuid, &updated);
                        }
                        Some(Some(fresh))
                    }
                    Ok(None) | Err(_) => Some(stored.value),
                }
            }
            Some(stored) => Some(stored.value),
            None if has_login_textures => None,
            None => {
                let found = auto_fetch(name).await?;
                self.store.save(uuid, &found);
                Some(found.value)
            }
        }
    }
}

/// The skin of nickname `name` from `provider` (`mojang` or `ely.by`).
async fn fetch(provider: &str, name: &str) -> Result<Option<Textures>, providers::Failure> {
    match provider {
        MOJANG => providers::mojang(name).await,
        ELY_BY => providers::ely_by(name).await,
        other => Err(format!("неизвестный источник скинов {other}")),
    }
}

/// The skin of the licensed account `name`, or else of the Ely.by one, as `SkinRestorer`'s
/// `autoFetch` with the providers `mojang` and `ely.by`.
async fn auto_fetch(name: &str) -> Option<StoredSkin> {
    for provider in [MOJANG, ELY_BY] {
        match fetch(provider, name).await {
            Ok(Some(textures)) => {
                return Some(StoredSkin::new(
                    provider,
                    Some(name.to_owned()),
                    None,
                    Some(textures),
                ));
            }
            Ok(None) => {}
            Err(error) => log::warn!("Skins: no skin of {name} from {provider}: {error}"),
        }
    }
    None
}

/// The `textures` property of `profile`.
fn textures_of(profile: &GameProfile) -> Option<&ProfileProperty> {
    profile
        .properties()
        .iter()
        .find(|property| property.name() == "textures")
}

/// The properties of `profile` with `textures` in place of its own; `None` removes them.
pub(crate) fn properties_with(
    profile: &GameProfile,
    textures: Option<&Textures>,
) -> Vec<ProfileProperty> {
    let mut properties: Vec<ProfileProperty> = profile
        .properties()
        .iter()
        .filter(|property| property.name() != "textures")
        .cloned()
        .collect();
    properties.extend(textures.and_then(Textures::property));
    properties
}

fn with_textures(profile: &GameProfile, textures: Option<&Textures>) -> Option<GameProfile> {
    GameProfile::new(
        profile.id(),
        profile.name().to_owned(),
        properties_with(profile, textures),
    )
    .inspect_err(|error| {
        log::warn!(
            "Skins: the skin of {} does not fit: {error}",
            profile.name()
        );
    })
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textures_replace_only_textures() {
        let other =
            ProfileProperty::new("other".to_owned(), "x".to_owned(), None).expect("property");
        let old =
            ProfileProperty::new("textures".to_owned(), "old".to_owned(), None).expect("property");
        let profile = GameProfile::new(Uuid::from_u128(1), "Timur".to_owned(), vec![other, old])
            .expect("profile");
        let new = Textures {
            name: "textures".to_owned(),
            value: "new".to_owned(),
            signature: Some("sig".to_owned()),
        };
        let replaced = with_textures(&profile, Some(&new)).expect("profile");
        let values: Vec<&str> = replaced
            .properties()
            .iter()
            .map(ProfileProperty::value)
            .collect();
        assert_eq!(values, ["x", "new"]);
        let cleared = with_textures(&profile, None).expect("profile");
        assert!(textures_of(&cleared).is_none());
        assert_eq!(cleared.properties().len(), 1);
    }
}
