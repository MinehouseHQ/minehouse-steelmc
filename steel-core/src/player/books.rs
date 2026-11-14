//! Writing, signing and reading books: vanilla
//! `ServerGamePacketListenerImpl.handleEditBook` and `ServerPlayer.openItemGui`.

use std::sync::Arc;

use steel_protocol::packets::game::{COpenBook, SEditBook};
use steel_registry::data_components::components::{
    Filterable, WritableBookContent, WrittenBookContent,
};
use steel_registry::data_components::vanilla_components::{
    WRITABLE_BOOK_CONTENT, WRITTEN_BOOK_CONTENT,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_items;
use steel_utils::types::InteractionHand;
use text_components::TextComponent;

use super::Player;
use super::player_inventory::PlayerInventory;
use crate::command::execution::{CommandSource, CommandTextResolver};
use crate::command::sender::CommandSender;
use crate::entity::Entity as _;
use crate::inventory::container::Container as _;

impl Player {
    /// Vanilla `ServerGamePacketListenerImpl.handleEditBook`: saves the pages
    /// of the writable book in `slot`, or signs it when a title is sent.
    ///
    /// Steel has no text filter service, so every text passes through
    /// unfiltered, as in vanilla without a `text-filtering-config`.
    pub fn handle_edit_book(&self, packet: SEditBook) {
        let Ok(slot) = usize::try_from(packet.slot) else {
            return;
        };
        if !PlayerInventory::HOTBAR.contains(&slot) && slot != PlayerInventory::SLOT_OFFHAND {
            return;
        }
        match packet.title {
            Some(title) => self.sign_book(title, packet.pages, slot),
            None => self.update_book_contents(packet.pages, slot),
        }
    }

    /// Vanilla `ServerGamePacketListenerImpl.updateBookContents`.
    fn update_book_contents(&self, pages: Vec<String>, slot: usize) {
        let mut inventory = self.inventory.lock();
        let carried = inventory.get_item(slot);
        if !carried.has(WRITABLE_BOOK_CONTENT) {
            return;
        }
        let Ok(content) =
            WritableBookContent::new(pages.into_iter().map(Filterable::pass_through).collect())
        else {
            return;
        };
        let mut book = carried.clone();
        book.set(WRITABLE_BOOK_CONTENT, content);
        inventory.set_item(slot, book);
    }

    /// Vanilla `ServerGamePacketListenerImpl.signBook`: turns the writable
    /// book into a resolved written book authored by this player.
    fn sign_book(&self, title: String, pages: Vec<String>, slot: usize) {
        let mut inventory = self.inventory.lock();
        let carried = inventory.get_item(slot);
        if !carried.has(WRITABLE_BOOK_CONTENT) {
            return;
        }
        let pages = pages
            .into_iter()
            .map(|page| Filterable::pass_through(TextComponent::plain(page)))
            .collect();
        let Ok(content) = WrittenBookContent::new(
            Filterable::pass_through(title),
            self.plain_text_name(),
            0,
            pages,
            true,
        ) else {
            return;
        };
        let mut written_book = ItemStack::with_count_and_patch(
            &vanilla_items::WRITTEN_BOOK,
            carried.count(),
            carried.components_patch().clone(),
        );
        written_book.remove(WRITABLE_BOOK_CONTENT);
        written_book.set(WRITTEN_BOOK_CONTENT, content);
        inventory.set_item(slot, written_book);
    }

    /// Vanilla `ServerPlayer.openItemGui`: resolves the written book in
    /// `hand` once, then tells the client to open it.
    pub fn open_item_gui(self: &Arc<Self>, hand: InteractionHand) {
        let content = self
            .inventory
            .lock()
            .get_item_in_hand(hand)
            .get(WRITTEN_BOOK_CONTENT)
            .cloned();
        let Some(content) = content else {
            return;
        };
        if self.resolve_written_book(hand, &content) {
            self.broadcast_inventory_changes_and_trigger();
        }
        self.send_packet(COpenBook { hand });
    }

    /// Vanilla `WrittenBookContent.resolveForItem` with the player's command
    /// source. Returns whether the pages were rewritten.
    ///
    /// Resolution runs outside the inventory lock, since NBT and selector
    /// components may read this player's own inventory.
    fn resolve_written_book(
        self: &Arc<Self>,
        hand: InteractionHand,
        content: &WrittenBookContent,
    ) -> bool {
        if content.resolved() {
            return false;
        }
        let resolved = self.server.upgrade().and_then(|server| {
            let source = CommandSource::new(CommandSender::Player(Arc::clone(self)), server);
            let resolver = CommandTextResolver::with_entity_override(&source, self.as_ref());
            resolve_pages(content, &resolver)
        });
        let changed = resolved.is_some();
        let replacement = resolved.unwrap_or_else(|| content.mark_resolved());
        self.inventory.lock().mutate_item_in_hand(hand, |stack| {
            // The book may have moved while the pages were resolving.
            if stack.get(WRITTEN_BOOK_CONTENT) == Some(content) {
                stack.set(WRITTEN_BOOK_CONTENT, replacement);
            }
        });
        changed
    }
}

/// Vanilla `WrittenBookContent.resolve`: `None` when a resolved page grows
/// past the page limit.
fn resolve_pages(
    content: &WrittenBookContent,
    resolver: &CommandTextResolver<'_>,
) -> Option<WrittenBookContent> {
    let pages = content
        .pages()
        .iter()
        .map(|page| resolve_page(page, resolver))
        .collect::<Option<Vec<_>>>()?;
    Some(content.with_resolved_pages(pages))
}

/// Vanilla `WrittenBookContent.resolvePage` through `Filterable.resolve`.
fn resolve_page(
    page: &Filterable<TextComponent>,
    resolver: &CommandTextResolver<'_>,
) -> Option<Filterable<TextComponent>> {
    let raw = resolve_page_text(page.raw(), resolver)?;
    let filtered = match page.filtered() {
        Some(filtered) => Some(resolve_page_text(filtered, resolver)?),
        None => None,
    };
    Some(Filterable::new(raw, filtered))
}

/// A component that fails to resolve stays as it was; one that resolves too
/// large rejects the whole book.
fn resolve_page_text(
    component: &TextComponent,
    resolver: &CommandTextResolver<'_>,
) -> Option<TextComponent> {
    let Ok(page) = component.try_resolve(resolver) else {
        return Some(component.clone());
    };
    (!WrittenBookContent::is_page_too_large(&page)).then_some(page)
}
