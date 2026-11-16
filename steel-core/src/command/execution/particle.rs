//! Vanilla `ParticleArgument`: a particle id, then its options as SNBT.

use simdnbt::owned::NbtCompound;
use steel_registry::particle_type::ParticleData;
use steel_registry::{REGISTRY, RegistryExt as _};
use steel_utils::nbt::parse_snbt_compound_argument;
use steel_utils::translations;

use super::argument::parse_identifier;
use super::suggestions::suggest_resources;
use crate::command::brigadier::{
    CommandSyntaxError, CommandSyntaxErrorKind, StringReader, SuggestionsBuilder,
};
use crate::world::particle_codec;

/// Vanilla `ParticleArgument.readParticle`.
pub(super) fn parse_particle(
    reader: &mut StringReader<'_>,
) -> Result<ParticleData, CommandSyntaxError> {
    let id = parse_identifier(reader)?;
    let Some(particle_type) = REGISTRY.particle_types.by_key(&id) else {
        return Err(reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(
            translations::PARTICLE_NOT_FOUND
                .message([id.to_string()])
                .component(),
        ))));
    };
    let options = if reader.peek() == Some('{') {
        let (options, consumed) =
            parse_snbt_compound_argument(reader.remaining()).map_err(|error| {
                reader.advance_bytes(error.cursor());
                reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(error.component())))
            })?;
        reader.advance_bytes(consumed);
        options
    } else {
        NbtCompound::new()
    };
    particle_codec::decode_options(particle_type, &options).map_err(|message| {
        CommandSyntaxError::dynamic(
            translations::PARTICLE_INVALID_OPTIONS
                .message([message])
                .component(),
        )
    })
}

/// Vanilla `ParticleArgument.listSuggestions`: every particle id.
pub(super) fn suggest_particles(builder: &mut SuggestionsBuilder<'_>) {
    suggest_resources(
        REGISTRY
            .particle_types
            .iter()
            .map(|(_, particle_type)| &particle_type.key),
        builder,
    );
}
