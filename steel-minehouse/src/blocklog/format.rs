//! How a page of the log reads in chat.

use text_components::TextComponent;

use super::store::{BREAK, CLICK, Event, PLACE, PUT, Page, Subject};
use crate::moderation::format::render;

/// «5 с назад», «12 мин назад», «3,5 ч назад», «2,1 д назад».
pub fn ago(now: i64, time: i64) -> String {
    let seconds = (now - time).max(0) / 1000;
    let tenths = |unit: i64| {
        let value = seconds * 10 / unit;
        format!("{},{}", value / 10, value % 10)
    };
    match seconds {
        0..60 => format!("{seconds} с назад"),
        60..3600 => format!("{} мин назад", seconds / 60),
        3600..86400 => format!("{} ч назад", tenths(3600)),
        _ => format!("{} д назад", tenths(86400)),
    }
}

/// `minecraft:stone` → `stone`; other namespaces stay.
fn short(kind: &str) -> &str {
    kind.strip_prefix("minecraft:").unwrap_or(kind)
}

/// One row. `with_place` adds the coordinates, for lookups wider than one block.
pub fn line(event: &Event, now: i64, with_place: bool) -> TextComponent {
    let (verb, what) = match &event.subject {
        Subject::Block { .. } => (
            match event.action {
                BREAK => "&cсломал",
                PLACE => "&aпоставил",
                CLICK => "&eнажал",
                _ => "&7изменил",
            },
            short(&event.kind).to_owned(),
        ),
        Subject::Items { amount, .. } => (
            if event.action == PUT {
                "&aположил"
            } else {
                "&cзабрал"
            },
            format!("{amount} × {}", short(&event.kind)),
        ),
    };
    let (x, y, z) = event.pos;
    let place = if with_place {
        format!(" &8({x} {y} {z})")
    } else {
        String::new()
    };
    let undone = if event.rolled_back {
        " &8(откачено)"
    } else {
        ""
    };
    render(
        &format!("&7{{ago}} &8· &f{{actor}} {verb} &f{{what}}{place}{undone}"),
        &[
            ("ago", &ago(now, event.time)),
            ("actor", &event.actor.name),
            ("what", &what),
        ],
    )
}

/// A page with its header and footer. `offset` is the first row's place in the whole answer.
pub fn page(
    title: &str,
    page: &Page,
    offset: u32,
    page_size: u32,
    now: i64,
    with_place: bool,
) -> Vec<TextComponent> {
    let mut lines = vec![render("&3Журнал &8· &7{title}", &[("title", title)])];
    if page.rows.is_empty() {
        lines.push(render("&7Ничего не найдено.", &[]));
        return lines;
    }
    lines.extend(page.rows.iter().map(|event| line(event, now, with_place)));
    let pages = page.total.div_ceil(u64::from(page_size.max(1)));
    if pages > 1 {
        let current = offset / page_size.max(1) + 1;
        lines.push(render(
            "&8Страница {current} из {pages} · /co page <номер>",
            &[
                ("current", &current.to_string()),
                ("pages", &pages.to_string()),
            ],
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocklog::store::Actor;

    #[test]
    fn times_read_in_the_largest_unit() {
        assert_eq!(ago(10_000, 5_000), "5 с назад");
        assert_eq!(ago(800_000, 0), "13 мин назад");
        assert_eq!(ago(3 * 3_600_000 + 1_800_000, 0), "3,5 ч назад");
        assert_eq!(ago(50 * 3_600_000, 0), "2,0 д назад");
        // A clock that went back is not a negative age.
        assert_eq!(ago(0, 5_000), "0 с назад");
    }

    #[test]
    fn a_row_names_who_did_what() {
        let event = Event {
            time: 0,
            actor: Actor {
                name: "Steve".to_owned(),
                uuid: None,
            },
            world: "minecraft:overworld".to_owned(),
            pos: (1, 64, -3),
            kind: "minecraft:diamond".to_owned(),
            action: PUT,
            rolled_back: false,
            subject: Subject::Items {
                amount: 3,
                data: None,
            },
        };
        let text = format!("{}", line(&event, 60_000, true));
        assert!(text.contains("1 мин назад"), "{text}");
        assert!(text.contains("Steve"), "{text}");
        assert!(text.contains("положил"), "{text}");
        assert!(text.contains("3 × diamond"), "{text}");
        assert!(text.contains("(1 64 -3)"), "{text}");
    }
}
