use serde_json::json;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_utils::Identifier;

use super::level_value::LevelBasedValue;
use super::{
    ContextFloatProvider, ContextIntProvider, Holder, LootCondition, LootContext, LootRegistries,
    LootRegistryKind, from_json, nbt_to_json,
};
use crate::test_support::test_world;

fn int(value: serde_json::Value) -> ContextIntProvider {
    from_json(value).expect("int provider decodes")
}

fn float(value: serde_json::Value) -> ContextFloatProvider {
    from_json(value).expect("float provider decodes")
}

#[test]
fn vanilla_registries_hold_every_built_in_entry() {
    let registries = LootRegistries::vanilla();
    assert_eq!(registries.ids(LootRegistryKind::Predicate).len(), 3);
    assert_eq!(registries.ids(LootRegistryKind::IntProvider).len(), 26);
    assert_eq!(registries.ids(LootRegistryKind::FloatProvider).len(), 4);
    assert!(registries.contains(
        LootRegistryKind::Predicate,
        &Identifier::vanilla_static("block/fast_cooking")
    ));
}

#[test]
fn int_providers_follow_java_arithmetic() {
    let world = test_world();
    let registries = LootRegistries::vanilla();
    let mut ctx = LootContext::new(world);
    ctx.registries = Some(&registries);
    let eval = |value| int(value).get_int_unsafe(&ctx);

    assert_eq!(eval(json!(5)), Ok(5));
    assert_eq!(eval(json!({"type": "constant", "value": 3})), Ok(3));
    assert_eq!(eval(json!({"type": "mod", "left": -7, "right": 3})), Ok(-1));
    assert_eq!(
        eval(json!({"type": "floor_mod", "left": 7, "right": -3})),
        Ok(-2)
    );
    assert_eq!(
        eval(json!({"type": "div", "left": i32::MIN, "right": -1})),
        Ok(i32::MIN)
    );
    assert_eq!(eval(json!({"type": "avg", "inputs": [1, 2]})), Ok(1));
    assert_eq!(eval(json!({"type": "max", "inputs": []})), Ok(i32::MIN));
    assert!(eval(json!({"type": "avg", "inputs": []})).is_err());
    assert!(eval(json!({"type": "negate", "input": i32::MIN})).is_err());
    assert!(eval(json!({"type": "abs", "input": i32::MIN})).is_err());
    assert!(eval(json!({"type": "from_float", "input": 3.0e10})).is_err());
    assert_eq!(eval(json!({"type": "from_float", "input": -2.9})), Ok(-2));
    assert_eq!(
        eval(json!({"type": "binomial", "n": 4, "p": 1.0})),
        Ok(4),
        "every trial passes with p = 1"
    );
    let reference: Holder<ContextIntProvider> =
        from_json(json!("minecraft:cooking/time_boats")).expect("reference decodes");
    let boats = reference.value(&ctx).expect("built-in entry");
    assert_eq!(boats.get_int_unsafe(&ctx), Ok(1200));
    // `getInt` hides errors as 0.
    assert_eq!(
        int(json!({"type": "div", "left": 1, "right": 0})).get_int(&ctx),
        0
    );
}

#[test]
fn float_providers_follow_vanilla_math() {
    let world = test_world();
    let ctx = LootContext::new(world);
    let eval = |value| float(value).get_float_unsafe(&ctx).expect("finite or NaN");

    assert_eq!(eval(json!({"type": "round", "input": 2.5})), 3.0);
    assert_eq!(eval(json!({"type": "round", "input": -2.5})), -2.0);
    assert_eq!(eval(json!({"type": "truncate", "input": -2.7})), -2.0);
    assert_eq!(
        eval(json!({"type": "ceil", "input": 1.0e20})),
        i32::MAX as f32
    );
    assert_eq!(eval(json!({"type": "length", "inputs": [3, 4]})), 5.0);
    assert_eq!(eval(json!({"type": "sin", "input": 0})), 0.0);
    assert_eq!(eval(json!({"type": "cos", "input": 0})), 1.0);
    assert!(eval(json!({"type": "mod", "left": 1, "right": 0})).is_nan());
    assert!(eval(json!({"type": "pow", "base": 0, "exponent": 0})).is_nan());
    assert_eq!(
        float(json!({"type": "sqrt", "input": -1})).get_float(&ctx),
        0.0
    );
    assert_eq!(
        eval(
            json!({"type": "enchantment_level", "amount": {"type": "linear", "base": 2, "per_level_above_first": 3}})
        ),
        -1.0,
        "no enchantment level reads as level 0"
    );
}

#[test]
fn conditions_decode_holders_and_test_ranges() {
    let world = test_world();
    let registries = LootRegistries::vanilla();
    let mut ctx = LootContext::new(world);
    ctx.registries = Some(&registries);
    let test = |value| {
        from_json::<LootCondition>(value)
            .expect("condition decodes")
            .test(&ctx)
    };

    assert!(test(
        json!({"type": "int_value_check", "value": 5, "test": {"min": 1}})
    ));
    assert!(!test(
        json!({"type": "int_value_check", "value": 5, "test": 6})
    ));
    assert!(test(
        json!({"type": "float_value_check", "value": 0.5, "test": {"max": 0.5}})
    ));
    assert!(test(
        json!({"type": "inverted", "term": "minecraft:tool/can_silk_touch"})
    ));
    assert!(test(
        json!({"type": "any_of", "terms": [{"type": "killed_by_player"}, {"type": "survives_explosion"}]})
    ));
    assert!(!test(
        json!({"type": "all_of", "terms": [{"type": "killed_by_player"}]})
    ));
    assert!(!test(
        json!({"type": "enchantment_active_check", "active": true})
    ));
    assert!(!test(json!({"type": "random_chance", "chance": 0})));
    assert!(test(json!({"type": "random_chance", "chance": 1})));
    assert!(
        from_json::<LootCondition>(
            json!({"type": "table_bonus", "enchantment": "minecraft:fortune", "chances": []})
        )
        .is_err()
    );
    assert!(
        from_json::<LootCondition>(
            json!({"type": "time_check", "clock": "minecraft:nope", "value": 1})
        )
        .is_err()
    );
}

#[test]
fn level_based_values_match_vanilla() {
    let lookup: LevelBasedValue = from_json(json!({
        "type": "lookup",
        "values": [1.0, 2.0],
        "fallback": {"type": "levels_squared", "added": 1.0}
    }))
    .expect("lookup decodes");
    assert_eq!(lookup.calculate(2), 2.0);
    assert_eq!(lookup.calculate(3), 10.0);
    let clamped: LevelBasedValue = from_json(json!({
        "type": "clamped", "value": {"type": "fraction", "numerator": 10, "denominator": 0}, "min": 1, "max": 2
    }))
    .expect("clamped decodes");
    assert_eq!(clamped.calculate(1), 1.0);
    assert!(
        from_json::<LevelBasedValue>(json!({"type": "clamped", "value": 1, "min": 2, "max": 2}))
            .is_err()
    );
}

#[test]
fn nbt_converts_to_codec_json() {
    let mut compound = NbtCompound::new();
    compound.insert("flag", NbtTag::Byte(1));
    compound.insert("ratio", NbtTag::Float(0.5));
    compound.insert("ids", NbtTag::List(NbtList::Int(vec![1, 2])));
    compound.insert("bytes", NbtTag::ByteArray(vec![255]));
    assert_eq!(
        nbt_to_json(&NbtTag::Compound(compound)),
        json!({"flag": 1, "ratio": 0.5, "ids": [1, 2], "bytes": [-1]})
    );
}
