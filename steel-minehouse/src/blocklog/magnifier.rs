//! «Лупа»: the item that shows the history of a block to any player, no permission needed.
//!
//! Hitting a block shows who placed and broke it. Using a container shows who put in and
//! took out what; using any other block shows the spot in front of the clicked face. The same
//! click again within a minute shows the next page.

use std::borrow::Cow;

use steel_registry::{REGISTRY, RegistryExt as _};
use steel_utils::Identifier;
use text_components::{TextComponent, translation::TranslatedMessage};

use super::{
    format::ago,
    store::{Actions, BREAK, Event, PLACE, PUT, Query, Subject},
};
use crate::moderation::format::{render, render_parts};

/// Rows on a page.
pub(super) const PAGE_SIZE: u32 = 7;
/// After this many pages the next click starts over.
pub(super) const MAX_PAGES: u32 = 30;
/// The same click within this time shows the next page.
pub(super) const PAGE_MS: i64 = 60_000;
/// A held button or a doubled packet is one click.
pub(super) const COOLDOWN_MS: i64 = 400;

/// What a player looked at last, for paging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Glance {
    pub world: String,
    pub pos: (i32, i32, i32),
    pub container: bool,
    pub page: u32,
    pub at: i64,
}

impl Glance {
    /// The page a click at `now` shows, given the glance before it.
    pub(super) fn next(
        previous: Option<&Self>,
        world: &str,
        pos: (i32, i32, i32),
        container: bool,
        now: i64,
    ) -> Self {
        let page = previous
            .filter(|last| {
                last.world == world
                    && last.pos == pos
                    && last.container == container
                    && now - last.at < PAGE_MS
            })
            .map_or(0, |last| last.page + 1);
        Self {
            world: world.to_owned(),
            pos,
            container,
            page: if page >= MAX_PAGES { 0 } else { page },
            at: now,
        }
    }

    /// The query of this page: one row more than a page, to know whether there is a next.
    pub(super) fn query(&self, partner: Option<(i32, i32, i32)>) -> Query {
        Query {
            world: Some(self.world.clone()),
            around: Some((self.pos, 0)),
            also: partner.into_iter().collect(),
            blocks: (!self.container).then_some(Actions::Any),
            containers: self.container.then_some(Actions::Any),
            offset: self.page * PAGE_SIZE,
            limit: PAGE_SIZE + 1,
            ..Query::default()
        }
    }
}

/// The client's own name of a block or an item; the id where the client has none.
fn name(kind: &str, block: bool) -> TextComponent {
    let Ok(key) = kind.parse::<Identifier>() else {
        return TextComponent::plain(kind.to_owned());
    };
    let group = if block || REGISTRY.blocks.by_key(&key).is_some() {
        "block"
    } else {
        "item"
    };
    TranslatedMessage {
        key: Cow::Owned(format!("{group}.{}.{}", key.namespace, key.path)),
        fallback: Some(Cow::Owned(kind.to_owned())),
        args: None,
    }
    .component()
}

fn line(event: &Event, now: i64) -> TextComponent {
    let (sign, verb, what) = match &event.subject {
        Subject::Block { .. } => {
            let (sign, verb) = match event.action {
                BREAK => ("&c-", "сломал"),
                PLACE => ("&a+", "поставил"),
                _ => ("&e·", "использовал"),
            };
            (sign, verb, name(&event.kind, true))
        }
        Subject::Items { amount, .. } => {
            let (sign, verb) = if event.action == PUT {
                ("&a+", "положил")
            } else {
                ("&c-", "забрал")
            };
            let mut what = TextComponent::plain(format!("{amount} × "));
            what.children.push(name(&event.kind, false));
            (sign, verb, what)
        }
    };
    // `#tnt` and the like are causes that are not players.
    let actor = if event.actor.name.starts_with('#') {
        "&6"
    } else {
        "&b"
    };
    let undone = if event.rolled_back {
        " &8(откачено)"
    } else {
        ""
    };
    render_parts(
        &format!("&7{{ago}} {sign} {actor}{{actor}} &7{verb} &f{{what}}{undone}"),
        &[("what", Some(what))],
        &[
            ("ago", &ago(now, event.time)),
            ("actor", event.actor.name.trim_start_matches('#')),
        ],
    )
}

