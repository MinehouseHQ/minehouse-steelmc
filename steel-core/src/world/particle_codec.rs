//! Vanilla `ParticleTypes.CODEC` and each particle type's options codec, over
//! NBT: the `/particle` options and the particles entities save.

use std::io::Cursor;

use glam::DVec3;
use simdnbt::borrow::{NbtTag as BorrowedNbtTag, read_tag};
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use simdnbt::{FromNbtTag as _, ToNbtTag as _};
use steel_registry::item_stack_template::ItemStackTemplate;
use steel_registry::particle_type::{
    BlockParticleOption, ColorParticleOption, DustColorTransitionOptions, DustParticleOptions,
    GeyserBaseParticleOptions, GeyserParticleOptions, ItemParticleOption, ParticleData,
    ParticleTypeRef, PowerParticleOption, SculkChargeParticleOptions, ShriekParticleOption,
    SimpleParticleOptions, SpellParticleOption, TrailParticleOption, VibrationParticleOption,
};
use steel_registry::position_source::{BlockPositionSource, PositionSource};
use steel_registry::{REGISTRY, RegistryExt as _, vanilla_position_source_types};
use steel_utils::nbt::{NbtNumeric as _, nbt_collection_values, to_canonical_snbt};
use steel_utils::{ArgbColor, BlockPos, BlockStateId, DowncastType as _, Identifier, RgbColor};

use crate::block_entity::block_state_nbt;

/// Vanilla `ScalableParticleOptionsBase.MIN_SCALE`.
const MIN_SCALE: f32 = 0.01;
/// Vanilla `ScalableParticleOptionsBase.MAX_SCALE`.
const MAX_SCALE: f32 = 4.0;

/// A codec failure, worded like vanilla's `DataResult` errors.
type DecodeResult<T> = Result<T, String>;

/// Vanilla `ParticleTypes.CODEC`: the particle id under `type`, with its
/// options beside it.
pub(crate) fn decode(tag: &NbtTag) -> DecodeResult<ParticleData> {
    let NbtTag::Compound(compound) = tag else {
        return Err(format!("Not a map: {}", snbt(tag)));
    };
    let id = compound
        .string("type")
        .ok_or_else(|| missing_key("type", compound))?
        .to_str();
    let key = id
        .parse::<Identifier>()
        .map_err(|_| format!("Not a valid resource location: {id}"))?;
    let particle_type = REGISTRY
        .particle_types
        .by_key(&key)
        .ok_or_else(|| format!("Unknown registry key in minecraft:particle_type: {key}"))?;
    decode_options(particle_type, compound)
}

/// Vanilla `ParticleTypes.CODEC` encoding: `type` plus the options' fields.
/// `None` for options vanilla refuses to encode (an entity vibration target).
pub(crate) fn encode(particle: &ParticleData) -> Option<NbtCompound> {
    let mut encoded = encode_options(particle)?;
    encoded.insert("type", particle.particle_type().key.to_string());
    Some(encoded)
}

/// `type.codec().codec().parse(options)` for one particle type.
pub(crate) fn decode_options(
    particle_type: ParticleTypeRef,
    options: &NbtCompound,
) -> DecodeResult<ParticleData> {
    let key = particle_type.options_type_key();
    let data = if key == SimpleParticleOptions::TYPE_KEY {
        ParticleData::simple(particle_type)
    } else if key == BlockParticleOption::TYPE_KEY {
        let state = block_state(required(options, "block_state")?)?;
        ParticleData::new(particle_type, BlockParticleOption::new(state))
    } else if key == ColorParticleOption::TYPE_KEY {
        let color = argb_color(required(options, "color")?)?;
        ParticleData::new(particle_type, ColorParticleOption::new(color))
    } else if key == DustParticleOptions::TYPE_KEY {
        let color = rgb_color(required(options, "color")?)?;
        let scale = scale(required(options, "scale")?)?;
        ParticleData::new(particle_type, DustParticleOptions::new(color, scale))
    } else if key == DustColorTransitionOptions::TYPE_KEY {
        let from = rgb_color(required(options, "from_color")?)?;
        let to = rgb_color(required(options, "to_color")?)?;
        let scale = scale(required(options, "scale")?)?;
        ParticleData::new(
            particle_type,
            DustColorTransitionOptions::new(from, to, scale),
        )
    } else {
        return decode_other_options(particle_type, options);
    };
    Ok(data)
}

