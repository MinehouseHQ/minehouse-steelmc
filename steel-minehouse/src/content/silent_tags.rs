//! Silent name tags (the Fabric `SilentTags`). A name tag whose name holds one of the
//! silent words ("Тихо", "Тихо Бобик", "Бобик [тихо]") makes the mob silent: the vanilla
//! Silent flag, so no ambient, hurt, death or step sounds, and it stays after a restart.
//! Naming the mob again with any other tag gives its voice back. A mob made silent some
//! other way (`/data`) keeps the flag: only what a tag did is undone, marked by an entity
//! tag.

use steel_core::{
    behavior::items::NameTagObserver,
    entity::{Entity, LivingEntity},
};
use steel_registry::vanilla_entities;
use text_components::{TextComponent, resolving::NoResolutor};

/// The entity tag on mobs a name tag made silent.
const MARK: &str = "minehouse_silenced";
/// The words that make a mob silent, in lower case.
const WORDS: [&str; 6] = ["тихо", "тихий", "тихая", "silent", "shh", "шшш"];

/// Whether a whole word of `name` (letters and digits) is one of the silent words, in any
/// case.
#[must_use]
pub fn is_silent_name(name: &str) -> bool {
    name.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|part| WORDS.contains(&part))
}

/// A name tag just named `target`: makes it silent, or gives back the voice a tag took.
pub(super) fn named<E: Entity + ?Sized>(target: &E, name: &TextComponent) {
    if is_silent_name(&name.to_plain(&NoResolutor)) {
        target.set_silent(true);
        target.add_tag(MARK.to_owned());
    } else if target.remove_tag(MARK) {
        target.set_silent(false);
    }
}

/// Makes mobs silent by their name tags.
pub(super) struct SilentTags;

/// The one observer of name tags.
pub(super) static SILENT_TAGS: SilentTags = SilentTags;

impl NameTagObserver for SilentTags {
    fn named(&self, target: &dyn LivingEntity, name: &TextComponent) {
        if target.entity_type() == &vanilla_entities::PLAYER {
            return;
        }
        named(target, name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_silent_word_anywhere_in_the_name_counts() {
        assert!(is_silent_name("Тихо"));
        assert!(is_silent_name("тихо бобик"));
        assert!(is_silent_name("Бобик [Тихо]"));
        assert!(is_silent_name("Rex, SILENT"));
    }

    #[test]
    fn a_word_that_only_starts_like_one_does_not() {
        assert!(!is_silent_name("Тихон"));
        assert!(!is_silent_name("Бобик"));
        assert!(!is_silent_name(""));
    }
}