/// The chat lines of a page. `rows` may hold one row more than a page: then there is a next.
pub(super) fn lines(glance: &Glance, block: &str, rows: &[Event], now: i64) -> Vec<TextComponent> {
    let (x, y, z) = glance.pos;
    let page = if glance.page > 0 {
        format!(" &7стр. {}", glance.page + 1)
    } else {
        String::new()
    };
    let block = if block == "minecraft:air" {
        TextComponent::plain("пустое место")
    } else {
        name(block, true)
    };
    let mut lines = vec![render_parts(
        &format!("&6Лупа &8· &f{{block}} &8({x}, {y}, {z}){page}"),
        &[("block", Some(block))],
        &[],
    )];
    if rows.is_empty() {
        lines.push(render(
            if glance.page > 0 {
                "&7Больше ничего нет"
            } else if glance.container {
                "&7Из этого контейнера никто ничего не брал и не клал"
            } else {
                "&7Здесь ничего не ставили и не ломали"
            },
            &[],
        ));
        return lines;
    }
    let shown = rows.len().min(PAGE_SIZE as usize);
    lines.extend(rows[..shown].iter().map(|event| line(event, now)));
    if rows.len() > shown {
        lines.push(render(
            if glance.container {
                "&8› ПКМ ещё раз — дальше"
            } else {
                "&8› ЛКМ ещё раз — дальше"
            },
            &[],
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::{super::store::Actor, *};

    #[test]
    fn the_same_click_again_turns_the_page() {
        let first = Glance::next(None, "minecraft:overworld", (1, 2, 3), false, 1_000);
        assert_eq!(first.page, 0);
        let second = Glance::next(Some(&first), "minecraft:overworld", (1, 2, 3), false, 2_000);
        assert_eq!(second.page, 1);
        assert_eq!(second.query(None).offset, PAGE_SIZE);
        // Another block, another kind of click, or a minute later: from the top.
        for (pos, container, now) in [
            ((1, 2, 4), false, 3_000),
            ((1, 2, 3), true, 3_000),
            ((1, 2, 3), false, 2_000 + PAGE_MS),
        ] {
            let other = Glance::next(Some(&second), "minecraft:overworld", pos, container, now);
            assert_eq!(other.page, 0);
        }
        let last = Glance {
            page: MAX_PAGES - 1,
            ..second
        };
        let wrapped = Glance::next(Some(&last), "minecraft:overworld", (1, 2, 3), false, 2_500);
        assert_eq!(wrapped.page, 0);
    }

    #[test]
    fn a_hit_asks_about_blocks_and_a_use_about_what_a_container_held() {
        let block = Glance::next(None, "minecraft:overworld", (1, 2, 3), false, 0);
        let query = block.query(None);
        assert_eq!(query.blocks, Some(Actions::Any));
        assert_eq!(query.containers, None);
        let chest = Glance::next(None, "minecraft:overworld", (1, 2, 3), true, 0);
        let query = chest.query(Some((2, 2, 3)));
        assert_eq!(query.blocks, None);
        assert_eq!(query.containers, Some(Actions::Any));
        assert_eq!(query.also, [(2, 2, 3)]);
        assert_eq!(query.limit, PAGE_SIZE + 1);
    }

    #[test]
    fn a_page_shows_seven_rows_and_says_when_there_are_more() {
        let glance = Glance::next(None, "minecraft:overworld", (1, 2, 3), false, 0);
        let row = Event {
            time: 0,
            actor: Actor {
                name: "Steve".to_owned(),
                uuid: None,
            },
            world: "minecraft:overworld".to_owned(),
            pos: (1, 2, 3),
            kind: "minecraft:stone".to_owned(),
            action: BREAK,
            rolled_back: false,
            subject: Subject::Block {
                state: String::new(),
            },
        };
        // The header and the line that nothing happened.
        assert_eq!(lines(&glance, "minecraft:air", &[], 0).len(), 2);
        let few = vec![row.clone(); 3];
        assert_eq!(lines(&glance, "minecraft:stone", &few, 0).len(), 4);
        // Eight rows came back: seven are shown, and a footer.
        let many = vec![row; 8];
        assert_eq!(lines(&glance, "minecraft:stone", &many, 0).len(), 9);
    }
}
