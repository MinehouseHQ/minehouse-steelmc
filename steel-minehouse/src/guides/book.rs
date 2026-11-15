//! The book of guides (the Fabric `GuideBook`): a vanilla written book whose pages list the
//! guides as lines to click, each running `/guide <id>`. The pages are put together again
//! every time the book is opened, so a new guide shows up in a book made long ago and a guide
//! that is not shown now stays out. A book and a sheet of paper make one on a workbench.

use std::sync::LazyLock;

use simdnbt::owned::NbtCompound;
use steel_core::behavior::{
    InteractionResult, ItemBehavior, UseItemContext, items::WrittenBookItem,
};
use steel_registry::{
    Registry,
    data_components::{
        CustomData,
        vanilla_components::{
            CUSTOM_DATA, Filterable, ItemLore, LORE, WRITTEN_BOOK_CONTENT, WrittenBookContent,
        },
    },
    item_stack::ItemStack,
    item_stack_template::ItemStackTemplate,
    recipe::{
        CraftingBookCategory, CraftingInput, CraftingRecipe, Ingredient, Recipe,
        RecipeBookCategoryKind, RecipeBookProperties, RecipeProperties, ShapelessRecipe,
        vanilla_recipe_types,
    },
    vanilla_items,
};
use steel_utils::Identifier;
use text_components::{
    Modifier as _, TextComponent,
    format::Color,
    interactivity::{ClickEvent, HoverEvent},
};

use super::{Guide, visible};

/// The flag in `custom_data` that marks the book.
const MARK: &str = "minehouse_guide_book";
/// The name of the book.
pub const TITLE: &str = "Гайды MineHouse";
const AUTHOR: &str = "MineHouse";
const WIKI: &str = "https://wiki.minehouse.pro/";
/// How many guides a page of the contents lists (a title may take two lines).
const GUIDES_PER_PAGE: usize = 5;
/// The key of the recipe, as on the Fabric server.
pub const RECIPE: (&str, &str) = ("minehouse-guides", "guide_book");

/// Whether `stack` is the book of guides.
#[must_use]
pub fn is_book(stack: &ItemStack) -> bool {
    stack.is(&vanilla_items::WRITTEN_BOOK)
        && stack
            .get(CUSTOM_DATA)
            .is_some_and(|data| data.as_compound().byte(MARK) == Some(1))
}

fn bold(text: &str) -> TextComponent {
    TextComponent::plain(text.to_owned()).bold(true)
}

fn gray(text: &str) -> TextComponent {
    TextComponent::plain(text.to_owned()).color(Color::DarkGray)
}

fn link(text: String, click: ClickEvent, hover: &str) -> TextComponent {
    TextComponent::plain(text)
        .color(Color::DarkAqua)
        .underlined(true)
        .click_event(click)
        .hover_event(HoverEvent::show_text(TextComponent::plain(
            hover.to_owned(),
        )))
}

/// The pages of the book for the guides `guides`.
fn pages_of(guides: &[&Guide]) -> Vec<TextComponent> {
    let mut pages = vec![
        TextComponent::plain("")
            .add_child(bold(TITLE).color(Color::DarkBlue))
            .add_child(TextComponent::plain(
                "\n\nНажми на гайд в оглавлении, и перед тобой появится сцена, которая покажет всё по шагам.\n\n",
            ))
            .add_child(gray("Рецепты — на последней странице.\n\n"))
            .add_child(gray(
                "Shift + ПКМ в воздух с предметом тоже открывает его гайд.",
            )),
    ];
    if guides.is_empty() {
        pages.push(TextComponent::plain("Гайдов пока нет, загляни позже."));
    }
    for chunk in guides.chunks(GUIDES_PER_PAGE) {
        let head = TextComponent::plain("")
            .add_child(bold("Оглавление"))
            .add_child(TextComponent::plain("\n"));
        pages.push(chunk.iter().fold(head, |page, guide| {
            page.add_child(TextComponent::plain("\n"))
                .add_child(link(
                    format!("▶ {}", guide.title),
                    ClickEvent::run_command(format!("/guide {}", guide.id)),
                    "Показать гайд",
                ))
                .add_child(TextComponent::plain("\n"))
        }));
    }
    pages.push(
        TextComponent::plain("")
            .add_child(bold("Рецепты"))
            .add_child(TextComponent::plain(
                "\n\nСо сборкой MineHouse: наведи на предмет и нажми ",
            ))
            .add_child(bold("R"))
            .add_child(TextComponent::plain(" — рецепт, "))
            .add_child(bold("U"))
            .add_child(TextComponent::plain(
                " — где он нужен.\n\nБез сборки: книга рецептов в инвентаре.\n\n",
            ))
            .add_child(link(
                "Вики сервера".to_owned(),
                ClickEvent::open_url(WIKI.to_owned()),
                WIKI,
            )),
    );
    pages
}

fn content(pages: Vec<TextComponent>) -> Option<WrittenBookContent> {
    WrittenBookContent::new(
        Filterable::pass_through(TITLE.to_owned()),
        AUTHOR.to_owned(),
        0,
        pages.into_iter().map(Filterable::pass_through).collect(),
        true,
    )
    .ok()
}

/// Puts the pages of today into the book; `false` if it had them already.
fn refresh(stack: &mut ItemStack) -> bool {
    let guides: Vec<&Guide> = visible().collect();
    let Some(content) = content(pages_of(&guides)) else {
        return false;
    };
    if stack.get(WRITTEN_BOOK_CONTENT) == Some(&content) {
        return false;
    }
    stack.set(WRITTEN_BOOK_CONTENT, content);
    true
}

