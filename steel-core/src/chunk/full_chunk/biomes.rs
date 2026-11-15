//! Biome rewrites on a live chunk, as `/fillbiome` makes them.

use std::io::Cursor;

use super::FullChunkRef;

/// Quarts (4×4×4 biome cells) along one side of a section.
const QUARTS_PER_SECTION: usize = 4;

impl FullChunkRef<'_> {
    /// Vanilla `ChunkAccess.fillBiomesFromNoise` with a command's resolver:
    /// `resolver(quart_x, quart_y, quart_z, current)` picks every quart's
    /// biome from its absolute quart position and the biome there now.
    ///
    /// Each section's container is rebuilt the way vanilla recreates it.
    /// Returns whether any biome changed; the chunk is then marked unsaved,
    /// as vanilla's `markUnsaved` after a counted change.
    pub(crate) fn fill_biomes(self, mut resolver: impl FnMut(i32, i32, i32, u16) -> u16) -> bool {
        let pos = self.chunk.pos();
        let quart_x = pos.0.x * QUARTS_PER_SECTION as i32;
        let quart_z = pos.0.y * QUARTS_PER_SECTION as i32;
        let min_section_y = self.min_y().div_euclid(16);
        let mut changed = false;
        for (section_index, holder) in self.chunk.sections.sections.iter().enumerate() {
            let quart_y = (min_section_y + section_index as i32) * QUARTS_PER_SECTION as i32;
            let mut section = holder.write();
            let biomes = section.biomes_mut();
            biomes.enter_building_mode();
            let Some(cells) = biomes.as_building_slice_mut() else {
                unreachable!("a container in building mode exposes its cells")
            };
            for (index, cell) in cells.iter_mut().enumerate() {
                let x = index % QUARTS_PER_SECTION;
                let z = (index / QUARTS_PER_SECTION) % QUARTS_PER_SECTION;
                let y = index / (QUARTS_PER_SECTION * QUARTS_PER_SECTION);
                let biome = resolver(
                    quart_x + x as i32,
                    quart_y + y as i32,
                    quart_z + z as i32,
                    *cell,
                );
                changed |= biome != *cell;
                *cell = biome;
            }
            biomes.finalize_building();
        }
        if changed {
            self.mark_unsaved();
        }
        changed
    }

    /// Vanilla `ClientboundChunksBiomesPacket.ChunkBiomeData.extractChunkData`:
    /// every section's biome container, bottom to top.
    ///
    /// # Panics
    ///
    /// Panics if a biome container holds a biome its palette cannot encode,
    /// which the chunk packet would refuse the same way.
    #[must_use]
    pub(crate) fn extract_biome_data(self) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        for holder in &self.chunk.sections.sections {
            holder
                .read()
                .biomes
                .write(&mut cursor)
                .expect("biome containers encode like the chunk packet");
        }
        cursor.into_inner()
    }
}
