//! The custom name and lock vanilla `BaseContainerBlockEntity` and
//! `BeaconBlockEntity` keep.

use glam::DVec3;
use simdnbt::borrow::NbtCompound as NbtCompoundView;
use simdnbt::owned::NbtCompound;
use simdnbt::{FromNbtTag as _, ToNbtTag as _};
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::vanilla_components::{CUSTOM_NAME, LOCK};
use steel_registry::data_components::{DataComponentMap, LockCode};
use steel_registry::sound_events;
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, translations};
use text_components::TextComponent;
use text_components::translation::Translation;

use crate::block_entity::{BlockEntity, ImplicitComponentGetter};
use crate::command::execution::registered_item_predicate_matches;
use crate::entity::Entity as _;
use crate::player::Player;

/// NBT key of the custom name.
const CUSTOM_NAME_TAG: &str = "CustomName";
/// NBT key of the lock, vanilla `LockCode.TAG_LOCK`.
const LOCK_TAG: &str = "lock";

/// A container's custom name and lock code.
pub struct ContainerName {
    name: SyncMutex<Option<TextComponent>>,
    lock: SyncMutex<LockCode>,
    default_name: &'static Translation<0>,
}

impl ContainerName {
    /// An unnamed, unlocked container whose name falls back to `default_name`.
    #[must_use]
    pub const fn new(default_name: &'static Translation<0>) -> Self {
        Self {
            name: SyncMutex::new(None),
            lock: SyncMutex::new(LockCode::NO_LOCK),
            default_name,
        }
    }

    /// The custom name, if one is set.
    #[must_use]
    pub fn custom_name(&self) -> Option<TextComponent> {
        self.name.lock().clone()
    }

    /// Sets or clears the custom name.
    pub fn set_custom_name(&self, name: Option<TextComponent>) {
        *self.name.lock() = name;
    }

    /// Vanilla `getDisplayName`: the custom name, or the container's default.
    #[must_use]
    pub fn display_name(&self) -> TextComponent {
        self.custom_name()
            .unwrap_or_else(|| TextComponent::translated(self.default_name.msg()))
    }

    /// The lock code; [`LockCode::NO_LOCK`] when unlocked.
    #[must_use]
    pub fn lock_code(&self) -> LockCode {
        self.lock.lock().clone()
    }

    /// Sets the lock code.
    pub fn set_lock_code(&self, lock: LockCode) {
        *self.lock.lock() = lock;
    }

    /// Vanilla `isLocked`.
    #[must_use]
    pub fn is_locked(&self) -> bool {
        *self.lock.lock() != LockCode::NO_LOCK
    }

    /// Vanilla `LockCode.canUnlock`: spectators, and players holding a
    /// matching item in the main hand.
    #[must_use]
    pub fn can_unlock(&self, player: &Player) -> bool {
        if player.is_spectator() {
            return true;
        }
        let lock = self.lock.lock().clone();
        if lock == LockCode::NO_LOCK {
            return true;
        }
        let held = player
            .inventory
            .lock()
            .get_item_in_hand(InteractionHand::MainHand)
            .clone();
        registered_item_predicate_matches(lock.predicate(), &held)
    }

    /// Vanilla `loadAdditional`: reads `lock` and `CustomName`.
    pub fn load(&self, nbt: NbtCompoundView<'_, '_>) {
        *self.lock.lock() = nbt
            .get(LOCK_TAG)
            .and_then(LockCode::from_nbt_tag)
            .unwrap_or(LockCode::NO_LOCK);
        *self.name.lock() = nbt
            .get(CUSTOM_NAME_TAG)
            .and_then(|tag| TextComponent::from_nbt(&tag.to_owned()));
    }

    /// Vanilla `saveAdditional`: writes `lock` unless unlocked, and
    /// `CustomName` when set.
    pub fn save(&self, nbt: &mut NbtCompound) {
        let lock = self.lock.lock().clone();
        if lock != LockCode::NO_LOCK {
            nbt.insert(LOCK_TAG, lock.to_nbt_tag());
        }
        if let Some(name) = self.name.lock().as_ref() {
            nbt.insert(CUSTOM_NAME_TAG, name.to_codec_nbt());
        }
    }

    /// Vanilla `applyImplicitComponents`: takes `custom_name` and `lock`.
    pub fn apply_implicit_components(&self, components: &mut ImplicitComponentGetter<'_>) {
        *self.name.lock() = components.get(CUSTOM_NAME);
        *self.lock.lock() = components.get_or_default(LOCK, LockCode::NO_LOCK);
    }

    /// Vanilla `collectImplicitComponents`: `custom_name`, and `lock` when
    /// locked.
    pub fn collect_implicit_components(&self, components: &mut DataComponentMap) {
        components.set(CUSTOM_NAME, self.custom_name());
        if self.is_locked() {
            components.set(LOCK, Some(self.lock_code()));
        }
    }

    /// Strips the keys [`Self::load`] reads from `nbt`, for block entities
    /// that keep the rest of their saved data as loaded.
    pub fn remove_tags(nbt: &mut NbtCompound) {
        while nbt.remove(CUSTOM_NAME_TAG).is_some() {}
        while nbt.remove(LOCK_TAG).is_some() {}
    }
}

/// Vanilla `Vec3.atCenterOf`: the center of the block at `pos`.
#[must_use]
pub fn block_center(pos: BlockPos) -> DVec3 {
    DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + 0.5,
        f64::from(pos.z()) + 0.5,
    )
}

/// Vanilla `BaseContainerBlockEntity.sendChestLockedNotifications`: tells
/// `player` the container is locked and plays the locked chest sound at
/// `pos`.
pub fn send_chest_locked_notifications(pos: DVec3, player: &Player, display_name: TextComponent) {
    let message = translations::CONTAINER_IS_LOCKED
        .message([display_name])
        .component();
    player.send_overlay_message(&message);
    player.get_world().play_sound_at(
        &sound_events::BLOCK_CHEST_LOCKED,
        SoundSource::Blocks,
        pos,
        1.0,
        1.0,
        None,
    );
}

/// Vanilla `canOpen` followed by the locked notification, as menu providers
/// check before opening: whether every container in `names` unlocks for
/// `player`. When one does not, `player` is told `display_name` is locked,
/// with the sound at `pos`; spectators always unlock.
pub fn check_container_locks(
    names: &[&ContainerName],
    player: &Player,
    pos: DVec3,
    display_name: &TextComponent,
) -> bool {
    if names.iter().all(|name| name.can_unlock(player)) {
        return true;
    }
    send_chest_locked_notifications(pos, player, display_name.clone());
    false
}

/// [`check_container_locks`] for the block entity at `pos`: its display name
/// for the menu title, or `None` when it is locked for `player`. Block
/// entities without a [`ContainerName`] open under `default_name`.
pub fn unlocked_menu_title(
    block_entity: &dyn BlockEntity,
    player: &Player,
    default_name: &'static Translation<0>,
) -> Option<TextComponent> {
    let Some(name) = block_entity.container_name() else {
        return Some(TextComponent::translated(default_name.msg()));
    };
    let title = name.display_name();
    check_container_locks(
        &[name],
        player,
        block_center(block_entity.get_block_pos()),
        &title,
    )
    .then_some(title)
}
