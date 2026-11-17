//! Vanilla `JigsawBlockEntity`.

use std::str::FromStr as _;
use std::sync::{Arc, Weak};

use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, FrontAndTop};
use steel_registry::template_pool::JointType;
use steel_registry::vanilla_block_entity_types;
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;
use crate::worldgen::structure::generate_jigsaw;

/// Vanilla `JigsawBlockEntity.DEFAULT_FINAL_STATE`.
pub const DEFAULT_FINAL_STATE: &str = "minecraft:air";

/// Vanilla `JigsawBlockEntity.EMPTY_ID`, also the key of `Pools.EMPTY`.
const fn empty_id() -> Identifier {
    Identifier::vanilla_static("empty")
}

/// What a jigsaw block connects to and what it turns into once placed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JigsawSettings {
    /// This connector's name.
    pub name: Identifier,
    /// The connector name it attaches to.
    pub target: Identifier,
    /// The template pool the next piece comes from.
    pub pool: Identifier,
    /// The block state the jigsaw becomes, as a state string.
    pub final_state: String,
    /// Whether an attached piece may roll around the connection.
    pub joint: JointType,
    /// Vanilla `placement_priority`.
    pub placement_priority: i32,
    /// Vanilla `selection_priority`.
    pub selection_priority: i32,
}

impl JigsawSettings {
    /// A fresh jigsaw block's settings, with the joint vanilla picks for `state`.
    #[must_use]
    pub fn new(state: BlockStateId) -> Self {
        Self {
            name: empty_id(),
            target: empty_id(),
            pool: empty_id(),
            final_state: DEFAULT_FINAL_STATE.to_owned(),
            joint: default_joint(state),
            placement_priority: 0,
            selection_priority: 0,
        }
    }
}

/// Vanilla `StructureTemplate.getDefaultJointType`: a jigsaw facing sideways
/// is aligned, one facing up or down rolls.
#[must_use]
pub fn default_joint(state: BlockStateId) -> JointType {
    let front = jigsaw_orientation(state).front();
    if front.is_horizontal() {
        JointType::Aligned
    } else {
        JointType::Rollable
    }
}

fn jigsaw_orientation(state: BlockStateId) -> FrontAndTop {
    state
        .try_get_value(&BlockStateProperties::ORIENTATION)
        .unwrap_or(FrontAndTop::NorthUp)
}

/// Vanilla `JigsawBlockEntity`.
pub struct JigsawBlockEntity {
    base: BlockEntityBase,
    settings: SyncMutex<JigsawSettings>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `JigsawBlockEntity`.
unsafe impl DowncastType for JigsawBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/jigsaw");
}

impl JigsawBlockEntity {
    /// Creates a jigsaw block entity with vanilla's defaults.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(&vanilla_block_entity_types::JIGSAW, level, pos, state),
            settings: SyncMutex::new(JigsawSettings::new(state)),
        }
    }

    /// The jigsaw's current settings.
    #[must_use]
    pub fn settings(&self) -> JigsawSettings {
        self.settings.lock().clone()
    }

    /// Replaces every setting, as the jigsaw screen does, and marks the entity changed.
    pub fn set_settings(&self, settings: JigsawSettings) {
        *self.settings.lock() = settings;
        self.set_changed();
    }

    /// Vanilla `generate`: grows a structure from the pool in front of the
    /// jigsaw, `levels` pieces deep. Returns whether it placed anything.
    pub fn generate(&self, world: &Arc<World>, levels: i32, keep_jigsaws: bool) -> bool {
        let front = jigsaw_orientation(self.get_block_state()).front();
        let position = self.get_block_pos().relative(front);
        let settings = self.settings();
        generate_jigsaw(
            world,
            &settings.pool,
            &settings.target,
            levels,
            position,
            keep_jigsaws,
        )
    }
}

impl BlockEntity for JigsawBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt: NbtCompoundView<'_, '_> = nbt.into();
        let identifier = |key: &str| {
            nbt.string(key)
                .and_then(|value| Identifier::from_str(&value.to_str()).ok())
                .unwrap_or_else(empty_id)
        };
        let joint = nbt
            .string("joint")
            .and_then(|value| JointType::by_name(&value.to_str()))
            .unwrap_or_else(|| default_joint(self.get_block_state()));
        *self.settings.lock() = JigsawSettings {
            name: identifier("name"),
            target: identifier("target"),
            pool: identifier("pool"),
            final_state: nbt
                .string("final_state")
                .map_or_else(|| DEFAULT_FINAL_STATE.to_owned(), ToString::to_string),
            joint,
            placement_priority: nbt.int("placement_priority").unwrap_or(0),
            selection_priority: nbt.int("selection_priority").unwrap_or(0),
        };
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let settings = self.settings();
        nbt.insert("name", settings.name.to_string());
        nbt.insert("target", settings.target.to_string());
        nbt.insert("pool", settings.pool.to_string());
        nbt.insert("final_state", settings.final_state);
        nbt.insert("joint", settings.joint.serialized_name());
        nbt.insert("placement_priority", settings.placement_priority);
        nbt.insert("selection_priority", settings.selection_priority);
    }
}
