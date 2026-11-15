//! Vanilla `DataCommands`.
//!
//! `modify ... compute` is not ported yet: it needs vanilla's loot-context
//! sources and inline number providers, which Steel does not have.

mod accessor;

use simdnbt::owned::{NbtCompound, NbtTag};
use steel_utils::{
    Identifier,
    nbt::{
        NbtPath, NbtPathMutationError, is_too_deep, merge_nbt_compounds, nbt_compounds_equal,
        nbt_list_values, to_canonical_snbt,
    },
    translations,
};
use text_components::TextComponent;

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use accessor::{AccessorKind, DataAccessor};

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("data"), |_| command())
}

fn command() -> Builder {
    let mut root = literal("data");
    for kind in AccessorKind::ALL {
        root = root
            .then(kind.wrap(literal("merge"), "target", move |target| {
                target.then(
                    argument("nbt", SteelArgumentType::compound_tag())
                        .executes(move |context| merge_data(context, kind)),
                )
            }))
            .then(kind.wrap(literal("get"), "target", move |target| {
                target
                    .executes(move |context| get_all_data(context, kind))
                    .then(
                        argument("path", SteelArgumentType::nbt_path())
                            .executes(move |context| get_data(context, kind))
                            .then(
                                argument("scale", ArgumentType::double(f64::MIN, f64::MAX))
                                    .executes(move |context| get_numeric(context, kind)),
                            ),
                    )
            }))
            .then(kind.wrap(literal("remove"), "target", move |target| {
                target.then(
                    argument("path", SteelArgumentType::nbt_path())
                        .executes(move |context| remove_data(context, kind)),
                )
            }));
    }
    root.then(modify())
}

/// Vanilla `decorateModification`.
fn modify() -> Builder {
    let mut modify = literal("modify");
    for target in AccessorKind::ALL {
        modify = target.wrap(modify, "target", move |target_node| {
            target_node.then(
                argument("targetPath", SteelArgumentType::nbt_path())
                    .then(literal("insert").then(with_sources(
                        argument("index", ArgumentType::integer(i32::MIN, i32::MAX)),
                        target,
                        Operation::Insert,
                    )))
                    .then(with_sources(literal("prepend"), target, Operation::Prepend))
                    .then(with_sources(literal("append"), target, Operation::Append))
                    .then(with_sources(literal("set"), target, Operation::Set))
                    .then(with_sources(literal("merge"), target, Operation::Merge)),
            )
        });
    }
    modify
}

/// Vanilla's `DataManipulator`s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation {
    Insert,
    Prepend,
    Append,
    Set,
    Merge,
}

/// Where the values of a `modify` come from.
#[derive(Clone, Copy, Debug)]
enum Source {
    /// `from <source>` (whole data) or `from <source> <sourcePath>`.
    From { kind: AccessorKind, path: bool },
    /// `string <source> [<sourcePath> [<start> [<end>]]]`.
    String {
        kind: AccessorKind,
        path: bool,
        start: bool,
        end: bool,
    },
    /// `value <value>`.
    Value,
}

/// Adds the `from`, `string` and `value` children of one operation, in vanilla's order.
fn with_sources(node: Builder, target: AccessorKind, operation: Operation) -> Builder {
    let run = move |source| {
        move |context: &SteelCommandContext<CommandSource>| {
            manipulate_data(context, target, operation, source)
        }
    };
    let mut from = literal("from");
    let mut string = literal("string");
    for kind in AccessorKind::ALL {
        from = kind.wrap(from, "source", |source| {
            source
                .executes(run(Source::From { kind, path: false }))
                .then(
                    argument("sourcePath", SteelArgumentType::nbt_path())
                        .executes(run(Source::From { kind, path: true })),
                )
        });
        let string_source = move |path, start, end| Source::String {
            kind,
            path,
            start,
            end,
        };
        string = kind.wrap(string, "source", |source| {
            source
                .executes(run(string_source(false, false, false)))
                .then(
                    argument("sourcePath", SteelArgumentType::nbt_path())
                        .executes(run(string_source(true, false, false)))
                        .then(
                            argument("start", ArgumentType::integer(i32::MIN, i32::MAX))
                                .executes(run(string_source(true, true, false)))
                                .then(
                                    argument("end", ArgumentType::integer(i32::MIN, i32::MAX))
                                        .executes(run(string_source(true, true, true))),
                                ),
                        ),
                )
        });
    }
    let value = literal("value")
        .then(argument("value", SteelArgumentType::nbt_tag()).executes(run(Source::Value)));
    node.then(from).then(string).then(value)
}

