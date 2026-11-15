//! Durations, times and chat formats of the ban and warn messages.

use std::{fmt::Write as _, mem};

use chrono::{DateTime, FixedOffset};
use text_components::{Modifier, TextComponent, format::Color};

/// Ten years: longer bans are permanent ones.
const MAX_DURATION_S: u64 = 10 * 365 * 24 * 3600;
/// Moscow time, which has had no daylight saving since 2014.
const MOSCOW_OFFSET_S: i32 = 3 * 3600;

/// `30m`, `12h`, `7d`, `2w`, `1mo`, sums like `1d12h`. Same format as on the site.
#[must_use]
pub fn parse_duration(input: &str) -> Option<u64> {
    let text = input.trim().to_ascii_lowercase();
    let mut rest = text.as_str();
    let mut total: u64 = 0;
    if rest.is_empty() {
        return None;
    }

    while !rest.is_empty() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let number: u64 = rest.get(..digits)?.parse().ok()?;
        if number > MAX_DURATION_S {
            return None;
        }
        rest = rest.get(digits..)?;

        // `mo` before `m`, or a month would read as minutes followed by garbage.
        let (unit, length) = if rest.starts_with("mo") {
            (30 * 86400, 2)
        } else {
            let unit = match rest.bytes().next()? {
                b's' => 1,
                b'm' => 60,
                b'h' => 3600,
                b'd' => 86400,
                b'w' => 7 * 86400,
                _ => return None,
            };
            (unit, 1)
        };
        rest = rest.get(length..)?;
        total = total.checked_add(number.checked_mul(unit)?)?;
    }

    (total > 0 && total <= MAX_DURATION_S).then_some(total)
}

/// «7 дней», «1 день 12 часов», «3 часа 30 минут», «45 секунд». Two largest units at most.
#[must_use]
pub fn format_duration(seconds: u64) -> String {
    // Whole minutes, rounded half up, once there is at least one.
    let rounded = if seconds >= 60 {
        (seconds + 30) / 60 * 60
    } else {
        seconds
    };
    let days = rounded / 86400;
    let hours = rounded % 86400 / 3600;
    let minutes = rounded % 3600 / 60;
    let mut out = String::new();

    if days > 0 {
        append(&mut out, days, ["день", "дня", "дней"]);
        if hours > 0 {
            append(&mut out, hours, ["час", "часа", "часов"]);
        }
    } else if hours > 0 {
        append(&mut out, hours, ["час", "часа", "часов"]);
        if minutes > 0 {
            append(&mut out, minutes, ["минуту", "минуты", "минут"]);
        }
    } else if minutes > 0 {
        append(&mut out, minutes, ["минуту", "минуты", "минут"]);
    } else {
        append(&mut out, rounded, ["секунду", "секунды", "секунд"]);
    }
    out
}

fn append(out: &mut String, number: u64, [one, few, many]: [&str; 3]) {
    let word = match (number % 100, number % 10) {
        (11..=14, _) => many,
        (_, 1) => one,
        (_, 2..=4) => few,
        _ => many,
    };
    if !out.is_empty() {
        out.push(' ');
    }
    // Writing to a `String` cannot fail.
    let _ = write!(out, "{number} {word}");
}

/// An ISO time of the site as `07.11.2026 18:00` Moscow time; the text itself if it is not one.
#[must_use]
pub fn format_time(iso: &str) -> String {
    let Some(offset) = FixedOffset::east_opt(MOSCOW_OFFSET_S) else {
        return iso.to_owned();
    };
    DateTime::parse_from_rfc3339(iso).map_or_else(
        |_| iso.to_owned(),
        |time| {
            time.with_timezone(&offset)
                .format("%d.%m.%Y %H:%M")
                .to_string()
        },
    )
}

/// Seconds from `created_at` to `expires_at`, both ISO times.
#[must_use]
pub fn duration_between(created_at: &str, expires_at: &str) -> Option<u64> {
    let created = DateTime::parse_from_rfc3339(created_at).ok()?;
    let expires = DateTime::parse_from_rfc3339(expires_at).ok()?;
    Some(u64::try_from((expires - created).num_seconds()).unwrap_or(0))
}

