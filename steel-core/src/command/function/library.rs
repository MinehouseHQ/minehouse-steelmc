//! Vanilla `ServerFunctionLibrary` and `ServerFunctionManager`, one per Steel domain.

use std::collections::BTreeMap;
use std::sync::Arc;

use rustc_hash::FxHashSet;
use serde_json::Value;
use steel_utils::{Identifier, locks::SyncRwLock};

use super::CommandFunction;
use crate::command::CommandDispatcher;
use crate::command::execution::CommandSource;
use crate::datapack::DataPack;

/// Vanilla `ServerFunctionManager.TICK_FUNCTION_TAG`.
const TICK_FUNCTION_TAG: Identifier = Identifier::vanilla_static("tick");
/// Vanilla `ServerFunctionManager.LOAD_FUNCTION_TAG`.
const LOAD_FUNCTION_TAG: Identifier = Identifier::vanilla_static("load");

/// Vanilla `ServerFunctionLibrary`: the functions and function tags of a
/// domain's data packs.
#[derive(Default)]
pub(crate) struct FunctionLibrary {
    functions: BTreeMap<Identifier, Arc<CommandFunction>>,
    tags: BTreeMap<Identifier, Vec<Arc<CommandFunction>>>,
}

impl FunctionLibrary {
    /// Vanilla `reload`: compiles every pack's functions, a later pack's file
    /// replacing an earlier one's, then builds the function tags.
    ///
    /// A function that fails to compile and a tag file that fails to parse are
    /// logged and left out, as vanilla does.
    pub(crate) fn load(
        packs: &[DataPack],
        dispatcher: &CommandDispatcher,
        compilation_source: &CommandSource,
    ) -> Self {
        let mut files = BTreeMap::new();
        for pack in packs {
            for resource in pack.list_resources("function", ".mcfunction") {
                files.insert(resource.id, resource.contents);
            }
        }
        let mut functions = BTreeMap::new();
        for (id, contents) in files {
            let text = String::from_utf8_lossy(&contents);
            let lines = java_lines(&text);
            match CommandFunction::from_lines(id.clone(), dispatcher, compilation_source, &lines) {
                Ok(function) => {
                    functions.insert(id, Arc::new(function));
                }
                Err(error) => log::error!("Failed to load function {id}: {error}"),
            }
        }

        let tags = build_tags(&load_tag_files(packs), &functions);
        Self { functions, tags }
    }

    /// Vanilla `getFunction`.
    pub(crate) fn function(&self, id: &Identifier) -> Option<&Arc<CommandFunction>> {
        self.functions.get(id)
    }

    /// Vanilla `getTag`: an unknown tag has no functions.
    pub(crate) fn tag(&self, id: &Identifier) -> &[Arc<CommandFunction>] {
        self.tags.get(id).map_or(&[], Vec::as_slice)
    }

    /// Every function id, for suggestions.
    pub(crate) fn function_ids(&self) -> impl Iterator<Item = &Identifier> {
        self.functions.keys()
    }

    /// Every tag id, for suggestions.
    pub(crate) fn tag_ids(&self) -> impl Iterator<Item = &Identifier> {
        self.tags.keys()
    }
}

/// Vanilla `ServerFunctionManager`'s state for one domain.
struct DomainFunctions {
    library: Arc<FunctionLibrary>,
    ticking: Vec<Arc<CommandFunction>>,
    post_reload: bool,
}

/// The function libraries of every Steel domain.
#[derive(Default)]
pub(crate) struct ServerFunctions {
    domains: SyncRwLock<BTreeMap<String, DomainFunctions>>,
}

impl ServerFunctions {
    /// The library loaded for `domain`.
    pub(crate) fn library(&self, domain: &str) -> Option<Arc<FunctionLibrary>> {
        self.domains
            .read()
            .get(domain)
            .map(|functions| Arc::clone(&functions.library))
    }

    /// Vanilla `replaceLibrary`: `#minecraft:load` runs on the next tick.
    pub(crate) fn replace(&self, domain: &str, library: FunctionLibrary) {
        let functions = DomainFunctions {
            ticking: library.tag(&TICK_FUNCTION_TAG).to_vec(),
            library: Arc::new(library),
            post_reload: true,
        };
        self.domains.write().insert(domain.to_owned(), functions);
    }