/// The option families past the colored ones.
fn decode_other_options(
    particle_type: ParticleTypeRef,
    options: &NbtCompound,
) -> DecodeResult<ParticleData> {
    let key = particle_type.options_type_key();
    let data = if key == GeyserParticleOptions::TYPE_KEY {
        let water_blocks = positive_int(required(options, "water_blocks")?)?;
        ParticleData::new(particle_type, GeyserParticleOptions::new(water_blocks))
    } else if key == GeyserBaseParticleOptions::TYPE_KEY {
        let water_blocks = positive_int(required(options, "water_blocks")?)?;
        let impulse = float(required(options, "burst_impulse_base")?)?;
        ParticleData::new(
            particle_type,
            GeyserBaseParticleOptions::new(water_blocks, impulse),
        )
    } else if key == PowerParticleOption::TYPE_KEY {
        let power = optional(options, "power", float)?.unwrap_or(1.0);
        ParticleData::new(particle_type, PowerParticleOption::new(power))
    } else if key == SpellParticleOption::TYPE_KEY {
        let color = optional(options, "color", rgb_color)?.unwrap_or(RgbColor::new(-1));
        let power = optional(options, "power", float)?.unwrap_or(1.0);
        ParticleData::new(particle_type, SpellParticleOption::new(color, power))
    } else if key == ItemParticleOption::TYPE_KEY {
        let item = item_template(required(options, "item")?)?;
        ParticleData::new(particle_type, ItemParticleOption::new(item))
    } else if key == SculkChargeParticleOptions::TYPE_KEY {
        let roll = float(required(options, "roll")?)?;
        ParticleData::new(particle_type, SculkChargeParticleOptions::new(roll))
    } else if key == ShriekParticleOption::TYPE_KEY {
        let delay = int(required(options, "delay")?)?;
        ParticleData::new(particle_type, ShriekParticleOption::new(delay))
    } else if key == TrailParticleOption::TYPE_KEY {
        let target = vec3(required(options, "target")?)?;
        let color = rgb_color(required(options, "color")?)?;
        let duration = positive_int(required(options, "duration")?)?;
        ParticleData::new(
            particle_type,
            TrailParticleOption::new(target, color, duration),
        )
    } else if key == VibrationParticleOption::TYPE_KEY {
        let destination = block_position_source(required(options, "destination")?)?;
        let arrival = int(required(options, "arrival_in_ticks")?)?;
        ParticleData::new(
            particle_type,
            VibrationParticleOption::new(destination, arrival),
        )
    } else {
        return Err(format!(
            "Particle {} has options with no codec",
            particle_type.key
        ));
    };
    Ok(data)
}

/// The options' fields alone, as their type's codec encodes them.
fn encode_options(particle: &ParticleData) -> Option<NbtCompound> {
    let mut encoded = NbtCompound::new();
    if let Some(block) = particle.downcast_ref::<BlockParticleOption>() {
        encoded.insert("block_state", block_state_nbt::save(block.state()));
    } else if let Some(color) = particle.downcast_ref::<ColorParticleOption>() {
        encoded.insert("color", color.color().raw());
    } else if let Some(dust) = particle.downcast_ref::<DustParticleOptions>() {
        encoded.insert("color", dust.color().raw());
        encoded.insert("scale", dust.scale());
    } else if let Some(dust) = particle.downcast_ref::<DustColorTransitionOptions>() {
        encoded.insert("from_color", dust.source_color().raw());
        encoded.insert("to_color", dust.target_color().raw());
        encoded.insert("scale", dust.scale());
    } else if let Some(geyser) = particle.downcast_ref::<GeyserParticleOptions>() {
        encoded.insert("water_blocks", geyser.water_blocks());
    } else if let Some(geyser) = particle.downcast_ref::<GeyserBaseParticleOptions>() {
        encoded.insert("water_blocks", geyser.water_blocks());
        encoded.insert("burst_impulse_base", geyser.burst_impulse_base());
    } else if let Some(power) = particle.downcast_ref::<PowerParticleOption>() {
        encoded.insert("power", power.power());
    } else if let Some(spell) = particle.downcast_ref::<SpellParticleOption>() {
        encoded.insert("color", spell.color().raw());
        encoded.insert("power", spell.power());
    } else {
        encode_other_options(particle, &mut encoded)?;
    }
    Some(encoded)
}

