use std::io::{Cursor, Error, Result};

use steel_macros::ServerPacket;
use steel_utils::codec::VarInt;
use steel_utils::serial::{ReadFrom, prefixed_read::read_utf};

/// Vanilla `WritableBookContent.PAGE_EDIT_LENGTH`.
pub const MAX_BOOK_PAGE_LENGTH: usize = 1024;
/// Vanilla `WritableBookContent.MAX_PAGES`.
pub const MAX_BOOK_PAGES: usize = 100;
/// Vanilla `WrittenBookContent.TITLE_MAX_LENGTH`.
pub const MAX_BOOK_TITLE_LENGTH: usize = 32;

/// Serverbound packet sent when a player saves or signs a writable book.
#[derive(ServerPacket, Clone, Debug, PartialEq, Eq)]
pub struct SEditBook {
    /// Inventory slot of the book: a hotbar slot or the offhand (40).
    pub slot: i32,
    /// The page texts, at most 100 pages of 1024 characters.
    pub pages: Vec<String>,
    /// The title when the book is being signed.
    pub title: Option<String>,
}

impl ReadFrom for SEditBook {
    fn read(data: &mut Cursor<&[u8]>) -> Result<Self> {
        let slot = VarInt::read(data)?.0;
        let count = usize::try_from(VarInt::read(data)?.0)
            .map_err(|_| Error::other("negative book page count"))?;
        if count > MAX_BOOK_PAGES {
            return Err(Error::other(format!(
                "{count} elements exceeded max size of: {MAX_BOOK_PAGES}"
            )));
        }
        let mut pages = Vec::with_capacity(count);
        for _ in 0..count {
            pages.push(read_utf(data, MAX_BOOK_PAGE_LENGTH)?);
        }
        let title = if bool::read(data)? {
            Some(read_utf(data, MAX_BOOK_TITLE_LENGTH)?)
        } else {
            None
        };
        Ok(Self { slot, pages, title })
    }
}

#[cfg(test)]
mod tests {
    use steel_utils::serial::WriteTo as _;

    use super::*;

    #[test]
    fn edit_book_reads_slot_pages_and_optional_title() {
        // Slot 3, two pages "a" and "bc", then a present title "T".
        let bytes = [3, 2, 1, b'a', 2, b'b', b'c', 1, 1, b'T'];
        let packet = SEditBook::read(&mut Cursor::new(&bytes[..])).expect("should decode");
        assert_eq!(
            packet,
            SEditBook {
                slot: 3,
                pages: vec!["a".to_owned(), "bc".to_owned()],
                title: Some("T".to_owned()),
            }
        );
    }

    #[test]
    fn edit_book_rejects_more_than_max_pages() {
        let bytes = [0, 101];
        assert!(SEditBook::read(&mut Cursor::new(&bytes[..])).is_err());
    }

    #[test]
    fn edit_book_bounds_pages_by_characters_not_bytes() {
        // 1024 Cyrillic characters take 2048 bytes but still fit one page.
        let page = "я".repeat(MAX_BOOK_PAGE_LENGTH);
        let mut bytes = vec![0, 1];
        VarInt(i32::try_from(page.len()).expect("page fits i32"))
            .write(&mut bytes)
            .expect("length should encode");
        bytes.extend(page.as_bytes());
        bytes.push(0);
        let packet = SEditBook::read(&mut Cursor::new(&bytes[..])).expect("should decode");
        assert_eq!(packet.pages, vec![page]);
    }

    #[test]
    fn edit_book_rejects_long_titles() {
        let mut bytes = vec![0, 0, 1, 33];
        bytes.extend([b'x'; 33]);
        assert!(SEditBook::read(&mut Cursor::new(&bytes[..])).is_err());
    }
}
