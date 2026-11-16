//! Small readers over JSON objects that fail with a message an author can act on.

use glam::DVec3;
use serde_json::{Map, Value};

use super::{ease::Ease, time};

/// A JSON object.
pub type Object = Map<String, Value>;

/// The value under `key`, unless it is missing or `null`.
#[must_use]
pub fn get<'a>(object: &'a Object, key: &str) -> Option<&'a Value> {
    object.get(key).filter(|value| !value.is_null())
}

/// Whether `key` is there and not `null`.
#[must_use]
pub fn has(object: &Object, key: &str) -> bool {
    get(object, key).is_some()
}

fn missing(key: &str) -> String {
    format!("нет поля \"{key}\"")
}

fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// A required string.
pub fn string(object: &Object, key: &str) -> Result<String, String> {
    get(object, key).map(text).ok_or_else(|| missing(key))
}

/// A string, or `fallback`.
#[must_use]
pub fn string_or(object: &Object, key: &str, fallback: &str) -> String {
    get(object, key).map_or_else(|| fallback.to_owned(), text)
}

/// A number, or `fallback`.
pub fn number(object: &Object, key: &str, fallback: f64) -> Result<f64, String> {
    match get(object, key) {
        None => Ok(fallback),
        Some(value) => as_number(value).ok_or_else(|| format!("\"{key}\": ожидаю число")),
    }
}

fn as_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// A number as `f32`, or `fallback`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "angles, sizes and volumes fit f32"
)]
pub fn float(object: &Object, key: &str, fallback: f32) -> Result<f32, String> {
    number(object, key, f64::from(fallback)).map(|value| value as f32)
}

/// A whole number, or `fallback`.
#[expect(clippy::cast_possible_truncation, reason = "clamped to i32 first")]
pub fn int(object: &Object, key: &str, fallback: i32) -> Result<i32, String> {
    number(object, key, f64::from(fallback)).map(|value| {
        value
            .round()
            .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
    })
}

/// A vector as `f32`s.
#[must_use]
#[expect(clippy::cast_possible_truncation, reason = "sizes and offsets fit f32")]
pub const fn narrow(vector: DVec3) -> glam::Vec3 {
    glam::Vec3::new(vector.x as f32, vector.y as f32, vector.z as f32)
}

/// The object under `key`, if there.
pub fn object<'a>(object: &'a Object, key: &str) -> Result<Option<&'a Object>, String> {
    match get(object, key) {
        None => Ok(None),
        Some(Value::Object(inner)) => Ok(Some(inner)),
        Some(_) => Err(format!("\"{key}\": ожидаю объект")),
    }
}

/// A flag, or `fallback`.
pub fn boolean(object: &Object, key: &str, fallback: bool) -> Result<bool, String> {
    match get(object, key) {
        None => Ok(fallback),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(format!("\"{key}\": ожидаю true или false")),
    }
}

/// A time in ticks, or `fallback`.
pub fn ticks(object: &Object, key: &str, fallback: u32) -> Result<u32, String> {
    get(object, key).map_or(Ok(fallback), time::from_json)
}

/// The curve under `"ease"`, or `fallback`.
pub fn ease(object: &Object, fallback: Ease) -> Result<Ease, String> {
    match get(object, "ease") {
        None => Ok(fallback),
        Some(value) => Ease::parse(&text(value)),
    }
}

/// `[x, y, z]`.
pub fn vec(value: &Value) -> Result<DVec3, String> {
    let wrong = || format!("ожидаю [x, y, z], а не {value}");
    let Value::Array(items) = value else {
        return Err(wrong());
    };
    let [x, y, z] = items.as_slice() else {
        return Err(wrong());
    };
    match (as_number(x), as_number(y), as_number(z)) {
        (Some(x), Some(y), Some(z)) => Ok(DVec3::new(x, y, z)),
        _ => Err(wrong()),
    }
}

/// A required `[x, y, z]`.
pub fn vec_at(object: &Object, key: &str) -> Result<DVec3, String> {
    get(object, key).map_or_else(|| Err(missing(key)), vec)
}

/// `[x, y, z]`, if there.
pub fn vec_opt(object: &Object, key: &str) -> Result<Option<DVec3>, String> {
    get(object, key).map(vec).transpose()
}

/// A size: a number gives the same value on all three axes.
pub fn scale(object: &Object, key: &str) -> Result<Option<DVec3>, String> {
    match get(object, key) {
        None => Ok(None),
        Some(value @ Value::Array(_)) => vec(value).map(Some),
        Some(value) => as_number(value)
            .map(|size| Some(DVec3::splat(size)))
            .ok_or_else(|| format!("\"{key}\": ожидаю число или [x, y, z]")),
    }
}

/// A number rounded to three places, for files people read.
#[must_use]
pub fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// A vector as `[x, y, z]`, rounded.
#[must_use]
pub fn array(vector: DVec3) -> Value {
    Value::from(vec![round(vector.x), round(vector.y), round(vector.z)])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn object(value: Value) -> Object {
        match value {
            Value::Object(object) => object,
            _ => Object::new(),
        }
    }

    #[test]
    fn a_missing_field_is_named() {
        let empty = Object::new();
        assert_eq!(string(&empty, "do"), Err("нет поля \"do\"".to_owned()));
        assert_eq!(vec_at(&empty, "pos"), Err("нет поля \"pos\"".to_owned()));
    }

    #[test]
    fn null_counts_as_missing() {
        let it = object(json!({"sub": null}));
        assert!(!has(&it, "sub"));
        assert_eq!(string_or(&it, "sub", "x"), "x");
    }

    #[test]
    fn a_size_is_a_number_or_three() {
        let it = object(json!({"a": 6, "b": [1, 2, 3]}));
        assert_eq!(scale(&it, "a"), Ok(Some(DVec3::splat(6.0))));
        assert_eq!(scale(&it, "b"), Ok(Some(DVec3::new(1.0, 2.0, 3.0))));
        assert_eq!(scale(&it, "c"), Ok(None));
    }

    #[test]
    fn a_place_needs_three_numbers() {
        assert!(vec(&json!([1, 2])).is_err());
        assert!(vec(&json!([1, "x", 3])).is_err());
        assert_eq!(vec(&json!([1, 2.5, -3])), Ok(DVec3::new(1.0, 2.5, -3.0)));
    }
}
