//! Vanilla `StructureBlockEntity`: saves the blocks around it as a template,
//! loads templates back, and marks the corners of a template's box.

use std::ptr;
use std::str::FromStr as _;
use std::sync::{Arc, Weak};

use glam::IVec3;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, StructureMode};
use steel_registry::{vanilla_block_entity_types, vanilla_blocks};
use steel_utils::locks::SyncMutex;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier, Rotation};
use steel_worldgen::structure::StructureMirror;

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;
use crate::worldgen::template::live::{LivePlacement, place_live, template_seed};
use crate::worldgen::template::{StoredTemplate, fill_from_world};

/// Vanilla `MAX_OFFSET_PER_AXIS`.
pub const MAX_OFFSET_PER_AXIS: i32 = 48;
/// Vanilla `MAX_SIZE_PER_AXIS`.
pub const MAX_SIZE_PER_AXIS: i32 = 48;
/// Vanilla `SCAN_CORNER_BLOCKS_RANGE`.
const SCAN_CORNER_BLOCKS_RANGE: i32 = 80;
/// Vanilla `DEFAULT_POS`.
const DEFAULT_POS: IVec3 = IVec3::new(0, 1, 0);

/// Everything a structure block's screen edits, vanilla's
/// `StructureBlockEntity` fields.
#[derive(Clone, Debug, PartialEq)]
pub struct StructureBlockSettings {
    /// Vanilla `structureName`.
    pub name: Option<Identifier>,
    /// Vanilla `author`.
    pub author: String,
    /// Vanilla `metaData`, a data block's marker text.
    pub metadata: String,
    /// Vanilla `structurePos`: the box's corner, relative to the block.
    pub pos: IVec3,
    /// Vanilla `structureSize`.
    pub size: IVec3,
    /// Vanilla `mirror`.
    pub mirror: StructureMirror,
    /// Vanilla `rotation`.
    pub rotation: Rotation,
    /// Vanilla `mode`.
    pub mode: StructureMode,
    /// Vanilla `ignoreEntities`.
    pub ignore_entities: bool,
    /// Vanilla `strict`.
    pub strict: bool,
    /// Vanilla `powered`.
    pub powered: bool,
    /// Vanilla `showAir`.
    pub show_air: bool,
    /// Vanilla `showBoundingBox`.
    pub show_bounding_box: bool,
    /// Vanilla `integrity`.
    pub integrity: f32,
    /// Vanilla `seed`.
    pub seed: i64,
}

impl StructureBlockSettings {
    /// Vanilla's defaults for a block in `mode`.
    #[must_use]
    pub const fn new(mode: StructureMode) -> Self {
        Self {
            name: None,
            author: String::new(),
            metadata: String::new(),
            pos: DEFAULT_POS,
            size: IVec3::ZERO,
            mirror: StructureMirror::None,
            rotation: Rotation::None,
            mode,
            ignore_entities: true,
            strict: false,
            powered: false,
            show_air: false,
            show_bounding_box: true,
            integrity: 1.0,
            seed: 0,
        }
    }

    /// Vanilla `getStructureName`.
    #[must_use]
    pub fn name_string(&self) -> String {
        self.name
            .as_ref()
            .map_or_else(String::new, ToString::to_string)
    }
}

/// Vanilla `setStructureName(String)`: an empty or invalid name is no name.
#[must_use]
pub fn parse_structure_name(name: &str) -> Option<Identifier> {
    if name.is_empty() {
        return None;
    }
    Identifier::from_str(name).ok()
}

/// Vanilla `Mirror.LEGACY_CODEC` names.
const fn mirror_name(mirror: StructureMirror) -> &'static str {
    match mirror {
        StructureMirror::None => "NONE",
        StructureMirror::LeftRight => "LEFT_RIGHT",
        StructureMirror::FrontBack => "FRONT_BACK",
    }
}

fn mirror_by_name(name: &str) -> Option<StructureMirror> {
    [
        StructureMirror::None,
        StructureMirror::LeftRight,
        StructureMirror::FrontBack,
    ]
    .into_iter()
    .find(|mirror| mirror_name(*mirror) == name)
}

/// Vanilla `Rotation.LEGACY_CODEC` names.
const fn rotation_name(rotation: Rotation) -> &'static str {
    match rotation {
        Rotation::None => "NONE",
        Rotation::Clockwise90 => "CLOCKWISE_90",
        Rotation::Clockwise180 => "CLOCKWISE_180",
        Rotation::CounterClockwise90 => "COUNTERCLOCKWISE_90",
    }
}