    /// Vanilla `ServerFunctionManager.tick`'s functions, per domain: the load
    /// tag once after a reload, then the tick tag. Nothing runs while the game
    /// is frozen.
    pub(crate) fn take_tick_functions(
        &self,
        runs_normally: bool,
    ) -> Vec<(String, Vec<Arc<CommandFunction>>)> {
        if !runs_normally {
            return Vec::new();
        }
        let mut domains = self.domains.write();
        domains
            .iter_mut()
            .map(|(domain, functions)| {
                let mut run = Vec::new();
                if functions.post_reload {
                    functions.post_reload = false;
                    run.extend_from_slice(functions.library.tag(&LOAD_FUNCTION_TAG));
                }
                run.extend_from_slice(&functions.ticking);
                (domain.clone(), run)
            })
            .filter(|(_, run)| !run.is_empty())
            .collect()
    }

    /// Drops every library. Compiled functions hold sources that hold the
    /// server, so shutdown clears them to break that cycle.
    pub(crate) fn clear(&self) {
        self.domains.write().clear();
    }
}

/// Java's `BufferedReader.lines`: lines end at `\n`, `\r` or `\r\n`, and a
/// final terminator starts no empty line.
fn java_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let Some(end) = rest.find(['\n', '\r']) else {
            lines.push(rest);
            break;
        };
        lines.push(&rest[..end]);
        let terminator = if rest[end..].starts_with("\r\n") {
            2
        } else {
            1
        };
        rest = &rest[end + terminator..];
    }
    lines
}

/// Vanilla `TagEntry`.
#[derive(Debug, PartialEq, Eq)]
struct TagEntry {
    id: Identifier,
    tag: bool,
    required: bool,
}

/// Vanilla `TagLoader.load`: each tag's entries across every pack, in pack
/// order, a `"replace": true` file dropping the entries before it.
fn load_tag_files(packs: &[DataPack]) -> BTreeMap<Identifier, Vec<TagEntry>> {
    let mut tags: BTreeMap<Identifier, Vec<TagEntry>> = BTreeMap::new();
    for pack in packs {
        for resource in pack.list_resources("tags/function", ".json") {
            match parse_tag_file(&resource.contents) {
                Ok((replace, entries)) => {
                    let tag = tags.entry(resource.id).or_default();
                    if replace {
                        tag.clear();
                    }
                    tag.extend(entries);
                }
                Err(error) => log::error!(
                    "Couldn't read tag list {} from data pack {}: {error}",
                    resource.id,
                    pack.id()
                ),
            }
        }
    }
    tags
}

/// Vanilla `TagFile.CODEC`: `{"replace": bool, "values": [entry, ...]}`.
fn parse_tag_file(contents: &[u8]) -> Result<(bool, Vec<TagEntry>), String> {
    let json: Value = serde_json::from_slice(contents).map_err(|error| error.to_string())?;
    let object = json.as_object().ok_or("Not a JSON object")?;
    let replace = match object.get("replace") {
        None => false,
        Some(value) => value.as_bool().ok_or("Not a boolean: replace")?,
    };
    let values = object
        .get("values")
        .ok_or("No key values in MapLike")?
        .as_array()
        .ok_or("Not a list: values")?;
    let entries = values
        .iter()
        .map(parse_tag_entry)
        .collect::<Result<_, _>>()?;
    Ok((replace, entries))
}

/// Vanilla `TagEntry.CODEC`: `"id"`, `"#tag"` or `{"id": ..., "required": bool}`.
fn parse_tag_entry(value: &Value) -> Result<TagEntry, String> {
    let (raw, required) = match value {
        Value::String(raw) => (raw.as_str(), true),
        Value::Object(object) => {
            let raw = object
                .get("id")
                .and_then(Value::as_str)
                .ok_or("No key id in MapLike")?;
            let required = match object.get("required") {
                None => true,
                Some(value) => value.as_bool().ok_or("Not a boolean: required")?,
            };
            (raw, required)
        }
        _ => return Err(format!("Not a tag entry: {value}")),
    };
    let (tag, id) = raw.strip_prefix('#').map_or((false, raw), |id| (true, id));
    let id = id
        .parse::<Identifier>()
        .map_err(|error| format!("Not a valid resource location: {raw} {error}"))?;
    Ok(TagEntry { id, tag, required })
}