/// The option families past the colored and numeric ones.
fn encode_other_options(particle: &ParticleData, encoded: &mut NbtCompound) -> Option<()> {
    if let Some(item) = particle.downcast_ref::<ItemParticleOption>() {
        encoded.insert("item", item.item().clone().to_nbt_tag());
    } else if let Some(charge) = particle.downcast_ref::<SculkChargeParticleOptions>() {
        encoded.insert("roll", charge.roll());
    } else if let Some(shriek) = particle.downcast_ref::<ShriekParticleOption>() {
        encoded.insert("delay", shriek.delay());
    } else if let Some(trail) = particle.downcast_ref::<TrailParticleOption>() {
        let target = trail.target();
        encoded.insert(
            "target",
            NbtList::Double(vec![target.x, target.y, target.z]),
        );
        encoded.insert("color", trail.color().raw());
        encoded.insert("duration", trail.duration());
    } else if let Some(vibration) = particle.downcast_ref::<VibrationParticleOption>() {
        let source = vibration
            .destination()
            .downcast_ref::<BlockPositionSource>()?;
        let pos = source.pos();
        let mut destination = NbtCompound::new();
        destination.insert("type", vanilla_position_source_types::BLOCK.key.to_string());
        destination.insert("pos", NbtTag::IntArray(vec![pos.x(), pos.y(), pos.z()]));
        encoded.insert("destination", destination);
        encoded.insert("arrival_in_ticks", vibration.arrival_in_ticks());
    }
    Some(())
}

fn snbt(tag: &NbtTag) -> String {
    to_canonical_snbt(tag).unwrap_or_default()
}

/// Vanilla `MapCodec` "No key" error.
fn missing_key(key: &str, compound: &NbtCompound) -> String {
    format!(
        "No key {key} in MapLike[{}]",
        snbt(&NbtTag::Compound(compound.clone()))
    )
}

fn required<'a>(compound: &'a NbtCompound, key: &str) -> DecodeResult<&'a NbtTag> {
    compound.get(key).ok_or_else(|| missing_key(key, compound))
}

/// `optionalFieldOf`: absent is `None`, present must decode.
fn optional<T>(
    compound: &NbtCompound,
    key: &str,
    decode: impl FnOnce(&NbtTag) -> DecodeResult<T>,
) -> DecodeResult<Option<T>> {
    compound.get(key).map(decode).transpose()
}

fn not_a_number(tag: &NbtTag) -> String {
    format!("Not a number: {}", snbt(tag))
}

/// Vanilla `Codec.INT`.
fn int(tag: &NbtTag) -> DecodeResult<i32> {
    tag.codec_i32().ok_or_else(|| not_a_number(tag))
}

/// Vanilla `Codec.FLOAT`.
fn float(tag: &NbtTag) -> DecodeResult<f32> {
    tag.codec_f32().ok_or_else(|| not_a_number(tag))
}

/// Vanilla `ExtraCodecs.POSITIVE_INT`.
fn positive_int(tag: &NbtTag) -> DecodeResult<i32> {
    let value = int(tag)?;
    if value >= 1 {
        Ok(value)
    } else {
        Err(format!("Value must be positive: {value}"))
    }
}

/// Vanilla `ScalableParticleOptionsBase.SCALE`.
fn scale(tag: &NbtTag) -> DecodeResult<f32> {
    let value = float(tag)?;
    if (MIN_SCALE..=MAX_SCALE).contains(&value) {
        Ok(value)
    } else {
        Err(format!("Value must be within range [0.01;4.0]: {value}"))
    }
}

