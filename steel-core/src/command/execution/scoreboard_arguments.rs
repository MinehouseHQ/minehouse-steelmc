//! Vanilla `ObjectiveCriteriaArgument`, `ScoreboardSlotArgument`,
//! `OperationArgument` and `StyleArgument`.

use simdnbt::owned::NbtTag;
use steel_registry::REGISTRY;
use steel_utils::{nbt::parse_snbt_argument, translations};
use text_components::TextComponent;

use super::suggestions::suggest_list;
use crate::{
    command::brigadier::{
        CommandSyntaxError, CommandSyntaxErrorKind, StringReader, SuggestionsBuilder,
    },
    scoreboard::{CustomCriteria, DisplaySlot, ObjectiveCriteria, TeamColor},
};

/// Reads up to the next space, as the criteria and operation parsers do.
fn read_to_space<'a>(reader: &mut StringReader<'a>) -> &'a str {
    let start = reader.read_so_far().len();
    while reader.can_read() && reader.peek() != Some(' ') {
        reader.skip();
    }
    &reader.input()[start..reader.read_so_far().len()]
}

/// Vanilla `ObjectiveCriteriaArgument.parse`.
pub(super) fn parse_criteria(
    reader: &mut StringReader<'_>,
) -> Result<ObjectiveCriteria, CommandSyntaxError> {
    let start = reader.checkpoint();
    let name = read_to_space(reader);
    ObjectiveCriteria::by_name(name).ok_or_else(|| {
        let message = translations::ARGUMENT_CRITERIA_INVALID
            .message([name.to_owned()])
            .component();
        reader.restore(start);
        reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(message)))
    })
}

/// Vanilla `ObjectiveCriteriaArgument.listSuggestions`: every custom
/// criterion, then every statistic of every stat type.
pub(super) fn suggest_criteria(builder: &mut SuggestionsBuilder<'_>) {
    let mut names: Vec<String> = CustomCriteria::all()
        .into_iter()
        .map(|criteria| ObjectiveCriteria::Custom(criteria).name())
        .collect();
    for (_, stat_type) in REGISTRY.stat_types.iter() {
        let type_name = stat_type.key.to_string().replace(':', ".");
        for id in 0..stat_type.registry_len() {
            if let Some(key) = stat_type.key_from_id(id) {
                names.push(format!("{type_name}:{}", key.to_string().replace(':', ".")));
            }
        }
    }
    suggest_list(builder, names);
}

/// Vanilla `ScoreboardSlotArgument.parse`.
pub(super) fn parse_display_slot(
    reader: &mut StringReader<'_>,
) -> Result<DisplaySlot, CommandSyntaxError> {
    let name = reader.read_unquoted_string();
    DisplaySlot::by_name(name).ok_or_else(|| {
        let message = translations::ARGUMENT_SCOREBOARD_DISPLAY_SLOT_INVALID
            .message([name.to_owned()])
            .component();
        reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(message)))
    })
}

/// Vanilla `ScoreboardSlotArgument.listSuggestions`.
pub(super) fn suggest_display_slots(builder: &mut SuggestionsBuilder<'_>) {
    suggest_list(builder, DisplaySlot::ALL.map(DisplaySlot::name));
}

/// Vanilla `TeamColorArgument.parse`.
pub(super) fn parse_team_color(
    reader: &mut StringReader<'_>,
) -> Result<TeamColor, CommandSyntaxError> {
    let name = reader.read_unquoted_string();
    TeamColor::by_name(name).ok_or_else(|| {
        let message = translations::ARGUMENT_COLOR_INVALID
            .message([name.to_owned()])
            .component();
        reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(message)))
    })
}

/// Vanilla `TeamColorArgument.listSuggestions`.
pub(super) fn suggest_team_colors(builder: &mut SuggestionsBuilder<'_>) {
    suggest_list(builder, TeamColor::ALL.map(TeamColor::name));
}

