//! Vanilla data pack functions: `CommandFunction`, its plain and macro forms,
//! and the library a `datapacks` folder loads them into.

mod library;
mod template;
mod timer;

use std::collections::VecDeque;
use std::sync::Arc;

use simdnbt::owned::{NbtCompound, NbtTag};
use steel_utils::{Identifier, locks::SyncMutex, nbt::to_canonical_snbt, translations};
use text_components::TextComponent;

use super::CommandDispatcher;
use super::brigadier::{CommandSyntaxError, StringReader};
use super::execution::{CommandSource, SteelContextChain};
use template::check_command_line_length;

pub(crate) use library::{FunctionLibrary, ServerFunctions, TagEntry, build_tags, load_tag_files};
pub(crate) use template::{StringTemplate, is_valid_variable_name};
pub(crate) use timer::{DomainTimerQueues, TimerCallback, TimerQueue};

/// One compiled function line, run as if typed by the function's source.
pub(crate) type FunctionEntry = SteelContextChain<CommandSource>;

/// Vanilla `MacroFunction.MAX_CACHE_ENTRIES`.
const MAX_CACHE_ENTRIES: usize = 8;

/// Vanilla `InstantiatedFunction`: a function's lines ready to run.
pub(crate) struct InstantiatedFunction {
    id: Identifier,
    entries: Arc<[FunctionEntry]>,
}

impl InstantiatedFunction {
    /// The function's id; a macro's instances add `/<hash>` to it.
    pub(crate) const fn id(&self) -> &Identifier {
        &self.id
    }

    /// The compiled lines.
    pub(crate) fn entries(&self) -> Arc<[FunctionEntry]> {
        Arc::clone(&self.entries)
    }
}

/// Vanilla `CommandFunction`: a `PlainTextFunction` or a `MacroFunction`.
pub(crate) enum CommandFunction {
    Plain(Arc<InstantiatedFunction>),
    Macro(Box<MacroFunction>),
}

/// Vanilla `MacroFunction`: lines with `$(name)` variables, compiled for each
/// set of arguments.
pub(crate) struct MacroFunction {
    id: Identifier,
    entries: Vec<MacroEntry>,
    parameters: Vec<String>,
    compilation_source: CommandSource,
    cache: SyncMutex<VecDeque<(Vec<String>, Arc<InstantiatedFunction>)>>,
}

enum MacroEntry {
    Plain(FunctionEntry),
    Macro {
        template: StringTemplate,
        parameters: Vec<usize>,
    },
}

impl CommandFunction {
    /// The function's id.
    pub(crate) fn id(&self) -> &Identifier {
        match self {
            Self::Plain(function) => function.id(),
            Self::Macro(function) => &function.id,
        }
    }

    /// Vanilla `CommandFunction.fromLines`: compiles a `.mcfunction` file.
    ///
    /// Lines are trimmed; a trailing `\` joins the next line; blank lines and
    /// `#` comments are skipped; a line starting with `$` is a macro line,
    /// compiled when the function is called with arguments.
    pub(crate) fn from_lines(
        id: Identifier,
        dispatcher: &CommandDispatcher,
        compilation_source: &CommandSource,
        lines: &[&str],
    ) -> Result<Self, String> {
        let mut builder = FunctionBuilder::default();
        let mut index = 0;
        while index < lines.len() {
            let line_number = index + 1;
            let line = joined_line(lines, &mut index)?;
            index += 1;
            let Some(first) = line.chars().next() else {
                continue;
            };
            match first {
                '#' => {}
                '/' => return Err(slash_error(&line, line_number)),
                '$' => builder.add_macro(&line[1..], line_number)?,
                _ => {
                    let entry =
                        parse_command(dispatcher, compilation_source, &line).map_err(|error| {
                            format!("Whilst parsing command on line {line_number}: {error}")
                        })?;
                    builder.add_command(entry);
                }
            }
        }
        Ok(builder.build(id, compilation_source))
    }

    /// Vanilla `instantiate`: the function's lines for `arguments`.
    ///
    /// The error is vanilla's `FunctionInstantiationException` message.
    pub(crate) fn instantiate(
        &self,
        arguments: Option<&NbtCompound>,
        dispatcher: &CommandDispatcher,
    ) -> Result<Arc<InstantiatedFunction>, Box<TextComponent>> {
        match self {
            Self::Plain(function) => Ok(Arc::clone(function)),
            Self::Macro(function) => function.instantiate(arguments, dispatcher),
        }
    }
}