fn merge_failed() -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(
        &translations::COMMANDS_DATA_MERGE_FAILED,
    ))
}

fn path_error(error: &NbtPathMutationError) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(error.component())
}

fn snbt(tag: &NbtTag) -> String {
    to_canonical_snbt(tag).unwrap_or_default()
}

/// The data vanilla `mergeData` writes back, or why it writes nothing.
fn merged_data(old: &NbtCompound, nbt: &NbtCompound) -> Result<NbtCompound, CommandSyntaxError> {
    if is_too_deep(&NbtTag::Compound(nbt.clone()), 0) {
        return Err(path_error(&NbtPathMutationError::TooDeep));
    }
    let mut result = old.clone();
    merge_nbt_compounds(&mut result, nbt);
    if nbt_compounds_equal(old, &result) {
        return Err(merge_failed());
    }
    Ok(result)
}

/// Vanilla `mergeData`.
fn merge_data(
    context: &SteelCommandContext<CommandSource>,
    kind: AccessorKind,
) -> Result<i32, CommandSyntaxError> {
    let accessor = DataAccessor::access(context, kind, "target")?;
    let result = merged_data(&accessor.data(), context.compound_tag("nbt")?)?;
    accessor.set_data(result)?;
    context
        .source()
        .send_success(&accessor.modified_success(), true);
    Ok(1)
}

/// Vanilla `getData(source, accessor)`.
fn get_all_data(
    context: &SteelCommandContext<CommandSource>,
    kind: AccessorKind,
) -> Result<i32, CommandSyntaxError> {
    let accessor = DataAccessor::access(context, kind, "target")?;
    let data = NbtTag::Compound(accessor.data());
    context
        .source()
        .send_success(&accessor.print_success(&data), false);
    Ok(1)
}

/// Vanilla `getSingleTag`.
fn single_tag(accessor: &DataAccessor<'_>, path: &NbtPath) -> Result<NbtTag, CommandSyntaxError> {
    let mut tags = path
        .get_existing(&NbtTag::Compound(accessor.data()))
        .map_err(|error| path_error(&error))?;
    if tags.len() > 1 {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_DATA_GET_MULTIPLE,
        )));
    }
    tags.pop().ok_or_else(|| {
        path_error(&NbtPathMutationError::NothingFound(
            path.as_str().to_owned(),
        ))
    })
}

/// A numeric tag's `doubleValue`.
const fn numeric_value(tag: &NbtTag) -> Option<f64> {
    Some(match tag {
        NbtTag::Byte(value) => *value as f64,
        NbtTag::Short(value) => *value as f64,
        NbtTag::Int(value) => *value as f64,
        NbtTag::Long(value) => *value as f64,
        NbtTag::Float(value) => *value as f64,
        NbtTag::Double(value) => *value,
        _ => return None,
    })
}

/// Vanilla `Mth.floor(double)`, with Java's saturating `(int)` cast.
const fn floor_to_int(value: f64) -> i32 {
    value.floor() as i32
}

/// Java's `String.length`: UTF-16 code units.
fn java_length(value: &str) -> i32 {
    i32::try_from(value.encode_utf16().count()).unwrap_or(i32::MAX)
}

/// The result of vanilla `getData(source, accessor, path)` for one tag.
fn tag_result(tag: &NbtTag) -> i32 {
    if let Some(value) = numeric_value(tag) {
        return floor_to_int(value);
    }
    let size = match tag {
        NbtTag::List(list) => nbt_list_values(list).len(),
        NbtTag::ByteArray(values) => values.len(),
        NbtTag::IntArray(values) => values.len(),
        NbtTag::LongArray(values) => values.len(),
        NbtTag::Compound(compound) => compound.len(),
        NbtTag::String(value) => return java_length(&value.to_str()),
        _ => 0,
    };
    i32::try_from(size).unwrap_or(i32::MAX)
}