/// Vanilla `TagLoader.build`: each tag's functions, referenced tags built
/// first. A tag missing a required function or tag is logged and left out.
///
/// Vanilla's `DependencySorter` drops a dependency that would close a cycle;
/// here a reference back to a tag still being built counts as missing, which
/// gives the same result for every acyclic set of tags.
fn build_tags(
    files: &BTreeMap<Identifier, Vec<TagEntry>>,
    functions: &BTreeMap<Identifier, Arc<CommandFunction>>,
) -> BTreeMap<Identifier, Vec<Arc<CommandFunction>>> {
    let mut builder = TagBuilder {
        files,
        functions,
        built: BTreeMap::new(),
        building: FxHashSet::default(),
    };
    for id in files.keys() {
        builder.build(id);
    }
    builder
        .built
        .into_iter()
        .filter_map(|(id, tag)| Some((id, tag?)))
        .collect()
}

struct TagBuilder<'a> {
    files: &'a BTreeMap<Identifier, Vec<TagEntry>>,
    functions: &'a BTreeMap<Identifier, Arc<CommandFunction>>,
    built: BTreeMap<Identifier, Option<Vec<Arc<CommandFunction>>>>,
    building: FxHashSet<Identifier>,
}

impl TagBuilder<'_> {
    /// The tag's functions, `None` when it is missing or failed to build.
    fn build(&mut self, id: &Identifier) -> Option<Vec<Arc<CommandFunction>>> {
        if let Some(built) = self.built.get(id) {
            return built.clone();
        }
        let entries = self.files.get(id)?;
        if !self.building.insert(id.clone()) {
            return None;
        }
        let mut values: Vec<Arc<CommandFunction>> = Vec::new();
        let mut missing = Vec::new();
        for entry in entries {
            let found = if entry.tag {
                self.build(&entry.id)
            } else {
                self.functions
                    .get(&entry.id)
                    .map(|function| vec![Arc::clone(function)])
            };
            match found {
                Some(found) => {
                    for function in found {
                        if !values.iter().any(|value| Arc::ptr_eq(value, &function)) {
                            values.push(function);
                        }
                    }
                }
                None if entry.required => missing.push(entry),
                None => {}
            }
        }
        self.building.remove(id);
        let tag = if missing.is_empty() {
            Some(values)
        } else {
            let missing = missing
                .iter()
                .map(|entry| format!("{}{}", if entry.tag { "#" } else { "" }, entry.id))
                .collect::<Vec<_>>()
                .join(", ");
            log::error!("Couldn't load tag {id} as it is missing following references: {missing}");
            None
        };
        self.built.insert(id.clone(), tag.clone());
        tag
    }
}

#[cfg(test)]
mod tests {
    use super::{TagEntry, java_lines, parse_tag_file};
    use steel_utils::Identifier;

    #[test]
    fn lines_split_like_javas_buffered_reader() {
        assert_eq!(java_lines("a\nb\r\nc\rd"), ["a", "b", "c", "d"]);
        assert_eq!(java_lines("a\n"), ["a"]);
        assert_eq!(java_lines("a\n\nb"), ["a", "", "b"]);
        assert_eq!(java_lines(""), Vec::<&str>::new());
    }

    #[test]
    fn tag_files_parse_every_entry_form() {
        let (replace, entries) = parse_tag_file(
            br##"{"replace": true, "values": ["demo:a", "#demo:group", {"id": "b", "required": false}]}"##,
        )
        .expect("tag file parses");
        assert!(replace);
        assert_eq!(
            entries,
            [
                TagEntry {
                    id: Identifier::new("demo", "a"),
                    tag: false,
                    required: true,
                },
                TagEntry {
                    id: Identifier::new("demo", "group"),
                    tag: true,
                    required: true,
                },
                TagEntry {
                    id: Identifier::vanilla_static("b"),
                    tag: false,
                    required: false,
                },
            ]
        );
        assert!(parse_tag_file(br#"{"replace": false}"#).is_err());
        assert!(parse_tag_file(br#"{"values": ["Bad:Id"]}"#).is_err());
    }
}
