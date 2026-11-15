//! The whole recipe book from the start (the Fabric `RecipeBookAll`): a player need not pick
//! up an ingredient first to see a recipe. On joining, every vanilla recipe the book can
//! show goes into the player's book quietly, with no toasts and no highlight, and the book
//! is sent again.
//!
//! Recipes of ours are left to unlock the vanilla way: some belong to features that are not
//! open yet. The book of guides is the one of ours everybody gets.

use rustc_hash::FxHashSet;
use steel_core::{
    inventory::recipe_displays::is_special,
    player::{Player, recipe_book::ServerRecipeBook},
    server::{
        Server,
        jobs::{JobPoll, ServerJob, ServerJobContext},
    },
};
use steel_registry::{REGISTRY, recipe::UntypedRecipeRef};
use steel_utils::Identifier;
use uuid::Uuid;

use crate::guides::book;

/// Every how many ticks newcomers are looked for.
const CHECK_EVERY_TICKS: u64 = 10;

/// Whether the recipe `key` is given to everyone: the vanilla ones.
#[must_use]
pub fn is_given(key: &Identifier) -> bool {
    key.namespace == "minecraft" || (&*key.namespace, &*key.path) == book::RECIPE
}

/// Adds the recipes of `keys` that `book` lacks. Returns how many were added.
fn fill<'a>(book: &mut ServerRecipeBook, keys: impl Iterator<Item = &'a Identifier>) -> usize {
    let mut added = 0;
    for key in keys {
        if is_given(key) && !book.contains(key) {
            book.add(key.clone());
            added += 1;
        }
    }
    added
}

/// The keys of the recipes a recipe book can show.
fn book_keys() -> impl Iterator<Item = &'static Identifier> {
    REGISTRY
        .recipes
        .iter()
        .filter(|recipe| !is_special(*recipe))
        .map(UntypedRecipeRef::key)
}

/// Gives `player` every recipe and sends the book again if it grew.
fn learn_all(player: &Player) {
    let mut book = player.recipe_book();
    let added = fill(&mut book, book_keys());
    if added > 0 {
        player.set_recipe_book(book);
        player.send_initial_recipe_book();
    }
}

/// Fills the book of each player who joins.
#[derive(Default)]
struct AllRecipes {
    online: FxHashSet<Uuid>,
}

impl ServerJob for AllRecipes {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        let Some(server) = context.server() else {
            return JobPoll::Finished;
        };
        if !context.tick_count.is_multiple_of(CHECK_EVERY_TICKS) {
            return JobPoll::Pending;
        }
        let players = server.get_players();
        for player in &players {
            if !self.online.contains(&player.gameprofile.id()) {
                learn_all(player);
            }
        }
        self.online = players
            .iter()
            .map(|player| player.gameprofile.id())
            .collect();
        JobPoll::Pending
    }
}

/// Starts filling recipe books.
pub(super) fn start(server: &Server) {
    server.jobs.spawn(AllRecipes::default());
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use super::*;

    #[test]
    fn the_book_takes_what_it_lacks_once() {
        let stick = Identifier::vanilla_static("stick");
        let torch = Identifier::vanilla_static("torch");
        let ours = Identifier::new_static("minehouse", "metro_station");
        let mut book = ServerRecipeBook::default();
        book.add(stick.clone());

        let keys = [stick, torch.clone(), ours.clone()];
        assert_eq!(fill(&mut book, keys.iter()), 1);
        assert!(book.contains(&torch));
        assert!(!book.contains(&ours));
        assert!(!book.is_highlighted(&torch));
        assert_eq!(fill(&mut book, keys.iter()), 0);
    }

    #[test]
    fn the_vanilla_book_is_large_and_has_no_special_recipes() {
        init_vanilla_registry();
        let mut book = ServerRecipeBook::default();
        let added = fill(&mut book, book_keys());
        assert!(added > 1000, "{added}");
        assert!(book.contains(&Identifier::vanilla_static("crafting_table")));
        assert!(
            REGISTRY
                .recipes
                .by_key(&Identifier::vanilla_static("firework_rocket"))
                .is_some()
        );
        assert!(!book.contains(&Identifier::vanilla_static("firework_rocket")));
    }
}