fn rotation_by_name(name: &str) -> Option<Rotation> {
    [
        Rotation::None,
        Rotation::Clockwise90,
        Rotation::Clockwise180,
        Rotation::CounterClockwise90,
    ]
    .into_iter()
    .find(|rotation| rotation_name(*rotation) == name)
}

/// Vanilla `StructureMode.LEGACY_CODEC` names.
const fn mode_name(mode: StructureMode) -> &'static str {
    match mode {
        StructureMode::Save => "SAVE",
        StructureMode::Load => "LOAD",
        StructureMode::Corner => "CORNER",
        StructureMode::Data => "DATA",
    }
}

fn mode_by_name(name: &str) -> Option<StructureMode> {
    [
        StructureMode::Save,
        StructureMode::Load,
        StructureMode::Corner,
        StructureMode::Data,
    ]
    .into_iter()
    .find(|mode| mode_name(*mode) == name)
}

fn block_mode(state: BlockStateId) -> StructureMode {
    state
        .try_get_value(&BlockStateProperties::STRUCTUREBLOCK_MODE)
        .unwrap_or(StructureMode::Load)
}

/// Vanilla `StructureBlockEntity`.
pub struct StructureBlockEntity {
    base: BlockEntityBase,
    settings: SyncMutex<StructureBlockSettings>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `StructureBlockEntity`.
unsafe impl DowncastType for StructureBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/structure_block");
}

