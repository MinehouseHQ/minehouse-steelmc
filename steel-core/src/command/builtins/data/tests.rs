use std::sync::Arc;

use steel_registry::{init_vanilla_registry, vanilla_blocks};
use steel_utils::{
    BlockPos, ChunkPos,
    nbt::{parse_nbt_path, parse_snbt, parse_snbt_compound},
    types::UpdateFlags,
};

use super::*;
use crate::{
    behavior::init_behaviors,
    block_entity::init_block_entities,
    command::storage::CommandStorage,
    test_support::{fresh_test_world, insert_ready_full_chunk},
};

fn compound(snbt: &str) -> NbtCompound {
    parse_snbt_compound(snbt).expect("test SNBT must parse")
}

fn tag(snbt: &str) -> NbtTag {
    parse_snbt(snbt).expect("test SNBT must parse")
}

fn path(input: &str) -> NbtPath {
    parse_nbt_path(input).expect("test path must parse")
}

fn assert_data(actual: &NbtTag, expected: &str) {
    let NbtTag::Compound(actual) = actual else {
        panic!("data must stay a compound");
    };
    assert!(
        nbt_compounds_equal(actual, &compound(expected)),
        "got {}",
        snbt(&NbtTag::Compound(actual.clone()))
    );
}

/// Runs one manipulator like `data modify` does and returns its result.
fn modify(
    data: &mut NbtTag,
    operation: Operation,
    index: i32,
    target: &str,
    values: &[&str],
) -> i32 {
    let values: Vec<_> = values.iter().map(|value| tag(value)).collect();
    apply_operation(operation, index, data, &path(target), &values).expect("the operation applies")
}

#[test]
fn merge_combines_nested_compounds_and_rejects_no_change() {
    let old = compound("{a:1,nested:{x:1b}}");
    let merged = merged_data(&old, &compound("{b:2,nested:{y:2b}}")).expect("data changes");
    assert!(nbt_compounds_equal(
        &merged,
        &compound("{a:1,b:2,nested:{x:1b,y:2b}}")
    ));
    // Merging what is already there is vanilla's "Nothing changed".
    assert!(merged_data(&merged, &compound("{a:1,nested:{x:1b}}")).is_err());
}

#[test]
fn list_manipulators_insert_where_vanilla_does() {
    let mut data = NbtTag::Compound(compound("{list:[1,2]}"));
    assert_eq!(modify(&mut data, Operation::Append, 0, "list", &["3"]), 1);
    assert_eq!(modify(&mut data, Operation::Prepend, 0, "list", &["0"]), 1);
    assert_eq!(modify(&mut data, Operation::Insert, -2, "list", &["9"]), 1);
    // Like vanilla, -1 is the end, so -2 is before the last element.
    assert_data(&data, "{list:[0,1,2,9,3]}");
    // An index past the end fails.
    let values = [tag("5")];
    assert!(apply_operation(Operation::Insert, 9, &mut data, &path("list"), &values).is_err());
    // Lists take mixed element types since 1.21.5, arrays only their own.
    assert_eq!(
        modify(&mut data, Operation::Append, 0, "list", &["\"x\""]),
        1
    );
    // A byte array takes numbers and refuses a string, which `data` reports as no change.
    let mut bytes = NbtTag::Compound(compound("{bytes:[B;1b]}"));
    assert_eq!(
        modify(&mut bytes, Operation::Append, 0, "bytes", &["2s"]),
        1
    );
    assert_eq!(
        modify(&mut bytes, Operation::Append, 0, "bytes", &["\"x\""]),
        0
    );
    assert_data(&bytes, "{bytes:[B;1b,2b]}");
}

#[test]
fn set_uses_the_last_value_and_creates_missing_parents() {
    let mut data = NbtTag::Compound(compound("{}"));
    assert_eq!(modify(&mut data, Operation::Set, 0, "a.b", &["1", "2"]), 1);
    assert_data(&data, "{a:{b:2}}");
    // Setting the same value again changes nothing.
    assert_eq!(modify(&mut data, Operation::Set, 0, "a.b", &["2"]), 0);
}

