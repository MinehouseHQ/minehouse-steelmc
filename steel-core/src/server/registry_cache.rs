use simdnbt::ToNbtTag;
use std::sync::Arc;

use steel_protocol::packet_traits::{ClientPacket, EncodedPacket};
use steel_protocol::packets::common::TagCollection;
use steel_protocol::{
    packets::{
        common::CUpdateTags,
        config::{CRegistryData, RegistryEntry},
    },
    utils::ConnectionProtocol,
};

use steel_registry::blocks::BlockRegistry;
use steel_registry::{
    BANNER_PATTERN_REGISTRY, BIOMES_REGISTRY, BLOCK_STATE_PROVIDER_REGISTRY,
    BLOCK_TRANSFORMER_REGISTRY, BLOCKS_REGISTRY, CAT_SOUND_VARIANT_REGISTRY, CAT_VARIANT_REGISTRY,
    CHAT_TYPE_REGISTRY, CHICKEN_SOUND_VARIANT_REGISTRY, CHICKEN_VARIANT_REGISTRY,
    COW_SOUND_VARIANT_REGISTRY, COW_VARIANT_REGISTRY, DAMAGE_TYPE_REGISTRY,
    DECORATED_POT_PATTERN_REGISTRY, DIALOG_REGISTRY, DIMENSION_TYPE_REGISTRY, ENCHANTMENT_REGISTRY,
    ENTITY_TYPE_REGISTRY, FLUID_REGISTRY, FROG_VARIANT_REGISTRY, INSTRUMENT_REGISTRY,
    ITEMS_REGISTRY, JUKEBOX_SONG_REGISTRY, PAINTING_VARIANT_REGISTRY, PIG_SOUND_VARIANT_REGISTRY,
    PIG_VARIANT_REGISTRY, REGISTRY, Registry, RegistryEntry as _, TIMELINE_REGISTRY,
    TRIM_MATERIAL_REGISTRY, TRIM_PATTERN_REGISTRY, TaggedRegistryExt, WOLF_SOUND_VARIANT_REGISTRY,
    WOLF_VARIANT_REGISTRY, WORLD_CLOCK_REGISTRY, ZOMBIE_NAUTILUS_VARIANT_REGISTRY,
};
use steel_utils::Identifier;
use steel_utils::codec::VarInt;
use steel_utils::types::client_block_state_count;

use steel_protocol::packet_traits::CompressionInfo;

/// Caches compressed registry packets to avoid re-compressing them for every player.
pub struct RegistryCache {
    /// The cached registry data packets.
    pub registry_packets: Arc<[EncodedPacket]>,
    /// The cached tags packet.
    pub tags_packet: Arc<EncodedPacket>,
}

/// The block tags as the client is sent them. A registry extension may add blocks the
/// client's game does not have, and tag them (so that a pickaxe mines them, for one). Such
/// a block is left out: an id past the client's registry would have it drop the connection.
/// `client_states` is the number of block states the client has, if it is not all of them.
fn block_tags(
    blocks: &BlockRegistry,
    client_states: Option<u16>,
) -> Vec<(Identifier, Vec<VarInt>)> {
    blocks
        .tag_keys()
        .map(|tag_key| {
            let ids = blocks
                .iter_tag(tag_key)
                .filter(|block| {
                    client_states.is_none_or(|count| blocks.get_base_state_id(block).0 < count)
                })
                .map(|block| VarInt::from(block.id()))
                .collect();
            (tag_key.clone(), ids)
        })
        .collect()
}

impl RegistryCache {
    /// Creates a new `RegistryCache` from the given registry.
    #[must_use]
    pub fn new(compression: Option<CompressionInfo>) -> Self {
        let registry_packets = Self::build_registry_packets(&REGISTRY);
        let tags_by_registry_packet = Self::build_tags_packet(&REGISTRY);

        let (registry_packets, tags_packet) =
            build_compressed_packets(registry_packets, tags_by_registry_packet, compression);

        Self {
            registry_packets,
            tags_packet: Arc::new(tags_packet),
        }
    }

