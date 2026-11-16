//! Conversion of vanilla POI chunks (`poi/r.<x>.<z>.mca`).
//!
//! Steel rebuilds points of interest from block states when a chunk loads and persists only
//! how many tickets each one has left, so only `free_tickets` is carried over.

use simdnbt::owned::{NbtCompound, NbtTag};
use steel_utils::ChunkPos;

use super::chunk::check_data_version;
use super::nbt;
use super::report::ImportReport;
use crate::chunk_saver::PersistentPoi;

/// Converts a POI chunk into Steel ticket records.
pub fn convert_poi_chunk(
    bytes: &[u8],
    pos: ChunkPos,
    min_y: i32,
    height: i32,
    report: &mut ImportReport,
) -> Result<Vec<PersistentPoi>, String> {
    let root = nbt::read_root(bytes)?;
    check_data_version(&root).map_err(|error| format!("POI chunk {error}"))?;
    let Some(sections) = root.compound("Sections") else {
        return Ok(Vec::new());
    };

    let mut records = Vec::new();
    for (_, section) in sections.iter() {
        let Some(section) = section.compound() else {
            continue;
        };
        // Vanilla rebuilds sections that are not valid from the blocks, dropping their tickets.
        if !nbt::boolean(section, "Valid") {
            continue;
        }
        for record in section
            .list("Records")
            .and_then(|list| list.compounds())
            .unwrap_or_default()
        {
            let Some([x, y, z]) = record
                .int_array("pos")
                .and_then(|pos| <[i32; 3]>::try_from(pos).ok())
            else {
                ImportReport::count(&mut report.dropped_records, "malformed POI record");
                ImportReport::count(&mut report.malformed_poi_shapes, shape(record));
                continue;
            };
            // Vanilla leaves `free_tickets` out when it is 0: a taken bed or workstation, and
            // every nether portal, which has no tickets at all.
            let free_tickets = record.int("free_tickets").unwrap_or(0);
            let in_chunk = x.div_euclid(16) == pos.0.x
                && z.div_euclid(16) == pos.0.y
                && (min_y..min_y + height).contains(&y);
            let Ok(free_tickets) = u32::try_from(free_tickets) else {
                ImportReport::count(&mut report.dropped_records, "malformed POI record");
                continue;
            };
            if !in_chunk {
                ImportReport::count(&mut report.dropped_records, "POI outside its chunk");
                continue;
            }
            report.pois += 1;
            records.push(PersistentPoi {
                x: x.rem_euclid(16) as u8,
                y: y as i16,
                z: z.rem_euclid(16) as u8,
                free_tickets,
            });
        }
    }
    Ok(records)
}

/// The fields of a record with their types, to tell in the report what a record that could
/// not be read looks like.
fn shape(record: &NbtCompound) -> String {
    let mut fields: Vec<String> = record
        .iter()
        .map(|(name, tag)| {
            let kind = match tag {
                NbtTag::Byte(_) => "byte",
                NbtTag::Short(_) => "short",
                NbtTag::Int(_) => "int",
                NbtTag::Long(_) => "long",
                NbtTag::String(_) => "string",
                NbtTag::IntArray(array) if array.len() == 3 => "int[3]",
                NbtTag::IntArray(_) => "int[]",
                NbtTag::List(_) => "list",
                NbtTag::Compound(_) => "compound",
                _ => "other",
            };
            format!("{name}: {kind}")
        })
        .collect();
    fields.sort();
    fields.join(", ")
}