#[test]
fn merge_manipulator_counts_changed_targets_and_needs_compounds() {
    let mut data = NbtTag::Compound(compound("{items:[{id:a},{id:b,count:2}]}"));
    let changed = modify(
        &mut data,
        Operation::Merge,
        0,
        "items[]",
        &["{count:2}", "{tag:1b}"],
    );
    assert_eq!(changed, 2);
    assert_data(
        &data,
        "{items:[{id:a,count:2,tag:1b},{id:b,count:2,tag:1b}]}",
    );
    // A missing compound is created to merge into.
    assert_eq!(
        modify(&mut data, Operation::Merge, 0, "fresh", &["{x:1}"]),
        1
    );
    assert_data(
        &data,
        "{items:[{id:a,count:2,tag:1b},{id:b,count:2,tag:1b}],fresh:{x:1}}",
    );
    let values = [tag("1")];
    assert!(apply_operation(Operation::Merge, 0, &mut data, &path("fresh"), &values).is_err());
    let values = [tag("{y:1}")];
    assert!(
        apply_operation(
            Operation::Merge,
            0,
            &mut data,
            &path("items[0].id"),
            &values
        )
        .is_err()
    );
}

#[test]
fn string_sources_follow_java_substring() {
    assert_eq!(as_text(&tag("\"hello\"")).ok().as_deref(), Some("hello"));
    assert_eq!(as_text(&tag("1.5f")).ok().as_deref(), Some("1.5f"));
    assert!(as_text(&tag("{}")).is_err());

    assert_eq!(substring("hello", 1, Some(3)).ok().as_deref(), Some("el"));
    assert_eq!(substring("hello", -3, None).ok().as_deref(), Some("llo"));
    assert_eq!(
        substring("hello", 0, Some(-1)).ok().as_deref(),
        Some("hell")
    );
    // Offsets count UTF-16 code units, as Java's String does.
    assert_eq!(substring("a😀b", 1, Some(3)).ok().as_deref(), Some("😀"));
    assert_eq!(substring("a😀b", 3, None).ok().as_deref(), Some("b"));
    assert!(substring("hello", 4, Some(2)).is_err());
    assert!(substring("hello", 0, Some(9)).is_err());
    assert!(substring("hello", -9, None).is_err());
}

#[test]
fn get_results_match_vanilla() {
    assert_eq!(tag_result(&tag("2.9d")), 2);
    assert_eq!(tag_result(&tag("-2.1f")), -3);
    assert_eq!(tag_result(&tag("[1,2,3]")), 3);
    assert_eq!(tag_result(&tag("[I;1,2]")), 2);
    assert_eq!(tag_result(&tag("{a:1,b:2}")), 2);
    assert_eq!(tag_result(&tag("\"a😀\"")), 3);
}

#[test]
fn remove_counts_every_removed_tag() {
    let mut data = NbtTag::Compound(compound("{list:[{a:1},{a:2},{b:3}]}"));
    assert_eq!(path("list[].a").remove(&mut data), 2);
    assert_data(&data, "{list:[{},{},{b:3}]}");
    assert_eq!(path("missing").remove(&mut data), 0);
}

#[test]
fn storage_accessor_reads_back_what_it_wrote() {
    let storage = CommandStorage::new();
    let id = Identifier::vanilla_static("test");
    let accessor = DataAccessor::Storage {
        storage: &storage,
        id: id.clone(),
    };
    assert_eq!(accessor.data().len(), 0);
    let data = merged_data(&accessor.data(), &compound("{score:3}")).expect("data changes");
    accessor.set_data(data).expect("storage takes any compound");
    assert!(nbt_compounds_equal(
        &storage.get(&id),
        &compound("{score:3}")
    ));
}

#[test]
fn block_accessor_loads_modified_data_into_the_block_entity() {
    init_vanilla_registry();
    init_behaviors();
    init_block_entities();
    let fixture = fresh_test_world("data_block");
    let world = &fixture.world;
    insert_ready_full_chunk(world, ChunkPos::new(0, 0));
    let pos = BlockPos::new(1, 64, 1);
    assert!(world.set_block(
        pos,
        vanilla_blocks::CHEST.default_state(),
        UpdateFlags::UPDATE_NONE
    ));
    let accessor = DataAccessor::Block {
        block_entity: world
            .get_block_entity(pos)
            .expect("a chest has a block entity"),
        pos,
        world: Arc::clone(world),
    };
    let mut data = NbtTag::Compound(accessor.data());
    let loot = "\"minecraft:chests/simple_dungeon\"";
    assert_eq!(
        modify(&mut data, Operation::Set, 0, "LootTable", &[loot]),
        1
    );
    let NbtTag::Compound(data) = data else {
        panic!("data must stay a compound");
    };
    accessor.set_data(data).expect("block entities accept data");
    let saved = accessor.data();
    assert_eq!(saved.get("LootTable").map(snbt).as_deref(), Some(loot));
}