/// `Codec.FLOAT.listOf()` or `Codec.DOUBLE.listOf()` with `Util.fixedSize`.
fn fixed_numbers<const N: usize>(tag: &NbtTag) -> DecodeResult<[f64; N]> {
    let values = nbt_collection_values(tag).ok_or_else(|| format!("Not a list: {}", snbt(tag)))?;
    if values.len() != N {
        return Err(format!(
            "Input is not a list of {N} elements, got {}",
            values.len()
        ));
    }
    let mut numbers = [0.0; N];
    for (number, value) in numbers.iter_mut().zip(&values) {
        *number = value.codec_f64().ok_or_else(|| not_a_number(value))?;
    }
    Ok(numbers)
}

/// Vanilla `ARGB.as8BitChannel`.
fn channel(value: f64) -> i32 {
    ((value as f32) * 255.0).floor() as i32 & 0xFF
}

/// Vanilla `ExtraCodecs.RGB_COLOR_CODEC`: an int, or three float channels.
fn rgb_color(tag: &NbtTag) -> DecodeResult<RgbColor> {
    if let Some(raw) = tag.codec_i32() {
        return Ok(RgbColor::new(raw));
    }
    let [red, green, blue] = fixed_numbers::<3>(tag)?;
    Ok(RgbColor::new(
        (0xFF << 24) | (channel(red) << 16) | (channel(green) << 8) | channel(blue),
    ))
}

/// Vanilla `ExtraCodecs.ARGB_COLOR_CODEC`: an int, or `[r, g, b, a]` floats.
fn argb_color(tag: &NbtTag) -> DecodeResult<ArgbColor> {
    if let Some(raw) = tag.codec_i32() {
        return Ok(ArgbColor::new(raw));
    }
    let [red, green, blue, alpha] = fixed_numbers::<4>(tag)?;
    Ok(ArgbColor::new(
        (channel(alpha) << 24) | (channel(red) << 16) | (channel(green) << 8) | channel(blue),
    ))
}

/// Vanilla `Vec3.CODEC`.
fn vec3(tag: &NbtTag) -> DecodeResult<DVec3> {
    let [x, y, z] = fixed_numbers::<3>(tag)?;
    Ok(DVec3::new(x, y, z))
}

/// Runs `read` on a borrowed copy of `tag`, for codecs written over the
/// borrowed view.
fn with_borrowed<T>(tag: &NbtTag, read: impl FnOnce(BorrowedNbtTag<'_, '_>) -> T) -> Option<T> {
    let mut bytes = Vec::new();
    tag.write(&mut bytes);
    let borrowed = read_tag(&mut Cursor::new(bytes.as_slice())).ok()?;
    Some(read(borrowed.as_tag()))
}

/// `BlockParticleOption`'s state codec: `BlockState.CODEC`, or a block id for
/// its default state.
fn block_state(tag: &NbtTag) -> DecodeResult<BlockStateId> {
    with_borrowed(tag, block_state_nbt::load)
        .flatten()
        .ok_or_else(|| format!("Not a valid block state: {}", snbt(tag)))
}

/// Vanilla `ItemStackTemplate.CODEC`.
fn item_template(tag: &NbtTag) -> DecodeResult<ItemStackTemplate> {
    with_borrowed(tag, ItemStackTemplate::from_nbt_tag)
        .flatten()
        .ok_or_else(|| format!("Not a valid item: {}", snbt(tag)))
}

/// `VibrationParticleOption.SAFE_POSITION_SOURCE_CODEC`: a block position
/// source; entity sources are refused.
fn block_position_source(tag: &NbtTag) -> DecodeResult<PositionSource> {
    let NbtTag::Compound(compound) = tag else {
        return Err(format!("Not a map: {}", snbt(tag)));
    };
    let kind = compound
        .string("type")
        .ok_or_else(|| missing_key("type", compound))?
        .to_str();
    let kind = kind
        .parse::<Identifier>()
        .map_err(|_| format!("Not a valid resource location: {kind}"))?;
    if kind == vanilla_position_source_types::ENTITY.key {
        return Err(String::from("Entity position sources are not allowed"));
    }
    if kind != vanilla_position_source_types::BLOCK.key {
        return Err(format!(
            "Unknown registry key in minecraft:position_source_type: {kind}"
        ));
    }
    let pos = required(compound, "pos")?;
    let values = nbt_collection_values(pos).ok_or_else(|| format!("Not a list: {}", snbt(pos)))?;
    let [x, y, z] = <[NbtTag; 3]>::try_from(values).map_err(|values| {
        format!(
            "Input is not a list of 3 ints, got {} elements",
            values.len()
        )
    })?;
    let pos = BlockPos::new(int(&x)?, int(&y)?, int(&z)?);
    Ok(PositionSource::new(
        &vanilla_position_source_types::BLOCK,
        BlockPositionSource::new(pos),
    ))
}

