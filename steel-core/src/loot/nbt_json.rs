//! NBT to JSON conversion, vanilla `NbtOps.convertTo(JsonOps.INSTANCE, ...)`,
//! so an inline SNBT command argument decodes through the same codecs as a
//! data pack's JSON file.

use serde_json::{Map, Number, Value};
use simdnbt::owned::{NbtList, NbtTag};

/// Converts an NBT tag into JSON.
///
/// Numbers keep their value; bytes stay numbers, as `NbtOps` has no boolean
/// tag and the codecs read a number where they want a boolean. Arrays and
/// lists become JSON arrays, compounds objects. A non-finite float or double
/// has no JSON form and becomes `null`, which no codec accepts.
#[must_use]
pub(crate) fn nbt_to_json(tag: &NbtTag) -> Value {
    match tag {
        NbtTag::Byte(value) => Value::from(*value),
        NbtTag::Short(value) => Value::from(*value),
        NbtTag::Int(value) => Value::from(*value),
        NbtTag::Long(value) => Value::from(*value),
        NbtTag::Float(value) => float(f64::from(*value)),
        NbtTag::Double(value) => float(*value),
        NbtTag::ByteArray(values) => values
            .iter()
            .map(|value| Value::from(value.cast_signed()))
            .collect(),
        NbtTag::String(value) => Value::String(value.to_str().into_owned()),
        NbtTag::List(list) => list_to_json(list),
        NbtTag::Compound(compound) => Value::Object(
            compound
                .iter()
                .map(|(key, value)| (key.to_str().into_owned(), nbt_to_json(value)))
                .collect::<Map<_, _>>(),
        ),
        NbtTag::IntArray(values) => values.iter().map(|value| Value::from(*value)).collect(),
        NbtTag::LongArray(values) => values.iter().map(|value| Value::from(*value)).collect(),
    }
}

fn float(value: f64) -> Value {
    Number::from_f64(value).map_or(Value::Null, Value::Number)
}

fn list_to_json(list: &NbtList) -> Value {
    Value::Array(list.as_nbt_tags().iter().map(nbt_to_json).collect())
}