    fn build_registry_packets(registry: &Registry) -> Vec<CRegistryData> {
        let mut packets = Vec::with_capacity(9);

        macro_rules! add_registry {
            ($reg_key:expr, $field:ident) => {
                packets.push(CRegistryData::new(
                    $reg_key,
                    registry
                        .$field
                        .iter()
                        .map(|(_, entry)| {
                            RegistryEntry::new(entry.key.clone(), Some(entry.to_nbt_tag()))
                        })
                        .collect(),
                ));
            };
        }

        //TODO: For non vanilla entries we need to encode the data into nbt

        add_registry!(BIOMES_REGISTRY, biomes);
        add_registry!(CHAT_TYPE_REGISTRY, chat_types);
        add_registry!(TRIM_PATTERN_REGISTRY, trim_patterns);
        add_registry!(TRIM_MATERIAL_REGISTRY, trim_materials);
        add_registry!(DECORATED_POT_PATTERN_REGISTRY, decorated_pot_patterns);
        add_registry!(WOLF_VARIANT_REGISTRY, wolf_variants);
        add_registry!(WOLF_SOUND_VARIANT_REGISTRY, wolf_sound_variants);
        add_registry!(PIG_VARIANT_REGISTRY, pig_variants);
        add_registry!(PIG_SOUND_VARIANT_REGISTRY, pig_sound_variants);
        add_registry!(FROG_VARIANT_REGISTRY, frog_variants);
        add_registry!(CAT_VARIANT_REGISTRY, cat_variants);
        add_registry!(CAT_SOUND_VARIANT_REGISTRY, cat_sound_variants);
        add_registry!(COW_VARIANT_REGISTRY, cow_variants);
        add_registry!(COW_SOUND_VARIANT_REGISTRY, cow_sound_variants);
        add_registry!(CHICKEN_VARIANT_REGISTRY, chicken_variants);
        add_registry!(CHICKEN_SOUND_VARIANT_REGISTRY, chicken_sound_variants);
        add_registry!(PAINTING_VARIANT_REGISTRY, painting_variants);
        add_registry!(DIMENSION_TYPE_REGISTRY, dimension_types);
        add_registry!(DAMAGE_TYPE_REGISTRY, damage_types);
        add_registry!(BANNER_PATTERN_REGISTRY, banner_patterns);
        add_registry!(ZOMBIE_NAUTILUS_VARIANT_REGISTRY, zombie_nautilus_variants);

        add_registry!(ENCHANTMENT_REGISTRY, enchantments);

        add_registry!(JUKEBOX_SONG_REGISTRY, jukebox_songs);
        add_registry!(INSTRUMENT_REGISTRY, instruments);
        add_registry!(TIMELINE_REGISTRY, timelines);
        add_registry!(DIALOG_REGISTRY, dialogs);

        add_registry!(WORLD_CLOCK_REGISTRY, world_clocks);
        add_registry!(BLOCK_STATE_PROVIDER_REGISTRY, block_state_providers);
        add_registry!(BLOCK_TRANSFORMER_REGISTRY, block_transformers);

        packets
    }

    fn build_tags_packet(registry: &Registry) -> CUpdateTags {
        let mut tags_by_registry: TagCollection = Vec::with_capacity(10);
        macro_rules! add_tags {
            ($reg_key:expr, $field:ident) => {
                let mut tags: Vec<(Identifier, Vec<VarInt>)> =
                    Vec::with_capacity(registry.$field.tag_keys().count());
                for tag_key in registry.$field.tag_keys() {
                    let mut ids = Vec::with_capacity(registry.$field.iter_tag(tag_key).count());
                    for entry in registry.$field.iter_tag(tag_key) {
                        ids.push(VarInt::from(entry.id()));
                    }
                    tags.push((tag_key.clone(), ids));
                }
                tags_by_registry.push(($reg_key, tags));
            };
        }

        tags_by_registry.push((
            BLOCKS_REGISTRY,
            block_tags(&registry.blocks, client_block_state_count()),
        ));
        add_tags!(ITEMS_REGISTRY, items);
        add_tags!(TIMELINE_REGISTRY, timelines);
        add_tags!(DIALOG_REGISTRY, dialogs);
        add_tags!(DAMAGE_TYPE_REGISTRY, damage_types);
        add_tags!(BANNER_PATTERN_REGISTRY, banner_patterns);
        add_tags!(ENTITY_TYPE_REGISTRY, entity_types);
        add_tags!(INSTRUMENT_REGISTRY, instruments);
        add_tags!(PAINTING_VARIANT_REGISTRY, painting_variants);
        add_tags!(ENCHANTMENT_REGISTRY, enchantments);

        // Build fluid tags
        let mut fluid_tags: Vec<(Identifier, Vec<VarInt>)> =
            Vec::with_capacity(registry.fluids.tag_keys().count());
        for tag_key in registry.fluids.tag_keys() {
            let mut fluid_ids = Vec::with_capacity(registry.fluids.iter_tag(tag_key).count());

            for fluid in registry.fluids.iter_tag(tag_key) {
                fluid_ids.push(VarInt::from(fluid.id() as i32));
            }

            fluid_tags.push((tag_key.clone(), fluid_ids));
        }
        tags_by_registry.push((FLUID_REGISTRY, fluid_tags));

        // Build and return a CUpdateTagsPacket based on the registry data
        CUpdateTags::new(tags_by_registry)
    }
}

