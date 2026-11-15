//! The words of a lookup: `u:Nick t:3d r:10 b:chest a:-block`, in any order.

use super::store::{Actions, BREAK, PLACE, PUT, Query, TAKE};
use crate::moderation::format::parse_duration;

/// Where a lookup is asked from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    /// The key of the world.
    pub world: String,
    /// The block the asker stands in.
    pub pos: (i32, i32, i32),
}

/// Reads the words of a lookup into a query. `now` is Unix milliseconds.
pub fn parse(
    input: &str,
    origin: Option<&Origin>,
    max_radius: i32,
    now: i64,
) -> Result<Query, String> {
    let mut query = Query::default();
    let mut radius = None;
    for word in input.split_whitespace() {
        let Some((key, value)) = word.split_once(':') else {
            return Err(format!(
                "Не понял «{word}». Пример: u:Ник t:3d r:10 b:chest a:-block"
            ));
        };
        if value.is_empty() {
            return Err(format!("После «{key}:» ничего нет."));
        }
        match key.to_ascii_lowercase().as_str() {
            "u" | "user" | "users" => {
                query.actors.extend(value.split(',').map(str::to_owned));
            }
            "t" | "time" => {
                let seconds = parse_duration(value)
                    .ok_or_else(|| format!("Не понял срок «{value}». Пример: 30m, 12h, 3d, 2w."))?;
                query.since = Some(now - i64::try_from(seconds).unwrap_or(i64::MAX / 1000) * 1000);
            }
            "r" | "radius" => {
                radius = if value.eq_ignore_ascii_case("#global")
                    || value.eq_ignore_ascii_case("global")
                {
                    None
                } else {
                    let blocks: i32 = value
                        .parse()
                        .map_err(|_| format!("Радиус «{value}» не число."))?;
                    if !(0..=max_radius).contains(&blocks) {
                        return Err(format!("Радиус от 0 до {max_radius}, или r:#global."));
                    }
                    Some(blocks)
                };
            }
            "b" | "block" | "blocks" | "i" | "item" | "items" => {
                query.kinds.extend(value.split(',').map(kind));
            }
            "a" | "action" => match value.to_ascii_lowercase().as_str() {
                "block" => query.blocks = Some(Actions::Any),
                "+block" => query.blocks = Some(Actions::Only(PLACE)),
                "-block" => query.blocks = Some(Actions::Only(BREAK)),
                "container" => query.containers = Some(Actions::Any),
                "+container" => query.containers = Some(Actions::Only(PUT)),
                "-container" => query.containers = Some(Actions::Only(TAKE)),
                _ => {
                    return Err(
                        "Действия: block, +block, -block, container, +container, -container."
                            .to_owned(),
                    );
                }
            },
            _ => return Err(format!("Не знаю параметр «{key}:». Есть u: t: r: b: a:")),
        }
    }
    if let Some(radius) = radius {
        let origin = origin.ok_or("Радиус работает только у игрока. Из консоли ищи по u: и t:.")?;
        query.world = Some(origin.world.clone());
        query.around = Some((origin.pos, radius));
    }
    if query == Query::default() {
        return Err("Укажи хоть что-нибудь: u:Ник, t:3d, r:10, b:chest или a:-block.".to_owned());
    }
    Ok(query)
}

/// `stone` → `minecraft:stone`.
pub fn kind(name: &str) -> String {
    let name = name.to_ascii_lowercase();
    if name.contains(':') {
        name
    } else {
        format!("minecraft:{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin() -> Origin {
        Origin {
            world: "minecraft:overworld".to_owned(),
            pos: (10, 64, -5),
        }
    }

    #[test]
    fn every_parameter_reads() {
        let query = parse(
            "u:Steve,Alex t:1h r:10 b:Chest,minecraft:tnt a:-block",
            Some(&origin()),
            100,
            10_000_000,
        )
        .expect("a valid lookup");
        assert_eq!(query.actors, ["Steve", "Alex"]);
        assert_eq!(query.since, Some(10_000_000 - 3_600_000));
        assert_eq!(query.around, Some(((10, 64, -5), 10)));
        assert_eq!(query.world.as_deref(), Some("minecraft:overworld"));
        assert_eq!(query.kinds, ["minecraft:chest", "minecraft:tnt"]);
        assert_eq!(query.blocks, Some(Actions::Only(BREAK)));
        assert_eq!(query.containers, None);
    }

    #[test]
    fn a_lookup_without_radius_is_global() {
        let query = parse("u:Steve", Some(&origin()), 100, 0).expect("a valid lookup");
        assert_eq!(query.around, None);
        assert_eq!(query.world, None);
        let query = parse("u:Steve r:#global", None, 100, 0).expect("a valid lookup");
        assert_eq!(query.around, None);
    }

    #[test]
    fn mistakes_are_explained() {
        assert!(parse("", None, 100, 0).is_err());
        assert!(parse("steve", None, 100, 0).is_err());
        assert!(parse("r:500", Some(&origin()), 100, 0).is_err());
        assert!(parse("r:5", None, 100, 0).is_err());
        assert!(parse("t:soon", None, 100, 0).is_err());
        assert!(parse("a:explode", None, 100, 0).is_err());
        assert!(parse("x:1", None, 100, 0).is_err());
    }
}
