//! Countdown lengths: what an admin types and what players read.

/// The longest countdown: a day.
pub const MAX_SECONDS: u64 = 24 * 60 * 60;

/// `90`, `30s`, `5m`, `1h30m`, `2м` (Russian letters too). A bare number is seconds.
/// `None` when the text is not a time or is longer than [`MAX_SECONDS`].
#[must_use]
pub fn parse_seconds(text: &str) -> Option<u64> {
    let value = text.trim().to_lowercase();
    if value.is_empty() {
        return None;
    }

    let mut total: u64 = 0;
    let mut digits = String::new();
    let mut add = |digits: &mut String, unit: u64| -> Option<()> {
        let number: u64 = digits.parse().ok()?;
        digits.clear();
        if number > MAX_SECONDS {
            return None;
        }
        total += number * unit;
        (total <= MAX_SECONDS).then_some(())
    };
    for character in value.chars() {
        match character {
            '0'..='9' => digits.push(character),
            'h' | 'ч' => add(&mut digits, 3600)?,
            'm' | 'м' => add(&mut digits, 60)?,
            's' | 'с' => add(&mut digits, 1)?,
            _ => return None,
        }
    }
    if !digits.is_empty() {
        add(&mut digits, 1)?;
    }
    Some(total)
}

/// `1 ч 5 мин`, `5 мин`, `1 мин 30 с`, `10 с`.
#[must_use]
pub fn describe(seconds: u64) -> String {
    let (hours, minutes, rest) = (seconds / 3600, seconds % 3600 / 60, seconds % 60);
    let mut parts = Vec::new();
    if hours > 0 {
        parts.push(format!("{hours} ч"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes} мин"));
    }
    if rest > 0 && hours == 0 {
        parts.push(format!("{rest} с"));
    }
    if parts.is_empty() {
        "0 с".to_owned()
    } else {
        parts.join(" ")
    }
}

/// `1:05:00`, `4:32`, `0:09` for the bossbar.
#[must_use]
pub fn clock(seconds: u64) -> String {
    let (hours, minutes, rest) = (seconds / 3600, seconds % 3600 / 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{rest:02}")
    } else {
        format!("{minutes}:{rest:02}")
    }
}

/// Whether a mark lies in `[left, previous)`, so a lag spike can't skip a warning.
#[must_use]
pub fn crossed(marks: &[u64], previous: u64, left: u64) -> bool {
    marks.iter().any(|&mark| mark >= left && mark < previous)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_parse() {
        assert_eq!(parse_seconds("90"), Some(90));
        assert_eq!(parse_seconds("30s"), Some(30));
        assert_eq!(parse_seconds("5m"), Some(300));
        assert_eq!(parse_seconds("1h30m"), Some(5400));
        assert_eq!(parse_seconds("2М"), Some(120));
        assert_eq!(parse_seconds("1ч5м10с"), Some(3910));
        assert_eq!(parse_seconds("24h"), Some(MAX_SECONDS));
        assert_eq!(parse_seconds("25h"), None);
        assert_eq!(parse_seconds("m"), None);
        assert_eq!(parse_seconds("5x"), None);
        assert_eq!(parse_seconds("обновление"), None);
        assert_eq!(parse_seconds(""), None);
        assert_eq!(parse_seconds("99999999999999999999"), None);
    }

    #[test]
    fn times_read_well() {
        assert_eq!(describe(3900), "1 ч 5 мин");
        assert_eq!(describe(300), "5 мин");
        assert_eq!(describe(90), "1 мин 30 с");
        assert_eq!(describe(3610), "1 ч");
        assert_eq!(describe(0), "0 с");
        assert_eq!(clock(3900), "1:05:00");
        assert_eq!(clock(272), "4:32");
        assert_eq!(clock(9), "0:09");
    }

    #[test]
    fn marks_survive_a_lag_spike() {
        let marks = [60, 30, 10];
        assert!(crossed(&marks, 31, 30));
        assert!(crossed(&marks, 40, 25));
        assert!(!crossed(&marks, 30, 29));
        assert!(!crossed(&marks, 59, 31));
    }
}