/// Vanilla `getData(source, accessor, path)`.
fn get_data(
    context: &SteelCommandContext<CommandSource>,
    kind: AccessorKind,
) -> Result<i32, CommandSyntaxError> {
    let accessor = DataAccessor::access(context, kind, "target")?;
    let path = context.nbt_path("path")?;
    let tag = single_tag(&accessor, path)?;
    let result = tag_result(&tag);
    context
        .source()
        .send_success(&accessor.print_success(&tag), false);
    Ok(result)
}

/// Vanilla `getNumeric`.
fn get_numeric(
    context: &SteelCommandContext<CommandSource>,
    kind: AccessorKind,
) -> Result<i32, CommandSyntaxError> {
    let accessor = DataAccessor::access(context, kind, "target")?;
    let path = context.nbt_path("path")?;
    let scale = context.double("scale")?;
    let Some(value) = numeric_value(&single_tag(&accessor, path)?) else {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_DATA_GET_INVALID
                .message([path.as_str().to_owned()])
                .component(),
        ));
    };
    let result = floor_to_int(value * scale);
    context
        .source()
        .send_success(&accessor.print_path_success(path, scale, result), false);
    Ok(result)
}

/// Vanilla `removeData`.
fn remove_data(
    context: &SteelCommandContext<CommandSource>,
    kind: AccessorKind,
) -> Result<i32, CommandSyntaxError> {
    let accessor = DataAccessor::access(context, kind, "target")?;
    let path = context.nbt_path("path")?;
    let mut data = NbtTag::Compound(accessor.data());
    let count = path.remove(&mut data);
    if count == 0 {
        return Err(merge_failed());
    }
    let NbtTag::Compound(data) = data else {
        return Err(merge_failed());
    };
    accessor.set_data(data)?;
    context
        .source()
        .send_success(&accessor.modified_success(), true);
    Ok(i32::try_from(count).unwrap_or(i32::MAX))
}

/// Vanilla `getAsText`.
fn as_text(tag: &NbtTag) -> Result<String, CommandSyntaxError> {
    match tag {
        NbtTag::String(value) => Ok(value.to_str().into_owned()),
        _ if numeric_value(tag).is_some() => Ok(snbt(tag)),
        _ => Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_DATA_MODIFY_EXPECTED_VALUE
                .message([snbt(tag)])
                .component(),
        )),
    }
}

/// Vanilla `getOffset`: a negative index counts back from the end.
const fn offset(index: i32, length: i32) -> i32 {
    if index >= 0 { index } else { length + index }
}

/// Vanilla `substring` over Java's UTF-16 code units.
fn substring(value: &str, start: i32, end: Option<i32>) -> Result<String, CommandSyntaxError> {
    let units: Vec<u16> = value.encode_utf16().collect();
    let length = java_length(value);
    let start = offset(start, length);
    let end = end.map_or(length, |end| offset(end, length));
    if start < 0 || end > length || start > end {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_DATA_MODIFY_INVALID_SUBSTRING
                .message([start.to_string(), end.to_string()])
                .component(),
        ));
    }
    let range = start.unsigned_abs() as usize..end.unsigned_abs() as usize;
    Ok(String::from_utf16_lossy(&units[range]))
}

