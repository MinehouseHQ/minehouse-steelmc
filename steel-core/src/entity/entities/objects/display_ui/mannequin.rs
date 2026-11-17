//! Vanilla `Mannequin`: a player-shaped living entity that wears a profile's skin.

use std::sync::Weak;
use std::sync::atomic::{AtomicBool, Ordering};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use simdnbt::{FromNbtTag as _, ToNbtTag as _};
use steel_macros::entity_behavior;
use steel_registry::data_components::vanilla_components::PROFILE;
use steel_registry::entity_data::{EntityPose, HumanoidArm};
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::resolvable_profile::ResolvableProfile;
use steel_registry::vanilla_entity_data::MannequinEntityData;
use steel_utils::locks::SyncMutex;
use steel_utils::{DowncastType, DowncastTypeKey, translations};
use text_components::TextComponent;

use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntitySyncedData, LivingEntity, LivingEntityBase,
    sync_dirty_mob_effects,
};
use crate::player::Player;
use crate::world::World;

/// Vanilla `PlayerModelPart`: serialized name and bit of each skin layer.
const MODEL_PARTS: [(&str, i8); 7] = [
    ("cape", 1),
    ("jacket", 1 << 1),
    ("left_sleeve", 1 << 2),
    ("right_sleeve", 1 << 3),
    ("left_pants_leg", 1 << 4),
    ("right_pants_leg", 1 << 5),
    ("hat", 1 << 6),
];
/// Vanilla `Mannequin.ALL_LAYERS`: every skin layer shown.
const ALL_LAYERS: i8 = 0x7F;

/// Vanilla `Mannequin.DEFAULT_DESCRIPTION`.
fn default_description() -> TextComponent {
    TextComponent::from(&translations::ENTITY_MINECRAFT_MANNEQUIN_LABEL)
}

/// Vanilla `Mannequin.LAYERS_CODEC` decode: the listed parts are hidden.
/// Any unknown part name fails the whole list, as the codec does.
fn hidden_layers_mask(names: &[&str]) -> Option<i8> {
    names.iter().try_fold(ALL_LAYERS, |mask, name| {
        MODEL_PARTS
            .iter()
            .find(|(part, _)| part == name)
            .map(|(_, bit)| mask & !bit)
    })
}

/// Vanilla `Mannequin.LAYERS_CODEC` encode: the parts whose bit is clear.
fn hidden_layer_names(mask: i8) -> Vec<String> {
    MODEL_PARTS
        .iter()
        .filter(|(_, bit)| mask & bit == 0)
        .map(|(part, _)| (*part).to_owned())
        .collect()
}

/// Vanilla `Mannequin.POSE_CODEC`: the serialized names of the allowed poses.
const fn pose_name(pose: EntityPose) -> Option<&'static str> {
    match pose {
        EntityPose::Standing => Some("standing"),
        EntityPose::Sneaking => Some("crouching"),
        EntityPose::Swimming => Some("swimming"),
        EntityPose::FallFlying => Some("fall_flying"),
        EntityPose::Sleeping => Some("sleeping"),
        _ => None,
    }
}

/// Vanilla `Mannequin.POSE_CODEC` decode, rejecting poses outside `VALID_POSES`.
fn pose_from_name(name: &str) -> Option<EntityPose> {
    match name {
        "standing" => Some(EntityPose::Standing),
        "crouching" => Some(EntityPose::Sneaking),
        "swimming" => Some(EntityPose::Swimming),
        "fall_flying" => Some(EntityPose::FallFlying),
        "sleeping" => Some(EntityPose::Sleeping),
        _ => None,
    }
}

/// Vanilla `HumanoidArm.CODEC` name.
const fn arm_name(arm: HumanoidArm) -> &'static str {
    match arm {
        HumanoidArm::Left => "left",
        HumanoidArm::Right => "right",
    }
}