/// Vanilla `OperationArgument.Operation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScoreOperation {
    /// `=`
    Assign,
    /// `+=`
    Add,
    /// `-=`
    Subtract,
    /// `*=`
    Multiply,
    /// `/=`, flooring
    Divide,
    /// `%=`, with the divisor's sign
    Modulo,
    /// `<`
    Min,
    /// `>`
    Max,
    /// `><`, which also changes the source score
    Swap,
}

impl ScoreOperation {
    /// The operators in vanilla's suggestion order.
    const ALL: [(&'static str, Self); 9] = [
        ("=", Self::Assign),
        ("+=", Self::Add),
        ("-=", Self::Subtract),
        ("*=", Self::Multiply),
        ("/=", Self::Divide),
        ("%=", Self::Modulo),
        ("<", Self::Min),
        (">", Self::Max),
        ("><", Self::Swap),
    ];

    /// Vanilla `SimpleOperation.apply` with Java's wrapping `int` arithmetic.
    /// [`Self::Swap`] returns the source value, the new target value.
    ///
    /// # Errors
    ///
    /// Returns `arguments.operation.div0` for `/=` and `%=` by zero.
    pub(crate) fn apply(self, target: i32, source: i32) -> Result<i32, CommandSyntaxError> {
        Ok(match self {
            Self::Assign | Self::Swap => source,
            Self::Add => target.wrapping_add(source),
            Self::Subtract => target.wrapping_sub(source),
            Self::Multiply => target.wrapping_mul(source),
            Self::Divide => floor_div(target, nonzero(source)?),
            Self::Modulo => floor_mod(target, nonzero(source)?),
            Self::Min => target.min(source),
            Self::Max => target.max(source),
        })
    }
}

fn nonzero(divisor: i32) -> Result<i32, CommandSyntaxError> {
    if divisor == 0 {
        Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::ARGUMENTS_OPERATION_DIV0,
        )))
    } else {
        Ok(divisor)
    }
}

/// Java `Math.floorDiv`.
const fn floor_div(dividend: i32, divisor: i32) -> i32 {
    let quotient = dividend.wrapping_div(divisor);
    if dividend.wrapping_rem(divisor) != 0 && (dividend ^ divisor) < 0 {
        quotient - 1
    } else {
        quotient
    }
}

/// Java `Math.floorMod`, which `Mth.positiveModulo` uses.
const fn floor_mod(dividend: i32, divisor: i32) -> i32 {
    let remainder = dividend.wrapping_rem(divisor);
    if remainder != 0 && (remainder ^ divisor) < 0 {
        remainder + divisor
    } else {
        remainder
    }
}

/// Vanilla `OperationArgument.parse`.
pub(super) fn parse_operation(
    reader: &mut StringReader<'_>,
) -> Result<ScoreOperation, CommandSyntaxError> {
    let invalid = |reader: &StringReader<'_>| {
        reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(
            TextComponent::from(&translations::ARGUMENTS_OPERATION_INVALID),
        )))
    };
    if !reader.can_read() {
        return Err(invalid(reader));
    }
    let operator = read_to_space(reader);
    ScoreOperation::ALL
        .into_iter()
        .find_map(|(name, operation)| (name == operator).then_some(operation))
        .ok_or_else(|| {
            // Vanilla throws the plain exception here, without the reader's
            // context.
            CommandSyntaxError::dynamic(TextComponent::from(
                &translations::ARGUMENTS_OPERATION_INVALID,
            ))
        })
}

/// Vanilla `OperationArgument.listSuggestions`.
pub(super) fn suggest_operations(builder: &mut SuggestionsBuilder<'_>) {
    suggest_list(builder, ScoreOperation::ALL.map(|(name, _)| name));
}