#[derive(Clone, Default)]
struct Style {
    color: Option<Color>,
    bold: bool,
    italic: bool,
    underlined: bool,
    strikethrough: bool,
    obfuscated: bool,
}

impl Style {
    /// Reads the formatting code after a `&`: the new style and the bytes the code takes.
    /// `None` when `after` does not start with one.
    fn code(&self, after: &str) -> Option<(Self, usize)> {
        if let Some(hex) = after.strip_prefix('#').and_then(|rest| rest.get(..6))
            && let Ok(rgb) = u32::from_str_radix(hex, 16)
            && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            let [_, red, green, blue] = rgb.to_be_bytes();
            let style = Self {
                color: Some(Color::Rgb(red, green, blue)),
                ..Self::default()
            };
            return Some((style, 7));
        }
        let code = after.chars().next()?;
        let style = self.with(code.to_ascii_lowercase())?;
        Some((style, code.len_utf8()))
    }

    /// Applies `&<code>`; `None` when `code` is not a formatting code.
    fn with(&self, code: char) -> Option<Self> {
        let color = match code {
            '0' => Color::Black,
            '1' => Color::DarkBlue,
            '2' => Color::DarkGreen,
            '3' => Color::DarkAqua,
            '4' => Color::DarkRed,
            '5' => Color::DarkPurple,
            '6' => Color::Gold,
            '7' => Color::Gray,
            '8' => Color::DarkGray,
            '9' => Color::Blue,
            'a' => Color::Green,
            'b' => Color::Aqua,
            'c' => Color::Red,
            'd' => Color::LightPurple,
            'e' => Color::Yellow,
            'f' => Color::White,
            'k' => {
                return Some(Self {
                    obfuscated: true,
                    ..self.clone()
                });
            }
            'l' => {
                return Some(Self {
                    bold: true,
                    ..self.clone()
                });
            }
            'm' => {
                return Some(Self {
                    strikethrough: true,
                    ..self.clone()
                });
            }
            'n' => {
                return Some(Self {
                    underlined: true,
                    ..self.clone()
                });
            }
            'o' => {
                return Some(Self {
                    italic: true,
                    ..self.clone()
                });
            }
            'r' => return Some(Self::default()),
            _ => return None,
        };
        // A color resets the decorations, like in vanilla.
        Some(Self {
            color: Some(color),
            ..Self::default()
        })
    }

    fn apply(&self, text: String) -> TextComponent {
        let mut component = TextComponent::plain(text);
        if let Some(color) = self.color.clone() {
            component = component.color(color);
        }
        if self.bold {
            component = component.bold(true);
        }
        if self.italic {
            component = component.italic(true);
        }
        if self.underlined {
            component = component.underlined(true);
        }
        if self.strikethrough {
            component = component.strikethrough(true);
        }
        if self.obfuscated {
            component = component.obfuscated(true);
        }
        component
    }
}

/// Renders `&` color codes and `{placeholders}`. Values are inserted as plain text, so a `&`
/// in a reason or a nickname is not read as a color.
#[must_use]
pub fn render(format: &str, values: &[(&str, &str)]) -> TextComponent {
    render_parts(format, &[], values)
}

/// [`render`] with placeholders that are ready-made components (a nickname with a hover, a
/// role badge, the chat message). A part takes the color in effect where it stands; `None`
/// leaves nothing. `&#rrggbb` is any color.
#[must_use]
pub fn render_parts(
    format: &str,
    parts: &[(&str, Option<TextComponent>)],
    values: &[(&str, &str)],
) -> TextComponent {
    let mut children = Vec::new();
    let mut text = String::new();
    let mut style = Style::default();
    let mut rest = format;

    while let Some(character) = rest.chars().next() {
        let after = &rest[character.len_utf8()..];

        if character == '&'
            && let Some((next, length)) = style.code(after)
        {
            if !text.is_empty() {
                children.push(style.apply(mem::take(&mut text)));
            }
            style = next;
            rest = &after[length..];
            continue;
        }

        if character == '{'
            && let Some(end) = after.find('}')
        {
            let name = &after[..end];
            if let Some((_, part)) = parts.iter().find(|(key, _)| *key == name) {
                if !text.is_empty() {
                    children.push(style.apply(mem::take(&mut text)));
                }
                if let Some(part) = part {
                    children.push(style.apply(String::new()).add_child(part.clone()));
                }
                rest = &after[end + 1..];
                continue;
            }
            if let Some((_, value)) = values.iter().find(|(key, _)| *key == name) {
                text.push_str(value);
                rest = &after[end + 1..];
                continue;
            }
        }

        text.push(character);
        rest = after;
    }

    if !text.is_empty() {
        children.push(style.apply(text));
    }
    TextComponent::plain("").add_children(children)
}