/// Compresses a packet.
fn compress_packet<P: ClientPacket>(
    packet: P,
    compression: Option<CompressionInfo>,
) -> Option<EncodedPacket> {
    let id = packet.get_id(ConnectionProtocol::Config);

    EncodedPacket::from_bare(packet, compression, ConnectionProtocol::Config)
        .map_err(|_| {
            log::error!("Failed to encode packet: {id:?}");
        })
        .ok()
}

/// # Panics
/// This function will panic if the compression fails.
#[must_use]
pub fn build_compressed_packets(
    registry_packets: Vec<CRegistryData>,
    tags_packet: CUpdateTags,
    compression: Option<CompressionInfo>,
) -> (Arc<[EncodedPacket]>, EncodedPacket) {
    let mut compressed_packets = Vec::with_capacity(registry_packets.len());

    for packet in registry_packets {
        compressed_packets
            .push(compress_packet(packet, compression).expect("Failed to compress packet"));
    }

    let compressed_tags_packet =
        compress_packet(tags_packet, compression).expect("Failed to compress tags packet");

    (compressed_packets.into(), compressed_tags_packet)
}

#[cfg(test)]
mod tests {
    use steel_registry::{
        RegistryExt as _,
        blocks::{Block, behavior::BlockConfig},
        vanilla_blocks,
    };

    use super::*;

    /// A block of a registry extension: the client's game does not have it.
    static EXTENSION_BLOCK: Block = Block::new(
        Identifier::new_static("steel-test", "tagged_extension_block"),
        BlockConfig::new(),
        &[],
    );

    #[test]
    fn a_tagged_block_the_client_lacks_stays_out_of_its_tags() {
        let mut registry = Registry::new_vanilla();
        let client_states = registry.blocks.next_state_id;
        let pickaxe = Identifier::vanilla_static("mineable/pickaxe");
        let vanilla_len = registry.blocks.iter_tag(&pickaxe).count();
        registry.blocks.register(&EXTENSION_BLOCK);
        registry.blocks.modify_tag(&pickaxe, |mut keys| {
            keys.push(EXTENSION_BLOCK.key.clone());
            keys
        });
        // The server has it in the tag.
        assert!(registry.blocks.is_in_tag(&EXTENSION_BLOCK, &pickaxe));

        let ids_of = |tags: Vec<(Identifier, Vec<VarInt>)>| {
            tags.into_iter()
                .find(|(key, _)| *key == pickaxe)
                .map(|(_, ids)| ids)
                .expect("the pickaxe tag is sent")
        };
        let stone = registry
            .blocks
            .id_from_key(&vanilla_blocks::STONE.key)
            .expect("stone is a block");
        let extension = EXTENSION_BLOCK.id();
        let has = |ids: &[VarInt], id: usize| ids.iter().any(|sent| sent.0 as usize == id);

        // The client is sent the vanilla blocks of the tag, and not ours.
        let sent = ids_of(block_tags(&registry.blocks, Some(client_states)));
        assert_eq!(sent.len(), vanilla_len);
        assert!(has(&sent, stone));
        assert!(!has(&sent, extension));

        // Without a smaller client registry nothing is left out.
        let all = ids_of(block_tags(&registry.blocks, None));
        assert_eq!(all.len(), vanilla_len + 1);
        assert!(has(&all, extension));
    }
}
