//! Vanilla `ItemProvider` and the `ItemCommands` distributors that build one.

use steel_registry::item_stack::ItemStack;

/// Vanilla `ItemCommands.ItemDistributor`: `replace`, `fill` or `override`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Distributor {
    Replace,
    Fill,
    Override,
}

impl Distributor {
    /// Vanilla registration order.
    pub(super) const ALL: [Self; 3] = [Self::Replace, Self::Fill, Self::Override];

    pub(super) const fn literal(self) -> &'static str {
        match self {
            Self::Replace => "replace",
            Self::Fill => "fill",
            Self::Override => "override",
        }
    }

    /// Vanilla `replaceItemProvider`, `fillItemProvider` and
    /// `overrideItemProvider`.
    pub(super) fn provider(self, items: Vec<ItemStack>) -> ItemProvider {
        let mode = match self {
            Self::Replace => Mode::Array,
            Self::Fill => Mode::Cycled,
            Self::Override => Mode::Fallback {
                item: ItemStack::empty(),
                using: false,
            },
        };
        ItemProvider {
            items,
            position: 0,
            mode,
        }
    }
}

/// Vanilla `ItemProvider` as `replace`, `fill` and `override` build it.
#[derive(Debug)]
pub(super) struct ItemProvider {
    items: Vec<ItemStack>,
    position: usize,
    mode: Mode,
}

#[derive(Debug)]
enum Mode {
    /// Vanilla `ArrayItemProvider`: each item once.
    Array,
    /// Vanilla `ArrayItemProvider.Cycled`: the items over and over.
    Cycled,
    /// Vanilla `FallbackItemProvider` over an `ArrayItemProvider`: each item
    /// once, then copies of `item` forever.
    Fallback { item: ItemStack, using: bool },
}

impl ItemProvider {
    pub(super) const fn has_next(&self) -> bool {
        match self.mode {
            Mode::Array => self.position < self.items.len(),
            Mode::Cycled => !self.items.is_empty(),
            Mode::Fallback { .. } => true,
        }
    }

    pub(super) const fn restart(&mut self) {
        self.position = 0;
        if let Mode::Fallback { using, .. } = &mut self.mode {
            *using = false;
        }
    }

    /// The next item, a copy. Only called while [`Self::has_next`].
    pub(super) fn next(&mut self) -> ItemStack {
        match &mut self.mode {
            Mode::Array => {
                let item = self.items[self.position].clone();
                self.position += 1;
                item
            }
            Mode::Cycled => {
                let item = self.items[self.position % self.items.len()].clone();
                self.position += 1;
                item
            }
            Mode::Fallback { item, using } => {
                if !*using {
                    if let Some(next) = self.items.get(self.position) {
                        self.position += 1;
                        return next.clone();
                    }
                    *using = true;
                }
                item.clone()
            }
        }
    }
}
