//! Vanilla `StringTemplate`: a macro line split around its `$(name)` variables.

/// Vanilla `CommandFunction.MAX_COMMAND_LENGTH` checked by `checkCommandLineLength`.
pub(super) const MAX_COMMAND_LENGTH: usize = 2_000_000;

/// A macro line's literal segments and the variables between them.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct StringTemplate {
    segments: Vec<String>,
    variables: Vec<String>,
}

impl StringTemplate {
    /// Vanilla `StringTemplate.fromString`.
    pub(crate) fn parse(input: &str) -> Result<Self, String> {
        let mut segments = Vec::new();
        let mut variables = Vec::new();
        let mut start = 0;
        let mut search = 0;
        while let Some(offset) = input[search..].find('$') {
            let index = search + offset;
            if input[index + 1..].starts_with('(') {
                segments.push(input[start..index].to_owned());
                let Some(end) = input[index + 1..].find(')').map(|end| index + 1 + end) else {
                    return Err("Unterminated macro variable".to_owned());
                };
                let variable = &input[index + 2..end];
                if !is_valid_variable_name(variable) {
                    return Err(format!("Invalid macro variable name '{variable}'"));
                }
                variables.push(variable.to_owned());
                start = end + 1;
                search = start;
            } else {
                search = index + 1;
            }
        }
        if start == 0 {
            return Err("No variables in macro".to_owned());
        }
        if start != input.len() {
            segments.push(input[start..].to_owned());
        }
        Ok(Self {
            segments,
            variables,
        })
    }

    /// The variable names, in the order they appear.
    pub(super) fn variables(&self) -> &[String] {
        &self.variables
    }

    /// Vanilla `substitute`: the line with each variable replaced by its argument.
    pub(super) fn substitute(&self, arguments: &[&str]) -> Result<String, String> {
        let mut output = String::new();
        for (segment, argument) in self.segments.iter().zip(arguments) {
            output.push_str(segment);
            output.push_str(argument);
            check_command_line_length(&output)?;
        }
        if self.segments.len() > self.variables.len()
            && let Some(last) = self.segments.last()
        {
            output.push_str(last);
        }
        check_command_line_length(&output)?;
        Ok(output)
    }
}

/// Vanilla `isValidVariableName`: letters, digits and `_`.
pub(crate) fn is_valid_variable_name(variable: &str) -> bool {
    variable
        .chars()
        .all(|character| character.is_alphanumeric() || character == '_')
}

/// Vanilla `CommandFunction.checkCommandLineLength`, counting UTF-16 units as
/// Java's `length` does.
pub(super) fn check_command_line_length(line: &str) -> Result<(), String> {
    let length = line.encode_utf16().count();
    if length <= MAX_COMMAND_LENGTH {
        return Ok(());
    }
    let truncated = line.chars().take(512).collect::<String>();
    Err(format!(
        "Command too long: {length} characters, contents: {truncated}..."
    ))
}

#[cfg(test)]
mod tests {
    use super::StringTemplate;

    #[test]
    fn splits_segments_around_variables() {
        let template = StringTemplate::parse("say $(a) and $(b)!").expect("template parses");
        assert_eq!(template.variables(), ["a", "b"]);
        assert_eq!(
            template.substitute(&["1", "2"]).expect("substitutes"),
            "say 1 and 2!"
        );

        let ends_with_variable = StringTemplate::parse("say $(a)").expect("template parses");
        assert_eq!(
            ends_with_variable.substitute(&["x"]).expect("substitutes"),
            "say x"
        );
    }

    #[test]
    fn dollar_without_parenthesis_is_literal() {
        let template = StringTemplate::parse("say $5 $(cost)").expect("template parses");
        assert_eq!(
            template.substitute(&["7"]).expect("substitutes"),
            "say $5 7"
        );
    }

    #[test]
    fn rejects_vanillas_invalid_templates() {
        assert_eq!(
            StringTemplate::parse("say hi"),
            Err("No variables in macro".to_owned())
        );
        assert_eq!(
            StringTemplate::parse("say $(a"),
            Err("Unterminated macro variable".to_owned())
        );
        assert_eq!(
            StringTemplate::parse("say $(a-b)"),
            Err("Invalid macro variable name 'a-b'".to_owned())
        );
    }
}
