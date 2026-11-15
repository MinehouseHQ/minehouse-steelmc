//! What the banker says and one talk with him: the pages type out, Shift goes on or takes an
//! answer, the wheel picks one.

use text_components::TextComponent;

use super::hud::hud;

/// How many characters appear in a tick.
const CHARS_PER_TICK: usize = 2;

/// An answer the player may give and where it leads.
pub struct Answer {
    label: &'static str,
    next: &'static str,
}

/// What the banker says at one point of the talk and what can be answered.
pub struct Node {
    pages: &'static [&'static str],
    answers: &'static [Answer],
}

const fn answer(label: &'static str, next: &'static str) -> Answer {
    Answer { label, next }
}

const TOPICS: &[Answer] = &[
    answer("Что такое АР?", "ar"),
    answer("Как открыть счёт?", "account"),
    answer("Внести и снять", "cash"),
    answer("Про карту", "card"),
    answer("Терминалы", "terminal"),
    answer("Переводы", "transfer"),
    answer("Сайт банка", "site"),
    answer("До свидания", "bye"),
];
const BACK: &[Answer] = &[answer("Ещё вопрос", "menu"), answer("До свидания", "bye")];

/// Every point of the talk.
pub const NODES: &[&str] = &[
    "start", "menu", "ar", "account", "cash", "card", "terminal", "transfer", "site", "bye",
];

/// The point of the talk with this name.
#[must_use]
pub fn node(id: &str) -> Option<Node> {
    let (answers, pages): (&[Answer], &[&str]) = match id {
        "start" => (
            TOPICS,
            &[
                "Добрый день, {name}! Я банкир MineHouse.",
                "Могу рассказать, как устроен наш банк. Что вам интересно?",
            ],
        ),
        "menu" => (TOPICS, &["Что ещё рассказать?"]),
        "ar" => (
            BACK,
            &[
                "АР — это алмазная руда, наша валюта. Одна руда — один АР, глубинная тоже считается.",
                "Руда на счёте хранится в банке: её не потеряешь при смерти и не оставишь в сундуке у соседа.",
            ],
        ),
        "account" => (
            BACK,
            &[
                "Счёт открывается в банкомате: подойдите к нему, нажмите ПКМ и выберите «Открыть счёт».",
                "Вместе со счётом банкомат выдаст карту MineHouse. Если её нет, там же есть «Выдать карту».",
                "Кроме личного можно открыть ещё до трёх счетов, например общий с друзьями.",
            ],
        ),
        "cash" => (
            BACK,
            &[
                "Положите алмазную руду в слоты банкомата и нажмите «Внести». Баланс вырастет сразу.",
                "«Снять АР» выдаёт глубинную алмазную руду обратно, сколько попросите.",
                "Снять получится, только если в инвентаре хватает места под руду.",
            ],
        ),
        "card" => (
            BACK,
            &[
                "Карта MineHouse — ключ к вашему счёту. Ею платят в терминалах и снимают руду.",
                "Платить картой может только хозяин счёта, так что чужая карта вору бесполезна.",
                "Потеряли карту? В банкомате есть «Перевыпустить карту», старая сразу перестанет работать.",
            ],
        ),
        "terminal" => (
            BACK,
            &[
                "Терминал оплаты продаёт банкомат за 5 АР, деньги спишутся со счёта.",
                "Нажмите ПКМ по новому терминалу: выберите счёт для денег и режим работы.",
                "Касса: вы выставляете сумму, а покупатель платит, нажав ПКМ по терминалу.",
                "Автомат: товар лежит в сундуке или бочке под терминалом и продаётся сам, даже без вас.",
                "Сундук автомата открыт всем, так что купить можно и за наличку: положить руду и забрать товар.",
                "«Сохранить и закрепить» вешает замок. Снять его можно, только сломав терминал.",
            ],
        ),
        "transfer" => (
            BACK,
            &[
                "Перевести АР другому игроку можно откуда угодно командой /transfer.",
                "Нужен только номер счёта получателя, он выглядит как MH-1234-5678. Карта не нужна.",
            ],
        ),
        "site" => (
            BACK,
            &[
                "На сайте minehouse.pro/bank видно баланс, историю операций и график заработка.",
                "Там же можно заблокировать карту и открыть дополнительные счета.",
            ],
        ),
        "bye" => (&[], &["Заходите ещё! Банк всегда рядом."]),
        _ => return None,
    };
    Some(Node { pages, answers })
}

/// What a press of Shift did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The rest of the page appeared at once.
    Shown,
    /// The next page.
    Turned,
    /// An answer was taken.
    Answered,
    /// The talk is over.
    Ended,
}

/// One talk.
pub struct Talk {
    name: String,
    answers: &'static [Answer],
    /// The pages of the point, each the lines of the window.
    pages: Vec<Vec<String>>,
    page: usize,
    /// How many characters of the page have appeared.
    shown: usize,
    selected: usize,
    scroll: usize,
    /// Ticks since the talk began.
    pub age: u32,
}