impl StructureBlockEntity {
    /// Vanilla `StructureBlockEntity(pos, state)`: the mode comes from the state.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::STRUCTURE_BLOCK,
                level,
                pos,
                state,
            ),
            settings: SyncMutex::new(StructureBlockSettings::new(block_mode(state))),
        }
    }

    /// The block's current settings.
    #[must_use]
    pub fn settings(&self) -> StructureBlockSettings {
        self.settings.lock().clone()
    }

    /// Changes the settings with `update`.
    pub fn update_settings(&self, update: impl FnOnce(&mut StructureBlockSettings)) {
        update(&mut self.settings.lock());
    }

    /// Vanilla `createdBy`: the placer becomes the author.
    pub fn created_by(&self, author: String) {
        self.settings.lock().author = author;
    }

    /// Vanilla `isPowered`.
    #[must_use]
    pub fn is_powered(&self) -> bool {
        self.settings.lock().powered
    }

    /// Vanilla `setPowered`.
    pub fn set_powered(&self, powered: bool) {
        self.settings.lock().powered = powered;
    }

    /// Vanilla `setMode`: also turns the block to show the mode.
    pub fn set_mode(&self, world: &Arc<World>, mode: StructureMode) {
        self.settings.lock().mode = mode;
        self.update_block_state(world, mode);
    }

    /// Vanilla `updateBlockState`.
    fn update_block_state(&self, world: &Arc<World>, mode: StructureMode) {
        let pos = self.get_block_pos();
        let state = world.get_block_state(pos);
        if state.get_block() == &vanilla_blocks::STRUCTURE_BLOCK && block_mode(state) != mode {
            world.set_block(
                pos,
                state.set_value(&BlockStateProperties::STRUCTUREBLOCK_MODE, mode),
                UpdateFlags::UPDATE_CLIENTS,
            );
        }
    }

    /// Vanilla `saveStructure()`: a save block saves its box to disk.
    pub fn save_structure_to_disk(&self, world: &World) -> bool {
        self.settings.lock().mode == StructureMode::Save && self.save_structure(world, true)
    }

    /// Vanilla `saveStructure(saveToDisk)`: captures the box into the
    /// template named by the block, and writes it out when `save_to_disk`.
    pub fn save_structure(&self, world: &World, save_to_disk: bool) -> bool {
        let settings = self.settings();
        let Some(name) = settings.name else {
            return false;
        };
        let corner = self
            .get_block_pos()
            .offset(settings.pos.x, settings.pos.y, settings.pos.z);
        let manager = world.structure_templates();
        let captured = fill_from_world(
            world,
            corner,
            settings.size,
            !settings.ignore_entities,
            &[&vanilla_blocks::STRUCTURE_VOID],
        );
        manager.update(&name, |template| {
            if let Some(nbt) = captured {
                template.nbt = nbt;
            }
            template.author = settings.author;
        });
        !save_to_disk || manager.save(&name)
    }

    /// Vanilla `isStructureLoadable`.
    #[must_use]
    pub fn is_structure_loadable(&self, world: &World) -> bool {
        let settings = self.settings.lock();
        settings.mode == StructureMode::Load
            && settings
                .name
                .as_ref()
                .is_some_and(|name| world.structure_templates().get(name).is_some())
    }

    /// Vanilla `placeStructureIfSameSize`: loads the template when the box
    /// already has its size, or else takes on its size so the next load fits.
    pub fn place_structure_if_same_size(&self, world: &Arc<World>) -> bool {
        let settings = self.settings();
        let Some(name) = settings
            .name
            .filter(|_| settings.mode == StructureMode::Load)
        else {
            return false;
        };
        let Some(template) = world.structure_templates().get(&name) else {
            return false;
        };
        let size = IVec3::from_array(template.size());
        if size == settings.size {
            self.place_template(world, &name, &template);
            true
        } else {
            self.load_structure_info(&template.author, size);
            false
        }
    }

    /// Vanilla `placeStructure(level)`.
    pub fn place_structure(&self, world: &Arc<World>) {
        let Some(name) = self.settings.lock().name.clone() else {
            return;
        };
        if let Some(template) = world.structure_templates().get(&name) {
            self.place_template(world, &name, &template);
        }
    }

    /// Vanilla `loadStructureInfo(template)`.
    fn load_structure_info(&self, author: &str, size: IVec3) {
        let mut settings = self.settings.lock();
        author.clone_into(&mut settings.author);
        settings.size = size;
        drop(settings);
        self.set_changed();
    }

    /// Vanilla `placeStructure(level, template)`.
    fn place_template(&self, world: &Arc<World>, name: &Identifier, template: &StoredTemplate) {
        self.load_structure_info(&template.author, IVec3::from_array(template.size()));
        let Some(mut parsed) = template.parse(name) else {
            return;
        };
        let settings = self.settings();
        if settings.ignore_entities {
            parsed.clear_entities();
        }
        let pos = self
            .get_block_pos()
            .offset(settings.pos.x, settings.pos.y, settings.pos.z);
        place_live(
            world,
            &parsed,
            pos,
            LivePlacement {
                rotation: settings.rotation,
                mirror: settings.mirror,
                integrity: settings.integrity,
                seed: template_seed(settings.seed),
                strict: settings.strict,
            },
        );
    }

    /// Vanilla `unloadStructure`: forgets the cached template.
    pub fn unload_structure(&self, world: &World) {
        if let Some(name) = &self.settings.lock().name {
            world.structure_templates().remove(name);
        }
    }

    /// Vanilla `detectSize`: a save block takes the box between the corner
    /// blocks of its name within 80 blocks.
    pub fn detect_size(&self, world: &World) -> bool {
        let settings = self.settings();
        if settings.mode != StructureMode::Save {
            return false;
        }
        let pos = self.get_block_pos();
        let Some((min, max)) = enclosing_box(world, pos, settings.name.as_ref()) else {
            return false;
        };
        let delta = max - min;
        if delta.x <= 1 || delta.y <= 1 || delta.z <= 1 {
            return false;
        }
        self.update_settings(|settings| {
            settings.pos = min - pos.0 + IVec3::ONE;
            settings.size = delta - IVec3::ONE;
        });
        self.set_changed();
        world.send_block_updated(pos);
        true
    }
}

/// Vanilla `getEnclosingBoundingBox`: the box around every corner block with
/// `name` near `pos`, taking in `pos` when there is only one.
fn enclosing_box(
    world: &World,
    pos: BlockPos,
    name: Option<&Identifier>,
) -> Option<(IVec3, IVec3)> {
    let min_x = pos.x() - SCAN_CORNER_BLOCKS_RANGE;
    let max_x = pos.x() + SCAN_CORNER_BLOCKS_RANGE;
    let min_z = pos.z() - SCAN_CORNER_BLOCKS_RANGE;
    let max_z = pos.z() + SCAN_CORNER_BLOCKS_RANGE;
    let mut corners = Vec::new();
    for chunk_z in (min_z >> 4)..=(max_z >> 4) {
        for chunk_x in (min_x >> 4)..=(max_x >> 4) {
            let block_entities = world
                .chunk_map
                .with_full_chunk(steel_utils::ChunkPos::new(chunk_x, chunk_z), |chunk| {
                    chunk.get_block_entities()
                })
                .unwrap_or_default();
            for block_entity in block_entities {
                let at = block_entity.get_block_pos();
                if !(min_x..=max_x).contains(&at.x()) || !(min_z..=max_z).contains(&at.z()) {
                    continue;
                }
                let Some(structure) =
                    steel_utils::Downcast::downcast_ref::<StructureBlockEntity>(&*block_entity)
                else {
                    continue;
                };
                let settings = structure.settings.lock();
                if settings.mode == StructureMode::Corner && settings.name.as_ref() == name {
                    corners.push(at.0);
                }
            }
        }
    }
    let first = *corners.first()?;
    let (mut min, mut max) = corners.iter().fold((first, first), |(min, max), corner| {
        (min.min(*corner), max.max(*corner))
    });
    if corners.len() == 1 {
        min = min.min(pos.0);
        max = max.max(pos.0);
    }
    Some((min, max))
}