/// Vanilla `HumanoidArm.CODEC` decode.
fn arm_from_name(name: &str) -> Option<HumanoidArm> {
    match name {
        "left" => Some(HumanoidArm::Left),
        "right" => Some(HumanoidArm::Right),
        _ => None,
    }
}

/// Vanilla `Mannequin`.
#[entity_behavior(class = "Mannequin")]
pub struct MannequinEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    entity_data: SyncMutex<MannequinEntityData>,
    /// Vanilla `Mannequin.description`: kept while the label is hidden.
    description: SyncMutex<TextComponent>,
    /// Vanilla `Mannequin.hideDescription`.
    hide_description: AtomicBool,
}

// SAFETY: This key is owned by Steel and uniquely identifies `MannequinEntity`.
unsafe impl DowncastType for MannequinEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/mannequin");
}

impl MannequinEntity {
    /// Creates a new mannequin.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved mannequin.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mut entity_data = MannequinEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        // Vanilla's constructor shows every skin layer, and `defineSynchedData`
        // labels the mannequin with the translatable "NPC" description.
        entity_data.avatar.player_mode_customization.set(ALL_LAYERS);
        entity_data
            .description
            .set(Some(Box::new(default_description())));
        Self {
            base,
            entity_type,
            living_base,
            entity_data: SyncMutex::new(entity_data),
            description: SyncMutex::new(default_description()),
            hide_description: AtomicBool::new(false),
        }
    }

    /// Vanilla `getProfile`.
    #[must_use]
    pub fn profile(&self) -> ResolvableProfile {
        self.entity_data.lock().profile.get().clone()
    }

    /// Vanilla `setProfile`.
    pub fn set_profile(&self, profile: ResolvableProfile) {
        self.entity_data.lock().profile.set(profile);
    }

    /// Vanilla `getImmovable`.
    #[must_use]
    pub fn is_immovable(&self) -> bool {
        *self.entity_data.lock().immovable.get()
    }

    /// Vanilla `setImmovable`.
    pub fn set_immovable(&self, immovable: bool) {
        self.entity_data.lock().immovable.set(immovable);
    }

    /// Vanilla `Avatar.getMainArm`.
    #[must_use]
    pub fn main_arm(&self) -> HumanoidArm {
        *self.entity_data.lock().avatar.player_main_hand.get()
    }

    /// Vanilla `Avatar.setMainArm`.
    pub fn set_main_arm(&self, arm: HumanoidArm) {
        self.entity_data.lock().avatar.player_main_hand.set(arm);
    }

    /// The `DATA_PLAYER_MODE_CUSTOMISATION` bits: a set bit shows that layer.
    #[must_use]
    pub fn shown_layers(&self) -> i8 {
        *self
            .entity_data
            .lock()
            .avatar
            .player_mode_customization
            .get()
    }

    fn set_shown_layers(&self, layers: i8) {
        self.entity_data
            .lock()
            .avatar
            .player_mode_customization
            .set(layers);
    }

    /// Vanilla `getDescription`: `None` while the label is hidden.
    #[must_use]
    pub fn description(&self) -> Option<TextComponent> {
        self.entity_data
            .lock()
            .description
            .get()
            .as_deref()
            .cloned()
    }

    /// Vanilla `setDescription`.
    pub fn set_description(&self, description: TextComponent) {
        *self.description.lock() = description;
        self.update_description();
    }

    /// Vanilla `setHideDescription`.
    pub fn set_hide_description(&self, hide_description: bool) {
        self.hide_description
            .store(hide_description, Ordering::Relaxed);
        self.update_description();
    }

    /// Vanilla `updateDescription`.
    fn update_description(&self) {
        let shown = if self.hide_description.load(Ordering::Relaxed) {
            None
        } else {
            Some(Box::new(self.description.lock().clone()))
        };
        self.entity_data.lock().description.set(shown);
    }
}