/// The tags a `modify` source yields: vanilla `getSingletonSource`,
/// `resolveSourcePath` and `stringifyTagList`.
fn source_values(
    context: &SteelCommandContext<CommandSource>,
    source: Source,
) -> Result<Vec<NbtTag>, CommandSyntaxError> {
    let (kind, path) = match source {
        Source::Value => return Ok(vec![context.nbt_tag("value")?.clone()]),
        Source::From { kind, path } | Source::String { kind, path, .. } => (kind, path),
    };
    let accessor = DataAccessor::access(context, kind, "source")?;
    let data = NbtTag::Compound(accessor.data());
    let values = if path {
        context
            .nbt_path("sourcePath")?
            .get_existing(&data)
            .map_err(|error| path_error(&error))?
    } else {
        vec![data]
    };
    let Source::String { start, end, .. } = source else {
        return Ok(values);
    };
    let start = start.then(|| context.integer("start")).transpose()?;
    let end = end.then(|| context.integer("end")).transpose()?;
    values
        .iter()
        .map(|tag| {
            let text = as_text(tag)?;
            let text = match start {
                Some(start) => substring(&text, start, end)?,
                None => text,
            };
            Ok(NbtTag::String(text.into()))
        })
        .collect()
}

/// Vanilla `manipulateData`.
fn manipulate_data(
    context: &SteelCommandContext<CommandSource>,
    target: AccessorKind,
    operation: Operation,
    source: Source,
) -> Result<i32, CommandSyntaxError> {
    let values = source_values(context, source)?;
    let accessor = DataAccessor::access(context, target, "target")?;
    let path = context.nbt_path("targetPath")?;
    let index = match operation {
        Operation::Insert => context.integer("index")?,
        _ => 0,
    };
    let mut data = NbtTag::Compound(accessor.data());
    let result = apply_operation(operation, index, &mut data, path, &values)?;
    if result == 0 {
        return Err(merge_failed());
    }
    let NbtTag::Compound(data) = data else {
        return Err(merge_failed());
    };
    accessor.set_data(data)?;
    context
        .source()
        .send_success(&accessor.modified_success(), true);
    Ok(result)
}

/// Applies one manipulator to `data`, returning how many tags it changed.
/// `index` is only read by [`Operation::Insert`].
fn apply_operation(
    operation: Operation,
    index: i32,
    data: &mut NbtTag,
    path: &NbtPath,
    values: &[NbtTag],
) -> Result<i32, CommandSyntaxError> {
    let changed = match operation {
        Operation::Insert => path.insert(index, data, values),
        Operation::Prepend => path.insert(0, data, values),
        Operation::Append => path.insert(-1, data, values),
        Operation::Set => match values.last() {
            Some(value) => path.set(data, value.clone()),
            None => Ok(0),
        },
        Operation::Merge => return merge_into_path(data, path, values),
    };
    let changed = changed.map_err(|error| path_error(&error))?;
    Ok(i32::try_from(changed).unwrap_or(i32::MAX))
}

/// A failure inside vanilla's `merge` manipulator.
enum MergeError {
    Path(NbtPathMutationError),
    Command(CommandSyntaxError),
}

impl From<NbtPathMutationError> for MergeError {
    fn from(error: NbtPathMutationError) -> Self {
        Self::Path(error)
    }
}

fn expected_object(tag: &NbtTag) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_DATA_MODIFY_EXPECTED_OBJECT
            .message([snbt(tag)])
            .component(),
    )
}

/// Vanilla's `modify ... merge` manipulator.
fn merge_into_path(
    data: &mut NbtTag,
    path: &NbtPath,
    values: &[NbtTag],
) -> Result<i32, CommandSyntaxError> {
    let mut combined = NbtCompound::new();
    for value in values {
        if is_too_deep(value, 0) {
            return Err(path_error(&NbtPathMutationError::TooDeep));
        }
        let NbtTag::Compound(compound) = value else {
            return Err(expected_object(value));
        };
        merge_nbt_compounds(&mut combined, compound);
    }
    let mut changed = 0;
    let merged = path.get_or_create(data, &NbtTag::Compound(NbtCompound::new()), |target| {
        let NbtTag::Compound(target) = target else {
            return Err(MergeError::Command(expected_object(target)));
        };
        let original = target.clone();
        merge_nbt_compounds(target, &combined);
        changed += i32::from(!nbt_compounds_equal(&original, target));
        Ok(())
    });
    match merged {
        Ok(()) => Ok(changed),
        Err(MergeError::Path(error)) => Err(path_error(&error)),
        Err(MergeError::Command(error)) => Err(error),
    }
}

#[cfg(test)]
mod tests;