impl MacroFunction {
    fn instantiate(
        &self,
        arguments: Option<&NbtCompound>,
        dispatcher: &CommandDispatcher,
    ) -> Result<Arc<InstantiatedFunction>, Box<TextComponent>> {
        let Some(arguments) = arguments else {
            return Err(translations::COMMANDS_FUNCTION_ERROR_MISSING_ARGUMENTS
                .message([self.id.to_string()])
                .component()
                .into());
        };
        let mut values = Vec::with_capacity(self.parameters.len());
        for parameter in &self.parameters {
            let Some(value) = arguments.get(parameter) else {
                return Err(translations::COMMANDS_FUNCTION_ERROR_MISSING_ARGUMENT
                    .message([self.id.to_string(), parameter.clone()])
                    .component()
                    .into());
            };
            values.push(stringify(value));
        }

        // Vanilla's linked map: a hit moves to the end, a miss evicts the first.
        let mut cache = self.cache.lock();
        if let Some(position) = cache.iter().position(|(cached, _)| *cached == values)
            && let Some(entry) = cache.remove(position)
        {
            let function = Arc::clone(&entry.1);
            cache.push_back(entry);
            return Ok(function);
        }
        if cache.len() >= MAX_CACHE_ENTRIES {
            cache.pop_front();
        }
        let function = Arc::new(self.substitute_and_parse(&values, dispatcher)?);
        cache.push_back((values, Arc::clone(&function)));
        Ok(function)
    }

    /// Vanilla `substituteAndParse`.
    fn substitute_and_parse(
        &self,
        values: &[String],
        dispatcher: &CommandDispatcher,
    ) -> Result<InstantiatedFunction, Box<TextComponent>> {
        let mut entries = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            match entry {
                MacroEntry::Plain(entry) => entries.push(entry.clone()),
                MacroEntry::Macro {
                    template,
                    parameters,
                } => {
                    let substitutions = parameters
                        .iter()
                        .map(|&index| values[index].as_str())
                        .collect::<Vec<_>>();
                    let command = template
                        .substitute(&substitutions)
                        .map_err(|error| Box::new(TextComponent::plain(error)))?;
                    let entry = parse_command(dispatcher, &self.compilation_source, &command)
                        .map_err(|error| {
                            Box::new(
                                translations::COMMANDS_FUNCTION_ERROR_PARSE
                                    .message([
                                        self.id.to_string(),
                                        command.clone(),
                                        error.to_string(),
                                    ])
                                    .component(),
                            )
                        })?;
                    entries.push(entry);
                }
            }
        }
        // Vanilla names the instance `<id>/<parameter names' List.hashCode>`.
        let id = Identifier::new(
            self.id.namespace.clone(),
            format!("{}/{}", self.id.path, list_hash_code(&self.parameters)),
        );
        Ok(InstantiatedFunction {
            id,
            entries: entries.into(),
        })
    }
}

/// Vanilla `FunctionBuilder`.
#[derive(Default)]
struct FunctionBuilder {
    plain_entries: Vec<FunctionEntry>,
    macro_entries: Option<Vec<MacroEntry>>,
    macro_arguments: Vec<String>,
}

impl FunctionBuilder {
    fn add_command(&mut self, entry: FunctionEntry) {
        match &mut self.macro_entries {
            Some(entries) => entries.push(MacroEntry::Plain(entry)),
            None => self.plain_entries.push(entry),
        }
    }

    fn add_macro(&mut self, command: &str, line_number: usize) -> Result<(), String> {
        let template = StringTemplate::parse(command).map_err(|error| {
            format!("Can't parse function line {line_number}: '{command}': {error}")
        })?;
        let parameters = template
            .variables()
            .iter()
            .map(|variable| self.argument_index(variable))
            .collect();
        let entries = self.macro_entries.get_or_insert_with(|| {
            self.plain_entries
                .drain(..)
                .map(MacroEntry::Plain)
                .collect()
        });
        entries.push(MacroEntry::Macro {
            template,
            parameters,
        });
        Ok(())
    }

    fn argument_index(&mut self, variable: &str) -> usize {
        self.macro_arguments
            .iter()
            .position(|argument| argument == variable)
            .unwrap_or_else(|| {
                self.macro_arguments.push(variable.to_owned());
                self.macro_arguments.len() - 1
            })
    }

    fn build(self, id: Identifier, compilation_source: &CommandSource) -> CommandFunction {
        match self.macro_entries {
            Some(entries) => CommandFunction::Macro(Box::new(MacroFunction {
                id,
                entries,
                parameters: self.macro_arguments,
                compilation_source: compilation_source.clone(),
                cache: SyncMutex::new(VecDeque::new()),
            })),
            None => CommandFunction::Plain(Arc::new(InstantiatedFunction {
                id,
                entries: self.plain_entries.into(),
            })),
        }
    }
}