/// The book with `pages`.
fn book(pages: Vec<TextComponent>) -> ItemStack {
    let mut stack = ItemStack::new(&vanilla_items::WRITTEN_BOOK);
    let mut tag = NbtCompound::new();
    tag.insert(MARK, 1_i8);
    if let Some(data) = CustomData::try_from_compound(tag) {
        stack.set(CUSTOM_DATA, data);
    }
    let mut hint =
        TextComponent::plain("ПКМ — открыть, нажми на гайд в оглавлении").color(Color::Gray);
    hint.format = hint.format.italic(false);
    if let Ok(lore) = ItemLore::new(vec![hint]) {
        stack.set(LORE, lore);
    }
    if let Some(content) = content(pages) {
        stack.set(WRITTEN_BOOK_CONTENT, content);
    }
    stack
}

/// A fresh book with the pages of today.
#[must_use]
pub fn create() -> ItemStack {
    let guides: Vec<&Guide> = visible().collect();
    book(pages_of(&guides))
}

/// The book in a hand: its pages are made anew before it opens.
pub(crate) struct GuideBook;

impl ItemBehavior for GuideBook {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        if context.inv.with_item(refresh) {
            // The client opens the book from its own copy of the stack: it gets the new
            // pages before it is told to open the book.
            context.player.broadcast_inventory_changes_and_trigger();
        }
        WrittenBookItem.use_item(context)
    }
}

/// A book and a sheet of paper. What comes out has one page; the real ones come when the
/// book is opened.
static BOOK_RECIPE: LazyLock<Recipe<CraftingRecipe, CraftingInput>> = LazyLock::new(|| {
    let made = book(vec![TextComponent::plain(
        "Открой книгу ещё раз, чтобы увидеть оглавление",
    )]);
    let result = ItemStackTemplate::for_registry_extension(
        &vanilla_items::WRITTEN_BOOK,
        1,
        made.components_patch().clone(),
    );
    let properties = RecipeProperties {
        special: false,
        placement: None,
        recipe_book: Some(RecipeBookProperties {
            category: RecipeBookCategoryKind::Crafting(CraftingBookCategory::Misc),
            group: None,
            show_notification: true,
        }),
    };
    Recipe::new(
        Identifier::new(RECIPE.0, RECIPE.1),
        &vanilla_recipe_types::CRAFTING,
        CraftingRecipe::Shapeless(ShapelessRecipe::new(
            properties,
            Box::new([
                Ingredient::Item(&vanilla_items::BOOK),
                Ingredient::Item(&vanilla_items::PAPER),
            ]),
            result,
        )),
    )
});

/// Adds the recipe of the book to the registry.
pub(crate) fn register_recipe(registry: &mut Registry) {
    registry.recipes.register(&*BOOK_RECIPE);
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, recipe::RecipeMatches as _};

    use super::*;
    use crate::guides::all;

    #[test]
    fn a_book_and_paper_make_the_book_of_guides() {
        init_vanilla_registry();
        let recipe = BOOK_RECIPE.data();
        let grid = |items: Vec<ItemStack>| CraftingInput::new(2, 1, items);
        assert!(recipe.matches(&grid(vec![
            ItemStack::new(&vanilla_items::PAPER),
            ItemStack::new(&vanilla_items::BOOK),
        ])));
        assert!(!recipe.matches(&grid(vec![
            ItemStack::new(&vanilla_items::BOOK),
            ItemStack::new(&vanilla_items::BOOK),
        ])));
        let CraftingRecipe::Shapeless(shapeless) = recipe else {
            panic!("the book is a shapeless recipe");
        };
        let made = shapeless.result.create();
        assert!(is_book(&made));
        assert!(!is_book(&ItemStack::new(&vanilla_items::WRITTEN_BOOK)));
        let written = made.get(WRITTEN_BOOK_CONTENT).map(|book| {
            (
                book.title().raw().clone(),
                book.author().to_owned(),
                book.pages().len(),
            )
        });
        assert_eq!(written, Some((TITLE.to_owned(), AUTHOR.to_owned(), 1)));
    }

    #[test]
    fn the_pages_list_the_guides_five_a_page() {
        let guides: Vec<&Guide> = all().iter().collect();
        let pages = pages_of(&guides);
        assert_eq!(pages.len(), 2 + guides.len().div_ceil(GUIDES_PER_PAGE));
        let contents = format!("{:?}", pages.get(1));
        assert!(contents.contains("/guide bank-atm"), "{contents}");
        assert!(contents.contains("▶ Банк: терминал оплаты"), "{contents}");
        let last = format!("{:?}", pages.last());
        assert!(last.contains(WIKI), "{last}");
        // With no guide to show the book says so.
        assert_eq!(pages_of(&[]).len(), 3);
    }

    #[test]
    fn opening_puts_the_pages_of_today_into_an_old_book() {
        init_vanilla_registry();
        let mut old = book(vec![TextComponent::plain("old")]);
        assert!(refresh(&mut old));
        assert!(!refresh(&mut old));
        assert!(is_book(&old));
        let pages = old
            .get(WRITTEN_BOOK_CONTENT)
            .map_or(0, |book| book.pages().len());
        assert!(pages >= 3);
    }
}