/// Vanilla `StyleArgument.parse`: an SNBT style, kept as the style of an
/// empty text component.
pub(super) fn parse_style(
    reader: &mut StringReader<'_>,
) -> Result<TextComponent, CommandSyntaxError> {
    let start = reader.checkpoint();
    let invalid = |reader: &StringReader<'_>, message: String| {
        reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(
            translations::ARGUMENT_STYLE_INVALID
                .message([message])
                .component(),
        )))
    };
    let (tag, consumed) = parse_snbt_argument(reader.remaining()).map_err(|error| {
        reader.advance_bytes(error.cursor());
        reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(error.component())))
    })?;
    let NbtTag::Compound(mut style) = tag else {
        reader.restore(start);
        return Err(invalid(reader, "Not a compound".to_owned()));
    };
    reader.advance_bytes(consumed);
    style.insert("text", String::new());
    TextComponent::try_from_nbt(&NbtTag::Compound(style)).map_err(|error| {
        reader.restore(start);
        invalid(reader, error.to_string())
    })
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;
    use text_components::format::Color;

    use super::{
        ScoreOperation, floor_div, floor_mod, parse_criteria, parse_display_slot, parse_operation,
        parse_style,
    };
    use crate::command::brigadier::StringReader;
    use crate::scoreboard::{CustomCriteria, DisplaySlot, ObjectiveCriteria, TeamColor};

    #[test]
    fn division_floors_like_java() {
        assert_eq!(floor_div(7, 2), 3);
        assert_eq!(floor_div(-7, 2), -4);
        assert_eq!(floor_div(7, -2), -4);
        assert_eq!(floor_div(i32::MIN, -1), i32::MIN);
        assert_eq!(floor_mod(-7, 3), 2);
        assert_eq!(floor_mod(7, -3), -2);
        assert_eq!(floor_mod(6, 3), 0);
    }

    #[test]
    fn operations_wrap_and_reject_zero_divisors() {
        assert_eq!(ScoreOperation::Add.apply(i32::MAX, 1).ok(), Some(i32::MIN));
        assert_eq!(ScoreOperation::Min.apply(3, -2).ok(), Some(-2));
        assert_eq!(ScoreOperation::Swap.apply(3, 9).ok(), Some(9));
        assert!(ScoreOperation::Divide.apply(3, 0).is_err());
        assert!(ScoreOperation::Modulo.apply(3, 0).is_err());
    }

    #[test]
    fn criteria_and_slots_parse_up_to_the_next_space() {
        init_vanilla_registry();
        let mut reader = StringReader::new("dummy rest");
        assert_eq!(
            parse_criteria(&mut reader).ok(),
            Some(ObjectiveCriteria::Custom(CustomCriteria::Dummy))
        );
        assert_eq!(reader.remaining(), " rest");
        let mut reader = StringReader::new("minecraft.mined:minecraft.stone");
        let Ok(mined) = parse_criteria(&mut reader) else {
            panic!("a stat criterion parses");
        };
        assert_eq!(mined.name(), "minecraft.mined:minecraft.stone");
        // An unknown criterion fails at its start, like vanilla.
        let mut reader = StringReader::new("nope");
        assert!(parse_criteria(&mut reader).is_err());
        assert_eq!(reader.cursor(), 0);

        let mut reader = StringReader::new("sidebar.team.red");
        assert_eq!(
            parse_display_slot(&mut reader).ok(),
            Some(DisplaySlot::TeamSidebar(TeamColor::Red))
        );
        assert!(parse_display_slot(&mut StringReader::new("belowname")).is_err());
    }

    #[test]
    fn operators_and_styles_parse_like_vanilla() {
        let mut reader = StringReader::new(">< x");
        assert_eq!(
            parse_operation(&mut reader).ok(),
            Some(ScoreOperation::Swap)
        );
        assert_eq!(reader.remaining(), " x");
        assert!(parse_operation(&mut StringReader::new("<>")).is_err());
        assert!(parse_operation(&mut StringReader::new("")).is_err());

        let mut reader = StringReader::new("{color:\"red\",bold:true} tail");
        let Ok(style) = parse_style(&mut reader) else {
            panic!("an SNBT style parses");
        };
        assert_eq!(style.format.color, Some(Color::Red));
        assert_eq!(style.format.bold, Some(true));
        assert_eq!(reader.remaining(), " tail");
        // A style must be a compound.
        let mut reader = StringReader::new("1b");
        assert!(parse_style(&mut reader).is_err());
        assert_eq!(reader.cursor(), 0);
    }
}