/// The trimmed line at `index`, joined with the lines after it while it ends
/// in `\`; `index` is left on the last line read.
fn joined_line(lines: &[&str], index: &mut usize) -> Result<String, String> {
    let mut line = lines[*index].trim().to_owned();
    while line.ends_with('\\') {
        *index += 1;
        let Some(next) = lines.get(*index) else {
            return Err("Line continuation at end of file".to_owned());
        };
        line.pop();
        line.push_str(next.trim());
        check_command_line_length(&line)?;
    }
    check_command_line_length(&line)?;
    Ok(line)
}

/// Vanilla's errors for a line starting with `/`.
fn slash_error(line: &str, line_number: usize) -> String {
    let mut reader = StringReader::new(&line[1..]);
    if reader.peek() == Some('/') {
        return format!(
            "Unknown or invalid command '{line}' on line {line_number} (if you intended to make a comment, use '#' not '//')"
        );
    }
    let name = reader.read_unquoted_string();
    format!(
        "Unknown or invalid command '{line}' on line {line_number} (did you mean '{name}'? Do not use a preceding forwards slash.)"
    )
}

/// Vanilla `CommandFunction.parseCommand`.
fn parse_command(
    dispatcher: &CommandDispatcher,
    compilation_source: &CommandSource,
    command: &str,
) -> Result<FunctionEntry, CommandSyntaxError> {
    let parse = dispatcher.parse(command, compilation_source.clone());
    dispatcher.context_chain(parse)
}

/// Vanilla `MacroFunction.stringify`: numbers without a type suffix, strings
/// unquoted, anything else as SNBT.
fn stringify(tag: &NbtTag) -> String {
    match tag {
        NbtTag::Float(value) => format_decimal(f64::from(*value)),
        NbtTag::Double(value) => format_decimal(*value),
        NbtTag::Byte(value) => value.to_string(),
        NbtTag::Short(value) => value.to_string(),
        NbtTag::Long(value) => value.to_string(),
        NbtTag::String(value) => value.to_str().into_owned(),
        _ => to_canonical_snbt(tag).unwrap_or_default(),
    }
}

/// Vanilla's `DecimalFormat("#")` with 15 fraction digits, in the root locale.
fn format_decimal(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "∞" } else { "-∞" }.to_owned();
    }
    let text = format!("{value:.15}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    // The pattern has no minimum integer digits: a fraction drops its leading
    // zero, and only a value with no fraction prints one.
    match text.split_once("0.") {
        Some(("" | "-", fraction)) => {
            format!("{}.{fraction}", &text[..text.len() - fraction.len() - 2])
        }
        _ => text.to_owned(),
    }
}

/// Java's `List<String>.hashCode`.
fn list_hash_code(values: &[String]) -> i32 {
    values.iter().fold(1_i32, |hash, value| {
        hash.wrapping_mul(31).wrapping_add(string_hash_code(value))
    })
}

/// Java's `String.hashCode`, over UTF-16 code units.
fn string_hash_code(value: &str) -> i32 {
    value.encode_utf16().fold(0_i32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(i32::from(unit))
    })
}

#[cfg(test)]
mod tests {
    use simdnbt::Mutf8String;
    use simdnbt::owned::{NbtCompound, NbtList, NbtTag};

    use super::{format_decimal, list_hash_code, stringify};

    #[test]
    fn stringify_matches_vanillas_macro_arguments() {
        assert_eq!(stringify(&NbtTag::Byte(-3)), "-3");
        assert_eq!(stringify(&NbtTag::Short(300)), "300");
        assert_eq!(stringify(&NbtTag::Int(7)), "7");
        assert_eq!(stringify(&NbtTag::Long(1 << 40)), "1099511627776");
        assert_eq!(stringify(&NbtTag::Double(2.5)), "2.5");
        assert_eq!(stringify(&NbtTag::Double(3.0)), "3");
        assert_eq!(stringify(&NbtTag::Float(0.1)), ".100000001490116");
        assert_eq!(
            stringify(&NbtTag::String(Mutf8String::from("hello world"))),
            "hello world"
        );
        assert_eq!(stringify(&NbtTag::List(NbtList::Int(vec![1, 2]))), "[1,2]");
        let mut compound = NbtCompound::new();
        compound.insert("a", NbtTag::Byte(1));
        assert_eq!(stringify(&NbtTag::Compound(compound)), "{a:1b}");
    }

    #[test]
    fn decimals_have_no_exponent_or_trailing_zeros() {
        assert_eq!(format_decimal(1e20), "100000000000000000000");
        assert_eq!(format_decimal(-0.0), "-0");
        assert_eq!(format_decimal(1e-16), "0");
        assert_eq!(format_decimal(0.5), ".5");
        assert_eq!(format_decimal(-0.25), "-.25");
        assert_eq!(format_decimal(10.5), "10.5");
    }

    #[test]
    fn instance_suffix_is_javas_list_hash() {
        // `List.of("a", "b").hashCode()` in Java.
        assert_eq!(list_hash_code(&["a".to_owned(), "b".to_owned()]), 4066);
    }
}
