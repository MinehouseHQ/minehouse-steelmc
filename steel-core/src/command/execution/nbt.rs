//! NBT path and SNBT command arguments.

use simdnbt::owned::{NbtCompound, NbtTag};
use steel_utils::nbt::{
    NbtPath, SnbtError, parse_nbt_path_argument as parse_path, parse_snbt_argument,
    parse_snbt_compound_argument,
};
use text_components::TextComponent;

use crate::command::brigadier::{CommandSyntaxError, CommandSyntaxErrorKind, StringReader};

pub(super) fn parse_nbt_path(reader: &mut StringReader<'_>) -> Result<NbtPath, CommandSyntaxError> {
    match parse_path(reader.remaining()) {
        Ok((path, consumed)) => {
            if !reader.advance_bytes(consumed) {
                return Err(dynamic_error(reader, "Invalid NBT path cursor"));
            }
            Ok(path)
        }
        Err(error) => {
            if !reader.advance_bytes(error.cursor()) {
                return Err(dynamic_error(reader, "Invalid NBT path cursor"));
            }
            Err(dynamic_error(reader, error.component()))
        }
    }
}

/// Vanilla `NbtTagArgument`: one SNBT value.
pub(super) fn parse_nbt_tag(reader: &mut StringReader<'_>) -> Result<NbtTag, CommandSyntaxError> {
    consume_snbt(reader, parse_snbt_argument(reader.remaining()))
}

/// Vanilla `CompoundTagArgument`: one SNBT compound.
pub(super) fn parse_compound_tag(
    reader: &mut StringReader<'_>,
) -> Result<NbtCompound, CommandSyntaxError> {
    consume_snbt(reader, parse_snbt_compound_argument(reader.remaining()))
}

fn consume_snbt<T>(
    reader: &mut StringReader<'_>,
    parsed: Result<(T, usize), SnbtError>,
) -> Result<T, CommandSyntaxError> {
    match parsed {
        Ok((value, consumed)) => {
            if !reader.advance_bytes(consumed) {
                return Err(dynamic_error(reader, "Invalid SNBT cursor"));
            }
            Ok(value)
        }
        Err(error) => {
            if !reader.advance_bytes(error.cursor()) {
                return Err(dynamic_error(reader, "Invalid SNBT cursor"));
            }
            Err(dynamic_error(reader, error.component()))
        }
    }
}

fn dynamic_error(
    reader: &StringReader<'_>,
    message: impl Into<TextComponent>,
) -> CommandSyntaxError {
    reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(message.into())))
}
