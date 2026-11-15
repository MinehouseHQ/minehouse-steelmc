//! Ingredient matching for recipes.

use std::sync::OnceLock;

use steel_utils::Identifier;

use crate::{REGISTRY, TaggedRegistryExt, item_stack::ItemStack, items::ItemRef};

/// Tells an item of a registry extension from the vanilla item it is made of: the id the
/// extension gave the stack, or `None` for a plain vanilla stack.
///
/// An extension's item is a vanilla item with components, since a vanilla client has no
/// other items. To a recipe it must not be that vanilla item: a magnifier made of paper is
/// not paper for a book.
pub type ExtensionItemId = fn(&ItemStack) -> Option<&'static str>;

static EXTENSION_ITEM_ID: OnceLock<ExtensionItemId> = OnceLock::new();

/// Sets how items of a registry extension are told from vanilla ones. From then on they
/// satisfy only [`Ingredient::Extension`], and vanilla ingredients turn them down. Returns
/// `false` if another extension set it first.
pub fn set_extension_item_id(id: ExtensionItemId) -> bool {
    EXTENSION_ITEM_ID.set(id).is_ok()
}

/// The id a registry extension gave `stack`, if it is an item of one.
fn extension_item_id(stack: &ItemStack) -> Option<&'static str> {
    EXTENSION_ITEM_ID.get().and_then(|id| id(stack))
}

/// Whether a vanilla ingredient may look at `stack`: it holds something, and that is not an
/// item of a registry extension.
fn is_vanilla_stack(stack: &ItemStack) -> bool {
    !stack.is_empty() && extension_item_id(stack).is_none()
}

/// Represents what items can satisfy a recipe slot.
/// Matches Java's `Ingredient` class.
#[derive(Debug, Clone)]
pub enum Ingredient {
    /// Matches nothing (empty slot required).
    Empty,
    /// Matches a single specific item.
    Item(ItemRef),
    /// Matches any item in a tag.
    Tag(Identifier),
    /// Matches any of the listed items (OR).
    Choice(&'static [ItemRef]),
    /// Matches the item a registry extension gave this id, and no other stack of `item`,
    /// the vanilla item it is made of. See [`set_extension_item_id`].
    Extension {
        /// The vanilla item clients see.
        item: ItemRef,
        /// The id of the extension's item.
        id: &'static str,
    },
}

impl Ingredient {
    /// Tests if the given item stack satisfies this ingredient.
    #[must_use]
    pub fn test(&self, stack: &ItemStack) -> bool {
        match self {
            Self::Empty => stack.is_empty(),
            Self::Item(item) => is_vanilla_stack(stack) && *item == stack.item,
            Self::Tag(tag) => is_vanilla_stack(stack) && REGISTRY.items.is_in_tag(stack.item, tag),
            Self::Choice(items) => is_vanilla_stack(stack) && items.contains(&stack.item),
            Self::Extension { item, id } => {
                !stack.is_empty() && *item == stack.item && extension_item_id(stack) == Some(*id)
            }
        }
    }

    /// Returns true if this ingredient matches an empty slot.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    /// Returns all items that could satisfy this ingredient (for recipe book display).
    #[must_use]
    pub fn get_items(&self) -> Vec<ItemRef> {
        match self {
            Self::Empty => Vec::new(),
            Self::Item(item) => vec![*item],
            Self::Tag(tag) => REGISTRY.items.get_tag(tag).unwrap_or_default(),
            Self::Choice(items) => items.to_vec(),
            // The client's recipe book has only the vanilla item to show.
            Self::Extension { item, .. } => vec![*item],
        }
    }

    /// Compares two ingredients for equality (used for symmetry detection).
    #[must_use]
    pub fn eq_ingredient(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Empty, Self::Empty) => true,
            (Self::Item(a), Self::Item(b)) => a == b,
            (Self::Tag(a), Self::Tag(b)) => a == b,
            (Self::Choice(a), Self::Choice(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x == y)
            }
            (Self::Extension { item: a, id: x }, Self::Extension { item: b, id: y }) => {
                a == b && x == y
            }
            _ => false,
        }
    }
}