/// Epoch milliseconds as `07.11.2026 18:00` Moscow time.
#[must_use]
pub fn format_epoch_millis(millis: i64) -> String {
    let time = FixedOffset::east_opt(MOSCOW_OFFSET_S)
        .zip(DateTime::from_timestamp_millis(millis))
        .map(|(offset, time)| time.with_timezone(&offset));
    time.map_or_else(
        || millis.to_string(),
        |time| time.format("%d.%m.%Y %H:%M").to_string(),
    )
}

#[cfg(test)]
mod tests {
    use text_components::resolving::NoResolutor;

    use super::*;

    #[test]
    fn durations_parse_like_on_the_site() {
        assert_eq!(parse_duration("30m"), Some(1800));
        assert_eq!(parse_duration("1d12h"), Some(129_600));
        assert_eq!(parse_duration("1mo"), Some(30 * 86400));
        assert_eq!(parse_duration("2W"), Some(14 * 86400));
        assert_eq!(parse_duration("1mo1m"), Some(30 * 86400 + 60));
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("0d"), None);
        assert_eq!(parse_duration("7"), None);
        assert_eq!(parse_duration("d"), None);
        assert_eq!(parse_duration("7days"), None);
        assert_eq!(parse_duration("11y"), None);
        assert_eq!(parse_duration("99999999999999999999d"), None);
    }

    #[test]
    fn durations_read_in_russian() {
        assert_eq!(format_duration(7 * 86400), "7 дней");
        assert_eq!(format_duration(86400 + 12 * 3600), "1 день 12 часов");
        assert_eq!(format_duration(3 * 3600 + 30 * 60), "3 часа 30 минут");
        assert_eq!(format_duration(45), "45 секунд");
        assert_eq!(format_duration(21 * 60), "21 минуту");
        assert_eq!(format_duration(22 * 3600), "22 часа");
        assert_eq!(format_duration(11 * 86400), "11 дней");
        assert_eq!(format_duration(89), "1 минуту");
        assert_eq!(format_duration(90), "2 минуты");
    }

    #[test]
    fn times_are_shown_in_moscow() {
        assert_eq!(format_time("2026-11-07T15:00:00Z"), "07.11.2026 18:00");
        assert_eq!(
            format_time("2026-12-31T23:30:00.123+00:00"),
            "01.01.2027 02:30"
        );
        assert_eq!(format_time("soon"), "soon");
        assert_eq!(
            duration_between("2026-11-07T15:00:00Z", "2026-11-08T15:00:00Z"),
            Some(86400)
        );
        assert_eq!(duration_between("2026-11-07T15:00:00Z", "never"), None);
    }

    #[test]
    fn formats_take_parts_and_hex_colors() {
        let name = TextComponent::plain("Steve").bold(true);
        let message = render_parts(
            "&#a855f7[Г] {prefix}&f{name}&8 » &f{message} &#zzzzzz",
            &[("prefix", None), ("name", Some(name))],
            &[("message", "привет {name}")],
        );
        assert_eq!(
            message.to_plain(&NoResolutor),
            "[Г] Steve » привет {name} &#zzzzzz"
        );
        assert_eq!(format_epoch_millis(1_794_063_600_000), "07.11.2026 18:00");
    }

    #[test]
    fn formats_keep_values_as_plain_text() {
        let message = render(
            "&c⛔ &f{player} &cзабанен. &7Причина: &f{reason} {unknown}",
            &[("player", "Steve"), ("reason", "&lгриф {player}")],
        );
        assert_eq!(
            message.to_plain(&NoResolutor),
            "⛔ Steve забанен. Причина: &lгриф {player} {unknown}"
        );
    }
}