impl Talk {
    /// A talk with the player of this name, at the greeting.
    #[must_use]
    pub fn new(name: &str) -> Self {
        let mut talk = Self {
            name: name.to_owned(),
            answers: &[],
            pages: Vec::new(),
            page: 0,
            shown: 0,
            selected: 0,
            scroll: 0,
            age: 0,
        };
        talk.go("start");
        talk
    }

    /// Moves to a point of the talk.
    pub fn go(&mut self, id: &str) {
        let hud = hud();
        let Some(node) = node(id) else {
            return;
        };
        let lines = hud.count("text").max(1);
        self.answers = node.answers;
        self.pages.clear();
        for page in node.pages {
            let text = hud.clean(&page.replace("{name}", &self.name));
            let wrapped = hud.wrap(&text, hud.width("text"));
            self.pages
                .extend(wrapped.chunks(lines).map(<[String]>::to_vec));
        }
        self.page = 0;
        self.shown = 0;
        self.selected = 0;
        self.scroll = 0;
    }

    fn lines(&self) -> &[String] {
        self.pages.get(self.page).map_or(&[], Vec::as_slice)
    }

    fn page_length(&self) -> usize {
        self.lines().iter().map(|line| line.chars().count()).sum()
    }

    /// Whether the whole page has appeared.
    #[must_use]
    pub fn typed(&self) -> bool {
        self.shown >= self.page_length()
    }

    const fn last_page(&self) -> bool {
        self.page + 1 >= self.pages.len()
    }

    /// Whether the player picks an answer now.
    #[must_use]
    pub fn answering(&self) -> bool {
        self.last_page() && self.typed() && !self.answers.is_empty()
    }

    /// A tick of typing; true while the page still types out.
    pub fn type_more(&mut self) -> bool {
        if self.typed() {
            return false;
        }
        self.shown = (self.shown + CHARS_PER_TICK).min(self.page_length());
        true
    }

    /// Shift: shows the whole page, turns it or takes the answer.
    pub fn advance(&mut self) -> Step {
        if !self.typed() {
            self.shown = self.page_length();
            Step::Shown
        } else if !self.last_page() {
            self.page += 1;
            self.shown = 0;
            Step::Turned
        } else if let Some(answer) = self.answers.get(self.selected) {
            self.go(answer.next);
            Step::Answered
        } else {
            Step::Ended
        }
    }

    /// The wheel: moves the choice a step up or down; true if it moved.
    pub fn pick(&mut self, down: bool) -> bool {
        if !self.answering() {
            return false;
        }
        let last = self.answers.len().saturating_sub(1);
        let next = if down {
            (self.selected + 1).min(last)
        } else {
            self.selected.saturating_sub(1)
        };
        if next == self.selected {
            return false;
        }
        self.selected = next;
        let rows = hud().count("rows").max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
        true
    }

    /// The whole window as one line of the action bar.
    #[must_use]
    pub fn render(&self) -> TextComponent {
        let hud = hud();
        let mut line = hud.builder();
        let text = hud.color("text");

        line.glyph("panel", hud.x("panel"));
        line.glyph("portrait_banker", hud.x("portrait") + 4);
        line.glyph("portrait_frame", hud.x("portrait"));
        let name = "Банкир";
        let name_width = hud.text_width(name) + 12;
        let plate = hud.x("name");
        line.glyph("name_plate_left", plate);
        let left = hud.advance("name_plate_left") - 1;
        let middle = (hud.advance("name_plate_mid") - 1).max(1);
        let mut x = left;
        while x < name_width - left {
            line.glyph("name_plate_mid", plate + x);
            x += middle;
        }
        line.glyph("name_plate_right", plate + left.max(name_width - left));
        line.text("name", hud.x("name_text"), name, hud.color("name"), false);

        let mut budget = self.shown;
        for (index, full) in self.lines().iter().enumerate() {
            let visible: String = full.chars().take(budget).collect();
            budget = budget.saturating_sub(full.chars().count());
            line.text(
                &format!("text{index}"),
                hud.x("text"),
                &visible,
                text.clone(),
                false,
            );
        }
        let waits = !self.last_page() || self.answers.is_empty();
        if self.typed() && waits && (self.age / 10).is_multiple_of(2) {
            line.glyph("next_arrow", hud.x("next"));
        }

        let hint = if self.answering() {
            self.render_answers(&mut line);
            "Колёсико — выбор · Shift — ответ · Пробел — уйти".to_owned()
        } else {
            let shift = if self.typed() && self.last_page() {
                "Shift — закрыть"
            } else {
                "Shift — дальше"
            };
            format!("{shift} · Пробел — уйти")
        };
        let hint = hud.clean(&hint);
        let hint_x = hud.hint_right() - hud.text_width(&hint);
        line.text("hint", hint_x, &hint, hud.color("hint"), true);
        line.build()
    }