impl Entity for MannequinEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        self.base_tick_living_entity();
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn update_data_before_sync(&self) {
        if let Some(display) = sync_dirty_mob_effects(&self.living_base, &self.entity_data) {
            self.entity_data
                .set_base_glowing_flag(self.has_glowing_tag() || display.glowing);
        }
    }

    /// Vanilla `Avatar.getDefaultDimensions` scaled as `LivingEntity.getDimensions` does.
    fn dimensions_for_pose(&self, pose: EntityPose) -> EntityDimensions {
        let dimensions = Player::dimensions_for_pose(pose);
        if pose == EntityPose::Sleeping {
            dimensions
        } else {
            dimensions.scale(LivingEntity::get_scale(self))
        }
    }

    /// Vanilla `Mannequin.isEffectiveAi`.
    fn is_effective_ai(&self) -> bool {
        !self.is_immovable() && self.is_server_driven_movement()
    }

    /// Vanilla `Mannequin.applyImplicitComponents`.
    fn apply_implicit_item_components(&self, item_stack: &ItemStack) {
        if let Some(profile) = item_stack.get(PROFILE) {
            self.set_profile(profile.clone());
        }
    }

    /// Vanilla `Mannequin.addAdditionalSaveData`.
    fn save_additional(&self, nbt: &mut NbtCompound) {
        nbt.insert("profile", self.profile().to_nbt_tag());
        nbt.insert(
            "hidden_layers",
            NbtTag::List(NbtList::from(hidden_layer_names(self.shown_layers()))),
        );
        nbt.insert("main_hand", arm_name(self.main_arm()));
        if let Some(pose) = pose_name(self.pose()) {
            nbt.insert("pose", pose);
        }
        nbt.insert("immovable", i8::from(self.is_immovable()));
        match self.description() {
            Some(description) => {
                if description != default_description()
                    && let Some(tag) = description.to_optional_nbt_tag()
                {
                    nbt.insert("description", tag);
                }
            }
            None => nbt.insert("hide_description", 1_i8),
        }
    }

    /// Vanilla `Mannequin.readAdditionalSaveData`.
    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        if let Some(profile) = ResolvableProfile::from_optional_nbt_tag(nbt.get("profile"))
            .ok()
            .flatten()
        {
            self.set_profile(profile);
        }
        let layers = nbt
            .list("hidden_layers")
            .and_then(|list| {
                let names = match list.strings() {
                    Some(names) => names.iter().map(|name| name.to_str()).collect(),
                    None if list.empty() => Vec::new(),
                    None => return None,
                };
                let names: Vec<&str> = names.iter().map(AsRef::as_ref).collect();
                hidden_layers_mask(&names)
            })
            .unwrap_or(ALL_LAYERS);
        self.set_shown_layers(layers);
        self.set_main_arm(
            nbt.string("main_hand")
                .and_then(|name| arm_from_name(&name.to_str()))
                .unwrap_or_default(),
        );
        self.set_pose(
            nbt.string("pose")
                .and_then(|name| pose_from_name(&name.to_str()))
                .unwrap_or(EntityPose::Standing),
        );
        self.set_immovable(nbt.byte("immovable").is_some_and(|value| value != 0));
        self.set_hide_description(nbt.byte("hide_description").is_some_and(|value| value != 0));
        self.set_description(
            TextComponent::from_optional_nbt_tag(nbt.get("description"))
                .ok()
                .flatten()
                .unwrap_or_else(default_description),
        );
    }
}

impl LivingEntity for MannequinEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn get_health(&self) -> f32 {
        *self.entity_data.lock().living_entity().health.get()
    }

    fn set_health(&self, health: f32) {
        let max_health = self.get_max_health();
        self.entity_data
            .lock()
            .living_entity_mut()
            .health
            .set(health.clamp(0.0, max_health));
    }

    /// Vanilla `Mannequin.isImmobile`.
    fn is_immobile(&self) -> bool {
        self.is_immovable() || self.default_is_immobile()
    }
}

#[cfg(test)]
mod tests;
