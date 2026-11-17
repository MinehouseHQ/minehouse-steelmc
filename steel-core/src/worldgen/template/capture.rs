//! Vanilla `StructureTemplate.fillFromWorld` and `StructureTemplate.save`:
//! the template a structure block captures, written as vanilla saves it.

use std::ptr;

use glam::{DVec3, IVec3};
use rustc_hash::FxHashMap;
use simdnbt::owned::{NbtCompound, NbtList};
use steel_registry::REGISTRY;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_utils::version::DATA_VERSION;
use steel_utils::{BlockPos, BlockStateId, Downcast as _, WorldAabb};

use super::{StructureBlockInfo, StructureTemplate};
use crate::block_entity::block_state_nbt;
use crate::entity::block_attached_entity::BlockAttachedEntity as _;
use crate::entity::entities::PaintingEntity;
use crate::world::World;

/// An entity captured into a template: vanilla `StructureEntityInfo`.
struct CapturedEntity {
    pos: DVec3,
    block_pos: BlockPos,
    nbt: NbtCompound,
}

/// Vanilla `save` of a `new StructureTemplate()`: no blocks, no entities,
/// size zero.
pub(super) fn write_empty_template(nbt: &mut NbtCompound) {
    nbt.insert("blocks", NbtList::Empty);
    nbt.insert("palette", NbtList::Empty);
    nbt.insert("entities", NbtList::Empty);
    nbt.insert("size", NbtList::Int(vec![0, 0, 0]));
    nbt.insert("DataVersion", DATA_VERSION);
}

/// Vanilla `fillFromWorld` followed by `save`: the blocks in the box of
/// `size` from `position`, minus `ignore_blocks`, and the entities in it when
/// `include_entities`. `None` when the box is empty, as vanilla then keeps
/// the template it had.
pub(crate) fn fill_from_world(
    world: &World,
    position: BlockPos,
    size: IVec3,
    include_entities: bool,
    ignore_blocks: &[BlockRef],
) -> Option<NbtCompound> {
    if size.x < 1 || size.y < 1 || size.z < 1 {
        return None;
    }
    let corner = position.offset(size.x - 1, size.y - 1, size.z - 1);
    let min = BlockPos::new(
        position.x().min(corner.x()),
        position.y().min(corner.y()),
        position.z().min(corner.z()),
    );
    let max = BlockPos::new(
        position.x().max(corner.x()),
        position.y().max(corner.y()),
        position.z().max(corner.z()),
    );
    let blocks = capture_blocks(world, min, max, ignore_blocks);
    let entities = if include_entities {
        capture_entities(world, min, max)
    } else {
        Vec::new()
    };
    let mut nbt = NbtCompound::new();
    write_blocks(&mut nbt, &blocks);
    nbt.insert(
        "entities",
        NbtList::Compound(entities.into_iter().map(entity_tag).collect()),
    );
    nbt.insert("size", NbtList::Int(vec![size.x, size.y, size.z]));
    nbt.insert("DataVersion", DATA_VERSION);
    Some(nbt)
}

/// The blocks of the box in vanilla's order: full blocks, other blocks, then
/// block entities, each sorted by y, x and z.
fn capture_blocks(
    world: &World,
    min: BlockPos,
    max: BlockPos,
    ignore_blocks: &[BlockRef],
) -> Vec<StructureBlockInfo> {
    let mut full_blocks = Vec::new();
    let mut other_blocks = Vec::new();
    let mut block_entities = Vec::new();
    for y in min.y()..=max.y() {
        for z in min.z()..=max.z() {
            for x in min.x()..=max.x() {
                let pos = BlockPos::new(x, y, z);
                let state = world.get_block_state(pos);
                if ignore_blocks
                    .iter()
                    .any(|block| ptr::eq(*block, state.get_block()))
                {
                    continue;
                }
                let nbt = world.get_block_entity(pos).map(|block_entity| {
                    let mut nbt = block_entity.save_without_metadata();
                    nbt.insert("id", block_entity.get_type().key.to_string());
                    nbt
                });
                let info = StructureBlockInfo {
                    pos: pos.offset(-min.x(), -min.y(), -min.z()),
                    state,
                    nbt,
                };
                if info.nbt.is_some() {
                    block_entities.push(info);
                } else if StructureTemplate::is_static_full_block(&REGISTRY, state) {
                    full_blocks.push(info);
                } else {
                    other_blocks.push(info);
                }
            }
        }
    }
    StructureTemplate::sort_block_infos(&mut full_blocks);
    StructureTemplate::sort_block_infos(&mut other_blocks);
    StructureTemplate::sort_block_infos(&mut block_entities);
    full_blocks.extend(other_blocks);
    full_blocks.extend(block_entities);
    full_blocks
}

/// Vanilla `fillEntityList`: every entity but players in the box, saved as
/// vanilla `Entity.save` saves it (passengers save nothing).
fn capture_entities(world: &World, min: BlockPos, max: BlockPos) -> Vec<CapturedEntity> {
    let aabb = WorldAabb::new(
        f64::from(min.x()),
        f64::from(min.y()),
        f64::from(min.z()),
        f64::from(max.x() + 1),
        f64::from(max.y() + 1),
        f64::from(max.z() + 1),
    );
    let origin = min.0.as_dvec3();
    world
        .get_entities_in_aabb_matching(&aabb, |entity| entity.as_player().is_none())
        .into_iter()
        .map(|entity| {
            let pos = entity.position() - origin;
            let block_pos = match entity.downcast_ref::<PaintingEntity>() {
                Some(painting) => {
                    let hanging = painting.block_attached_entity_base().pos();
                    hanging.offset(-min.x(), -min.y(), -min.z())
                }
                None => BlockPos::containing(pos.x, pos.y, pos.z),
            };
            let nbt = if entity.is_passenger() {
                NbtCompound::new()
            } else {
                entity.nbt_for_passenger_save().unwrap_or_default()
            };
            CapturedEntity {
                pos,
                block_pos,
                nbt,
            }
        })
        .collect()
}

/// Vanilla `save` of a single palette: each block's `pos`, `state` and
/// `nbt`, with states numbered in the order they first appear.
fn write_blocks(nbt: &mut NbtCompound, blocks: &[StructureBlockInfo]) {
    let mut ids: FxHashMap<BlockStateId, i32> = FxHashMap::default();
    let mut palette = Vec::new();
    let mut block_tags = Vec::with_capacity(blocks.len());
    for block in blocks {
        let next = i32::try_from(palette.len()).unwrap_or(i32::MAX);
        let id = *ids.entry(block.state).or_insert_with(|| {
            palette.push(block_state_nbt::save_full(block.state));
            next
        });
        let mut tag = NbtCompound::new();
        tag.insert(
            "pos",
            NbtList::Int(vec![block.pos.x(), block.pos.y(), block.pos.z()]),
        );
        tag.insert("state", id);
        if let Some(block_nbt) = &block.nbt {
            tag.insert("nbt", block_nbt.clone());
        }
        block_tags.push(tag);
    }
    nbt.insert("blocks", NbtList::Compound(block_tags));
    nbt.insert("palette", NbtList::Compound(palette));
}

fn entity_tag(entity: CapturedEntity) -> NbtCompound {
    let mut tag = NbtCompound::new();
    tag.insert(
        "pos",
        NbtList::Double(vec![entity.pos.x, entity.pos.y, entity.pos.z]),
    );
    tag.insert(
        "blockPos",
        NbtList::Int(vec![
            entity.block_pos.x(),
            entity.block_pos.y(),
            entity.block_pos.z(),
        ]),
    );
    tag.insert("nbt", entity.nbt);
    tag
}
