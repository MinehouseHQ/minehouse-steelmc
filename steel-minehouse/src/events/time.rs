//! Times in show scripts, all turned into server ticks (20 a second): `"1:05.5"` (minutes
//! and seconds), `"12s"`, `"500ms"`, `"40t"` (ticks), `"5m"`, or a bare number of seconds.

use serde_json::Value;

/// Server ticks in a second.
pub const TICKS_PER_SECOND: u32 = 20;

/// Parses a time as written in a script or typed into a command.
pub fn parse(text: &str) -> Result<u32, String> {
    let value = text.trim().to_lowercase();
    if value.is_empty() {
        return Err("пустое время".to_owned());
    }
    let unknown = || format!("не понимаю время \"{text}\"");
    let number = |part: &str| part.parse::<f64>().map_err(|_| unknown());

    if value.contains(':') {
        let mut total = 0.0;
        for part in value.split(':') {
            total = total * 60.0 + number(part)?;
        }
        return seconds(total);
    }
    if let Some(rest) = value.strip_suffix("ms") {
        return seconds(number(rest)? / 1000.0);
    }
    if let Some(rest) = value.strip_suffix('s') {
        return seconds(number(rest)?);
    }
    if let Some(rest) = value.strip_suffix('t') {
        return rest.parse::<u32>().map_err(|_| unknown());
    }
    if let Some(rest) = value.strip_suffix('m') {
        return seconds(number(rest)? * 60.0);
    }
    seconds(number(&value)?)
}

/// Seconds as ticks, rounded to the nearest tick.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "checked to be a non-negative number of ticks that fits"
)]
pub fn seconds(seconds: f64) -> Result<u32, String> {
    if !seconds.is_finite() || seconds < 0.0 {
        return Err("время не может быть отрицательным".to_owned());
    }
    let ticks = (seconds * f64::from(TICKS_PER_SECOND)).round();
    if ticks > f64::from(u32::MAX) {
        return Err("слишком большое время".to_owned());
    }
    Ok(ticks as u32)
}

/// A time of a script: a number is seconds, a string goes through [`parse`].
pub fn from_json(value: &Value) -> Result<u32, String> {
    match value {
        Value::Number(number) => seconds(number.as_f64().unwrap_or(0.0)),
        Value::String(text) => parse(text),
        other => Err(format!("ожидаю время, а не {other}")),
    }
}

/// `1250` ticks as `"1:02.5"`.
#[must_use]
pub fn format(ticks: u32) -> String {
    // 20 ticks are 10 tenths; half a tenth rounds up, as `Math.round` does.
    let tenths = ticks.div_ceil(2);
    let minutes = tenths / 600;
    let seconds = tenths / 10 % 60;
    let rest = tenths % 10;
    if rest == 0 {
        format!("{minutes}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}.{rest}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_way_to_write_a_time() {
        assert_eq!(parse("1:05.5"), Ok(1310));
        assert_eq!(parse("12s"), Ok(240));
        assert_eq!(parse("500ms"), Ok(10));
        assert_eq!(parse("40t"), Ok(40));
        assert_eq!(parse("10m"), Ok(12000));
        assert_eq!(parse(" 2.5 "), Ok(50));
        assert_eq!(parse("0:13"), Ok(260));
    }

    #[test]
    fn nonsense_is_refused_with_the_text() {
        assert_eq!(parse(""), Err("пустое время".to_owned()));
        assert_eq!(parse("soon"), Err("не понимаю время \"soon\"".to_owned()));
        assert!(parse("-3").is_err());
    }

    #[test]
    fn a_number_of_a_script_is_seconds() {
        assert_eq!(from_json(&serde_json::json!(1.5)), Ok(30));
        assert_eq!(from_json(&serde_json::json!("40t")), Ok(40));
        assert!(from_json(&serde_json::json!([1])).is_err());
    }

    #[test]
    fn ticks_are_shown_as_minutes_and_seconds() {
        assert_eq!(format(1250), "1:02.5");
        assert_eq!(format(260), "0:13");
        assert_eq!(format(0), "0:00");
    }
}
