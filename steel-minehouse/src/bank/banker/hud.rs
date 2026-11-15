//! The talk window over the game (the Fabric `DialogHud`): pictures and lines of the fonts of
//! the server pack in the action bar.
//!
//! The action bar centers a line by its width, so the line is put together with a total
//! shift of 0: then x = 0 is the middle of the screen and every piece is placed by shifts
//! (characters of the font `minehouse-bank:dialog` with a negative or positive width). The
//! height comes from the fonts themselves. `dialog.json` is made by `tools/gen_dialog.py` of
//! the Fabric mod together with the fonts.

use std::{collections::BTreeMap, mem, sync::LazyLock};

use serde_json::Value;
use text_components::{Modifier as _, TextComponent, format::Color};

const LAYOUT: &str = include_str!("dialog.json");
const FONT: &str = "minehouse-bank:dialog";

/// A picture of the font.
#[derive(Clone, Copy)]
struct Glyph {
    character: char,
    advance: i32,
}

/// What `dialog.json` says of the window.
pub struct Hud {
    glyphs: BTreeMap<String, Glyph>,
    widths: BTreeMap<char, i32>,
    layout: Value,
    colors: Value,
}

static HUD: LazyLock<Hud> = LazyLock::new(|| Hud::read(LAYOUT));

/// The window of the server pack.
#[must_use]
pub fn hud() -> &'static Hud {
    &HUD
}

fn int(value: Option<&Value>) -> i32 {
    value
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or(0)
}

const fn rgb(color: i32) -> Color {
    let [_, red, green, blue] = color.to_be_bytes();
    Color::Rgb(red, green, blue)
}

fn no_shadow(mut text: TextComponent) -> TextComponent {
    text.format = text.format.shadow_color(0, 0, 0, 0);
    text
}

fn in_font(mut text: TextComponent, font: String) -> TextComponent {
    text.format = text.format.font(font);
    text
}

impl Hud {
    fn read(text: &str) -> Self {
        let root: Value = serde_json::from_str(text).unwrap_or_else(|error| {
            log::error!("Bank: the dialog layout can't be read: {error}");
            Value::Null
        });
        let entries = |key: &str| root.get(key).and_then(Value::as_object);
        let glyphs = entries("glyphs")
            .into_iter()
            .flatten()
            .filter_map(|(name, glyph)| {
                let character = glyph.get("char")?.as_str()?.chars().next()?;
                let advance = int(glyph.get("advance"));
                Some((name.clone(), Glyph { character, advance }))
            })
            .collect();
        let widths = entries("widths")
            .into_iter()
            .flatten()
            .filter_map(|(character, width)| Some((character.chars().next()?, int(Some(width)))))
            .collect();
        Self {
            glyphs,
            widths,
            layout: root.get("layout").cloned().unwrap_or(Value::Null),
            colors: root.get("colors").cloned().unwrap_or(Value::Null),
        }
    }

    fn element(&self, element: &str, key: &str) -> Option<&Value> {
        self.layout.get(element)?.get(key)
    }

    /// Where an element starts, from the middle of the screen.
    #[must_use]
    pub fn x(&self, element: &str) -> i32 {
        int(self.element(element, "x"))
    }

    /// How wide the text of an element may be.
    #[must_use]
    pub fn width(&self, element: &str) -> i32 {
        int(self.element(element, "width"))
    }

    /// How many lines an element has.
    #[must_use]
    pub fn count(&self, element: &str) -> usize {
        self.element(element, "b")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    }

    /// How many places the thumb of the scroll bar has.
    #[must_use]
    pub fn thumb_steps(&self) -> i32 {
        int(self.element("thumb", "steps"))
    }

    /// Where the hint ends.
    #[must_use]
    pub fn hint_right(&self) -> i32 {
        int(self.element("hint", "right"))
    }

    /// A color of the window.
    #[must_use]
    pub fn color(&self, name: &str) -> Color {
        rgb(int(self.colors.get(name)))
    }

    /// How far a picture moves the pen.
    #[must_use]
    pub fn advance(&self, glyph: &str) -> i32 {
        self.glyphs.get(glyph).map_or(0, |glyph| glyph.advance)
    }

    fn char_width(&self, character: char) -> i32 {
        self.widths
            .get(&character)
            .or_else(|| self.widths.get(&'?'))
            .copied()
            .unwrap_or(0)
    }

    /// The text with every character the fonts can't draw turned into «?».
    #[must_use]
    pub fn clean(&self, text: &str) -> String {
        text.chars()
            .map(|character| {
                if self.widths.contains_key(&character) {
                    character
                } else {
                    '?'
                }
            })
            .collect()
    }

    /// The width of a text in pixels.
    #[must_use]
    pub fn text_width(&self, text: &str) -> i32 {
        text.chars()
            .map(|character| self.char_width(character))
            .sum()
    }

    /// Cuts with «…» so the text fits `max` pixels.
    #[must_use]
    pub fn fit(&self, text: &str, max: i32) -> String {
        if self.text_width(text) <= max {
            return text.to_owned();
        }
        let mut cut: Vec<char> = text.chars().collect();
        let dots = self.char_width('…');
        while !cut.is_empty() && self.text_width(&cut.iter().collect::<String>()) + dots > max {
            cut.pop();
        }
        let cut: String = cut.into_iter().collect();
        format!("{}…", cut.trim_end())
    }