    fn render_answers(&self, line: &mut super::hud::Builder<'_>) {
        let hud = hud();
        let rows = hud.count("rows");
        line.glyph("answers_box", hud.x("box"));
        let shown = self.answers.iter().enumerate().skip(self.scroll).take(rows);
        for (row, (index, answer)) in shown.enumerate() {
            let selected = index == self.selected;
            let picture = if selected {
                "answer_row_selected_"
            } else {
                "answer_row_"
            };
            line.glyph(&format!("{picture}{row}"), hud.x("rows"));
            if selected {
                line.glyph(&format!("cursor_{row}"), hud.x("cursor"));
            }
            let label = hud.fit(&hud.clean(answer.label), hud.width("row_text"));
            let color = hud.color(if selected { "selected" } else { "text" });
            line.text(
                &format!("row{row}"),
                hud.x("row_text"),
                &label,
                color,
                false,
            );
        }
        if self.answers.len() > rows {
            line.glyph("scroll_track", hud.x("track"));
            let steps = usize::try_from(hud.thumb_steps()).unwrap_or(1).max(1);
            let span = self.answers.len() - rows;
            // Rounded to the nearest place of the thumb.
            let step = (self.scroll * (steps - 1) * 2 + span) / (span * 2);
            line.glyph(&format!("scroll_thumb_{step}"), hud.x("thumb"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(talk: &mut Talk) {
        while !talk.typed() {
            talk.type_more();
        }
    }

    #[test]
    fn every_answer_leads_to_a_point_of_the_talk() {
        for id in NODES {
            let node = node(id).unwrap_or_else(|| unreachable!());
            assert!(!node.pages.is_empty(), "{id}");
            for answer in node.answers {
                assert!(NODES.contains(&answer.next), "{id}: {}", answer.next);
            }
        }
        assert!(node("nowhere").is_none());
    }

    #[test]
    fn every_page_fits_the_window() {
        let hud = hud();
        for id in NODES {
            let mut talk = Talk::new("Player_With_16_ch");
            talk.go(id);
            for page in &talk.pages {
                assert!(page.len() <= hud.count("text"), "{id}");
                for line in page {
                    assert!(hud.text_width(line) <= hud.width("text"), "{id}: {line}");
                }
            }
        }
    }

    #[test]
    fn the_greeting_names_the_player_and_types_out() {
        let mut talk = Talk::new("Steve");
        let greeting: String = talk.lines().join(" ");
        assert_eq!(greeting, "Добрый день, Steve! Я банкир MineHouse.");
        assert!(!talk.typed());
        assert!(talk.type_more());
        assert_eq!(talk.shown, CHARS_PER_TICK);
        assert_eq!(talk.advance(), Step::Shown);
        assert!(talk.typed());
        assert!(!talk.answering());
        assert_eq!(talk.advance(), Step::Turned);
        read(&mut talk);
        assert!(talk.answering());
    }

    #[test]
    fn the_wheel_picks_an_answer_and_scrolls_the_rows() {
        let mut talk = Talk::new("Steve");
        assert!(!talk.pick(true));
        talk.advance();
        talk.advance();
        read(&mut talk);
        assert!(!talk.pick(false));
        for _ in 0..3 {
            assert!(talk.pick(true));
        }
        assert_eq!((talk.selected, talk.scroll), (3, 1));
        for _ in 0..10 {
            talk.pick(true);
        }
        assert_eq!((talk.selected, talk.scroll), (7, 5));
        assert!(talk.pick(false));
        assert_eq!((talk.selected, talk.scroll), (6, 5));
    }

    #[test]
    fn an_answer_leads_on_and_goodbye_ends_the_talk() {
        let mut talk = Talk::new("Steve");
        talk.advance();
        talk.advance();
        read(&mut talk);
        assert_eq!(talk.advance(), Step::Answered);
        assert!(talk.lines().join(" ").starts_with("АР — это алмазная руда"));
        while !talk.answering() {
            talk.advance();
        }
        // «Ещё вопрос», then down to «До свидания».
        assert!(talk.pick(true));
        assert_eq!(talk.advance(), Step::Answered);
        assert_eq!(talk.lines().join(" "), "Заходите ещё! Банк всегда рядом.");
        read(&mut talk);
        assert_eq!(talk.advance(), Step::Ended);
    }

    #[test]
    fn the_window_is_drawn_at_every_point() {
        for id in NODES {
            let mut talk = Talk::new("Steve");
            talk.go(id);
            assert_ne!(talk.render().children.len(), 0);
            while !talk.answering() && talk.advance() != Step::Ended {}
            assert_ne!(talk.render().children.len(), 0);
        }
    }
}