impl BlockEntity for StructureBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt: NbtCompoundView<'_, '_> = nbt.into();
        let string = |key: &str| nbt.string(key).map(|value| value.to_str().into_owned());
        let int = |key: &str, default: i32, min: i32, max: i32| {
            nbt.int(key).unwrap_or(default).clamp(min, max)
        };
        let flag = |key: &str, default: bool| nbt.byte(key).map_or(default, |value| value != 0);
        let offset = MAX_OFFSET_PER_AXIS;
        let settings = StructureBlockSettings {
            name: parse_structure_name(&string("name").unwrap_or_default()),
            author: string("author").unwrap_or_default(),
            metadata: string("metadata").unwrap_or_default(),
            pos: IVec3::new(
                int("posX", DEFAULT_POS.x, -offset, offset),
                int("posY", DEFAULT_POS.y, -offset, offset),
                int("posZ", DEFAULT_POS.z, -offset, offset),
            ),
            size: IVec3::new(
                int("sizeX", 0, 0, MAX_SIZE_PER_AXIS),
                int("sizeY", 0, 0, MAX_SIZE_PER_AXIS),
                int("sizeZ", 0, 0, MAX_SIZE_PER_AXIS),
            ),
            rotation: string("rotation")
                .and_then(|name| rotation_by_name(&name))
                .unwrap_or(Rotation::None),
            mirror: string("mirror")
                .and_then(|name| mirror_by_name(&name))
                .unwrap_or(StructureMirror::None),
            mode: string("mode")
                .and_then(|name| mode_by_name(&name))
                .unwrap_or(StructureMode::Data),
            ignore_entities: flag("ignoreEntities", true),
            strict: flag("strict", false),
            powered: flag("powered", false),
            show_air: flag("showair", false),
            show_bounding_box: flag("showboundingbox", true),
            integrity: nbt.float("integrity").unwrap_or(1.0),
            seed: nbt.long("seed").unwrap_or(0),
        };
        let mode = settings.mode;
        *self.settings.lock() = settings;
        // Vanilla `updateBlockState`, only once the block is in its world.
        if block_mode(self.get_block_state()) != mode
            && let Some(world) = self.get_level()
            && world
                .get_block_entity(self.get_block_pos())
                .is_some_and(|placed| ptr::addr_eq(Arc::as_ptr(&placed), self))
        {
            self.update_block_state(&world, mode);
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let settings = self.settings();
        nbt.insert("name", settings.name_string());
        nbt.insert("author", settings.author);
        nbt.insert("metadata", settings.metadata);
        nbt.insert("posX", settings.pos.x);
        nbt.insert("posY", settings.pos.y);
        nbt.insert("posZ", settings.pos.z);
        nbt.insert("sizeX", settings.size.x);
        nbt.insert("sizeY", settings.size.y);
        nbt.insert("sizeZ", settings.size.z);
        nbt.insert("rotation", rotation_name(settings.rotation));
        nbt.insert("mirror", mirror_name(settings.mirror));
        nbt.insert("mode", mode_name(settings.mode));
        nbt.insert("ignoreEntities", i8::from(settings.ignore_entities));
        nbt.insert("strict", i8::from(settings.strict));
        nbt.insert("powered", i8::from(settings.powered));
        nbt.insert("showair", i8::from(settings.show_air));
        nbt.insert("showboundingbox", i8::from(settings.show_bounding_box));
        nbt.insert("integrity", settings.integrity);
        nbt.insert("seed", settings.seed);
    }

    /// Vanilla `getUpdateTag`: the client draws the box and fills the screen
    /// from this.
    fn get_update_tag(&self) -> Option<NbtCompound> {
        Some(self.save_custom_only())
    }
}