#[cfg(test)]
mod tests {
    use steel_registry::{
        init_vanilla_registry, vanilla_blocks, vanilla_items, vanilla_particle_types,
    };
    use steel_utils::nbt::parse_snbt;

    use super::*;

    fn parse(snbt: &str) -> DecodeResult<ParticleData> {
        init_vanilla_registry();
        decode(&parse_snbt(snbt).expect("test SNBT should parse"))
    }

    fn round_trip(particle: &ParticleData) {
        let encoded = encode(particle).expect("particle should encode");
        let decoded = decode(&NbtTag::Compound(encoded)).expect("encoded particle should decode");
        assert_eq!(&decoded, particle);
    }

    #[test]
    fn simple_particles_need_only_their_type() {
        let particle = parse("{type:\"minecraft:flame\"}").expect("flame");
        assert_eq!(
            particle.particle_type().key,
            vanilla_particle_types::FLAME.key
        );
        round_trip(&particle);
    }

    #[test]
    fn dust_reads_float_channels_and_checks_the_scale() {
        let particle = parse("{type:\"dust\",color:[1.0f,0.5f,0.0f],scale:2.0f}").expect("dust");
        let dust = particle
            .downcast_ref::<DustParticleOptions>()
            .expect("dust options");
        assert_eq!(dust.color().raw() & 0xFF_FFFF, 0xFF_7F00);
        assert!((dust.scale() - 2.0).abs() < f32::EPSILON);
        round_trip(&particle);

        let error = parse("{type:\"dust\",color:16711680,scale:5.0f}").expect_err("scale");
        assert!(error.contains("[0.01;4.0]"), "{error}");
    }

    #[test]
    fn block_and_item_particles_use_their_codecs() {
        let particle = parse("{type:\"block\",block_state:\"minecraft:stone\"}").expect("block");
        let block = particle
            .downcast_ref::<BlockParticleOption>()
            .expect("block options");
        assert_eq!(block.state(), vanilla_blocks::STONE.default_state());
        round_trip(&particle);

        let particle = parse("{type:\"item\",item:\"minecraft:diamond\"}").expect("item");
        let item = particle
            .downcast_ref::<ItemParticleOption>()
            .expect("item options");
        assert!(item.item().create().is(&vanilla_items::DIAMOND));
        round_trip(&particle);
    }

    #[test]
    fn missing_fields_and_entity_targets_are_errors() {
        let error = parse("{type:\"block\"}").expect_err("missing block_state");
        assert!(error.starts_with("No key block_state"), "{error}");
        let error = parse(
            "{type:\"vibration\",destination:{type:\"entity\",source_entity:[I;1,2,3,4]},arrival_in_ticks:5}",
        )
        .expect_err("entity source");
        assert_eq!(error, "Entity position sources are not allowed");
    }

    #[test]
    fn optional_fields_take_vanilla_defaults() {
        let particle = parse("{type:\"effect\"}").expect("effect");
        let spell = particle
            .downcast_ref::<SpellParticleOption>()
            .expect("spell options");
        assert_eq!(spell.color().raw(), -1);
        assert!((spell.power() - 1.0).abs() < f32::EPSILON);

        let particle = parse(
            "{type:\"vibration\",destination:{type:\"block\",pos:[I;1,2,3]},arrival_in_ticks:20}",
        )
        .expect("vibration");
        round_trip(&particle);
        let particle =
            parse("{type:\"trail\",target:[1.0d,2.0d,3.0d],color:255,duration:4}").expect("trail");
        round_trip(&particle);
    }
}