    /// Word wrap to lines of at most `max` pixels.
    #[must_use]
    pub fn wrap(&self, text: &str, max: i32) -> Vec<String> {
        let mut lines = Vec::new();
        let mut line = String::new();
        for word in text.split(' ') {
            let next = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            if self.text_width(&next) <= max {
                line = next;
            } else if line.is_empty() {
                line = self.fit(&next, max);
            } else {
                lines.push(line);
                line = self.fit(word, max);
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
        lines
    }

    /// A line to put pieces on.
    #[must_use]
    pub fn builder(&self) -> Builder<'_> {
        Builder {
            hud: self,
            root: TextComponent::plain(""),
            cursor: 0,
        }
    }
}

/// The fog behind the window, for the title: left, middle and right tiles that overlap by a
/// pixel.
#[must_use]
pub fn fog() -> TextComponent {
    let fog = TextComponent::plain("\u{E001}\u{E003}\u{E000}\u{E003}\u{E002}")
        .color(Color::Rgb(255, 255, 255));
    no_shadow(in_font(fog, format!("{FONT}_fog")))
}

/// Collects pieces at an absolute x; [`Builder::build`] comes back to x = 0.
pub struct Builder<'a> {
    hud: &'a Hud,
    root: TextComponent,
    cursor: i32,
}

impl Builder<'_> {
    fn push(&mut self, piece: TextComponent) {
        let root = mem::replace(&mut self.root, TextComponent::plain(""));
        self.root = root.add_child(piece);
    }

    fn move_to(&mut self, x: i32) {
        let shift = x - self.cursor;
        if shift == 0 {
            return;
        }
        // The font has a shift for every power of two up to 256, left from U+E000 and right
        // from U+E010.
        let base = if shift < 0 { 0xE000 } else { 0xE010 };
        let mut left = shift.unsigned_abs();
        let mut shifts = String::new();
        for bit in (0..=8).rev() {
            while left >= 1 << bit {
                shifts.extend(char::from_u32(base + bit));
                left -= 1 << bit;
            }
        }
        self.push(in_font(TextComponent::plain(shifts), FONT.to_owned()));
        self.cursor = x;
    }

    /// Puts a picture at `x`; a picture the layout does not know is left out.
    pub fn glyph(&mut self, key: &str, x: i32) {
        let Some(glyph) = self.hud.glyphs.get(key).copied() else {
            return;
        };
        self.move_to(x);
        let picture =
            TextComponent::plain(glyph.character.to_string()).color(Color::Rgb(255, 255, 255));
        self.push(no_shadow(in_font(picture, FONT.to_owned())));
        self.cursor += glyph.advance;
    }

    /// Puts a text at `x` in the font of the line `line`.
    pub fn text(&mut self, line: &str, x: i32, text: &str, color: Color, shadow: bool) {
        if text.is_empty() {
            return;
        }
        self.move_to(x);
        let piece = in_font(
            TextComponent::plain(text.to_owned()).color(color),
            format!("{FONT}_{line}"),
        );
        self.push(if shadow { piece } else { no_shadow(piece) });
        self.cursor += self.hud.text_width(text);
    }

    /// The line, back at x = 0.
    #[must_use]
    pub fn build(mut self) -> TextComponent {
        self.move_to(0);
        self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_of_the_pack_is_read() {
        let hud = hud();
        assert_eq!(hud.x("panel"), -197);
        assert_eq!(hud.width("text"), 234);
        assert_eq!(hud.count("text"), 2);
        assert_eq!(hud.count("rows"), 3);
        assert_eq!(hud.thumb_steps(), 8);
        assert_eq!(hud.hint_right(), 200);
        assert_eq!(hud.advance("panel"), 251);
        assert_eq!(hud.color("selected"), Color::Rgb(255, 255, 255));
        assert_eq!(hud.color("text"), Color::Rgb(0x40, 0x40, 0x40));
    }

    #[test]
    fn a_text_is_measured_cleaned_and_cut() {
        let hud = hud();
        assert_eq!(hud.text_width("  "), 8);
        assert_eq!(hud.clean("АР \u{1F600}"), "АР ?");
        let long = "Очень длинная строка ответа банкира";
        let cut = hud.fit(long, 60);
        assert!(cut.ends_with('…'));
        assert!(hud.text_width(&cut) <= 60);
        assert_eq!(hud.fit("АР", 60), "АР");
    }

    #[test]
    fn words_wrap_to_the_width() {
        let hud = hud();
        let text = "Положите алмазную руду в слоты банкомата и нажмите «Внести». Баланс вырастет.";
        let lines = hud.wrap(text, 234);
        assert!(lines.len() >= 2);
        assert!(lines.iter().all(|line| hud.text_width(line) <= 234));
        assert_eq!(lines.join(" "), text);
    }

    #[test]
    fn a_built_line_ends_where_it_started() {
        let hud = hud();
        let mut builder = hud.builder();
        builder.glyph("panel", hud.x("panel"));
        builder.text("text0", hud.x("text"), "АР", hud.color("text"), false);
        builder.glyph("no such picture", 5);
        assert_eq!(builder.cursor, hud.x("text") + hud.text_width("АР"));
        let line = builder.build();
        // The shift to the panel, the panel, the shift to the text, the text, the shift back.
        assert_eq!(line.children.len(), 5);
    }
}
