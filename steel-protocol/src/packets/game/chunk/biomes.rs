use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_CHUNKS_BIOMES;
use steel_utils::PackedChunkPos;

/// Vanilla `ClientboundChunksBiomesPacket`: new biomes for chunks the
/// client already has, as `/fillbiome` sends them.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_CHUNKS_BIOMES)]
pub struct CChunksBiomes {
    /// One entry per changed chunk.
    #[write(as = Prefixed(VarInt))]
    pub chunk_biome_data: Vec<ChunkBiomeData>,
}

/// Vanilla `ClientboundChunksBiomesPacket.ChunkBiomeData`.
#[derive(WriteTo, Clone, Debug)]
pub struct ChunkBiomeData {
    /// The chunk the biomes belong to.
    pub pos: PackedChunkPos,
    /// Every section's biome container, bottom to top, in the chunk
    /// packet's encoding.
    #[write(as = Prefixed(VarInt), bound = 2097152)]
    pub buffer: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use steel_utils::{ChunkPos, PackedChunkPos, serial::WriteTo};

    use super::{CChunksBiomes, ChunkBiomeData};

    #[test]
    fn writes_count_then_packed_position_and_prefixed_buffer() {
        let packet = CChunksBiomes {
            chunk_biome_data: vec![ChunkBiomeData {
                pos: PackedChunkPos::from(ChunkPos::new(1, -2)),
                buffer: vec![0xAA, 0xBB],
            }],
        };
        let mut data = Vec::new();
        let Ok(()) = packet.write(&mut data) else {
            panic!("chunk biomes packet should encode");
        };
        // Count, then vanilla ChunkPos.pack(): x low, z high, big-endian.
        let mut expected = vec![1];
        expected.extend_from_slice(&((-2_i64 << 32) | 1).to_be_bytes());
        expected.extend_from_slice(&[2, 0xAA, 0xBB]);
        assert_eq!(data, expected);
    }
}
