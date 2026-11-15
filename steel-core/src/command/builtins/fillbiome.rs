//! Vanilla `/fillbiome` (`FillBiomeCommand`): set the biome of every quart
//! in a region, then resend the changed chunks' biomes to their viewers.

use rustc_hash::FxHashMap;
use steel_protocol::packets::game::{CChunksBiomes, ChunkBiomeData};
use steel_registry::biome::BiomeRef;
use steel_registry::vanilla_game_rules::MAX_BLOCK_MODIFICATIONS;
use steel_registry::{REGISTRY, RegistryEntry as _, RegistryExt as _};
use steel_utils::{BlockPos, BoundingBox, ChunkPos, Identifier, PackedChunkPos, translations};
use text_components::TextComponent;

use super::execute::loaded_block_position;
use super::fill::block_region_volume;
use crate::command::brigadier::{CommandNodeBuilder, CommandSyntaxError};
use crate::command::execution::{
    BiomeOrTag, CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime,
    argument, literal,
};
use crate::command::registration::CommandRegistration;
use crate::world::World;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("fillbiome"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("fillbiome").then(
        argument("from", SteelArgumentType::block_pos()).then(
            argument("to", SteelArgumentType::block_pos()).then(
                argument("biome", SteelArgumentType::biome())
                    .executes(|context| fill_command(context, None))
                    .then(literal("replace").then(
                        argument("filter", SteelArgumentType::biome_or_tag()).executes(|context| {
                            fill_command(context, Some(context.biome_or_tag("filter")?))
                        }),
                    )),
            ),
        ),
    )
}

fn fill_command(
    context: &SteelCommandContext<CommandSource>,
    filter: Option<&BiomeOrTag>,
) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let from = loaded_block_position(context, "from")?;
    let to = loaded_block_position(context, "to")?;
    let biome = context.biome("biome")?;
    let (count, region) = fill(source.world(), from, to, biome, |current| {
        filter.is_none_or(|filter| filter.matches(current))
    })?;
    source.send_success(
        &translations::COMMANDS_FILLBIOME_SUCCESS_COUNT
            .message(
                [
                    count,
                    region.min_x(),
                    region.min_y(),
                    region.min_z(),
                    region.max_x(),
                    region.max_y(),
                    region.max_z(),
                ]
                .map(|value| TextComponent::plain(value.to_string())),
            )
            .component(),
        true,
    );
    Ok(count)
}

/// Vanilla `quantize`: the block at the corner of the position's quart.
const fn quantize(pos: BlockPos) -> BlockPos {
    BlockPos::new(
        (pos.0.x >> 2) << 2,
        (pos.0.y >> 2) << 2,
        (pos.0.z >> 2) << 2,
    )
}

/// Vanilla `FillBiomeCommand.fill`: the number of quarts changed and the
/// quantized region, or the command's failure.
pub(crate) fn fill(
    world: &World,
    raw_from: BlockPos,
    raw_to: BlockPos,
    biome: BiomeRef,
    filter: impl Fn(BiomeRef) -> bool,
) -> Result<(i32, BoundingBox), CommandSyntaxError> {
    let region = BoundingBox::from_corners(quantize(raw_from), quantize(raw_to));
    let volume = block_region_volume(region);
    let limit = world.get_game_rule(&MAX_BLOCK_MODIFICATIONS);
    if volume > i64::from(limit) {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_FILLBIOME_TOOBIG
                .message([limit.to_string(), volume.to_string()])
                .component(),
        ));
    }

    let mut chunks = Vec::new();
    for chunk_z in region.min_z() >> 4..=region.max_z() >> 4 {
        for chunk_x in region.min_x() >> 4..=region.max_x() >> 4 {
            let pos = ChunkPos::new(chunk_x, chunk_z);
            if world.chunk_map.with_full_chunk(pos, |_| ()).is_none() {
                return Err(CommandSyntaxError::dynamic(TextComponent::from(
                    &translations::ARGUMENT_POS_UNLOADED,
                )));
            }
            chunks.push(pos);
        }
    }

    let target = biome.id() as u16;
    let mut count: i32 = 0;
    chunks.retain(|&pos| {
        world
            .chunk_map
            .with_full_chunk(pos, |chunk| {
                chunk.fill_biomes(|quart_x, quart_y, quart_z, current| {
                    let inside = region.contains_xyz(quart_x << 2, quart_y << 2, quart_z << 2);
                    let matches = REGISTRY
                        .biomes
                        .by_id(usize::from(current))
                        .is_some_and(&filter);
                    if inside && matches {
                        if current != target {
                            count += 1;
                        }
                        target
                    } else {
                        current
                    }
                })
            })
            .unwrap_or(false)
    });

    if count == 0 {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_FILLBIOME_NO_CHANGES,
        )));
    }
    resend_biomes(world, &chunks);
    Ok((count, region))
}

/// Vanilla `ChunkMap.resendBiomesForChunks`: one packet per player with
/// every changed chunk their client has.
fn resend_biomes(world: &World, chunks: &[ChunkPos]) {
    let mut by_player: FxHashMap<i32, Vec<ChunkBiomeData>> = FxHashMap::default();
    for &pos in chunks {
        #[expect(
            clippy::redundant_closure_for_method_calls,
            reason = "the method path does not satisfy the higher-ranked lifetime bound"
        )]
        let Some(buffer) = world
            .chunk_map
            .with_full_chunk(pos, |chunk| chunk.extract_biome_data())
        else {
            continue;
        };
        for player in world.get_packet_tracking_players(pos) {
            by_player.entry(player).or_default().push(ChunkBiomeData {
                pos: PackedChunkPos::from(pos),
                buffer: buffer.clone(),
            });
        }
    }
    for (entity_id, chunk_biome_data) in by_player {
        if let Some(player) = world.players.get_by_entity_id(entity_id) {
            player.send_packet(CChunksBiomes { chunk_biome_data });
        }
    }
}
