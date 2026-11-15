//! The byte format of Plasmo Voice packets: Java's `DataOutput`, big-endian, strings as
//! `writeUTF` (a 16-bit length, then modified UTF-8).

use uuid::Uuid;

/// A packet that ended early or holds a value out of its bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireError;

/// Writes a packet.
#[derive(Default)]
pub struct Out(pub Vec<u8>);

impl Out {
    pub fn byte(&mut self, value: u8) {
        self.0.push(value);
    }

    pub fn bool(&mut self, value: bool) {
        self.0.push(u8::from(value));
    }

    pub fn short(&mut self, value: i16) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    pub fn int(&mut self, value: i32) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    /// A count: Java writes them as `int`.
    pub fn count(&mut self, value: usize) {
        self.int(i32::try_from(value).unwrap_or(i32::MAX));
    }

    pub fn long(&mut self, value: i64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    pub fn double(&mut self, value: f64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    pub fn uuid(&mut self, value: Uuid) {
        self.0.extend_from_slice(value.as_bytes());
    }

    /// `writeUTF`. A text over 65535 bytes can't be written and goes out empty.
    pub fn utf(&mut self, value: &str) {
        let mut bytes = Vec::with_capacity(value.len());
        for unit in value.encode_utf16() {
            match unit {
                0x0001..=0x007F => bytes.push(low_byte(unit)),
                0x0000 | 0x0080..=0x07FF => {
                    bytes.push(0xC0 | low_byte(unit >> 6));
                    bytes.push(0x80 | low_byte(unit & 0x3F));
                }
                _ => {
                    bytes.push(0xE0 | low_byte(unit >> 12));
                    bytes.push(0x80 | low_byte((unit >> 6) & 0x3F));
                    bytes.push(0x80 | low_byte(unit & 0x3F));
                }
            }
        }
        let Ok(length) = u16::try_from(bytes.len()) else {
            self.0.extend_from_slice(&0u16.to_be_bytes());
            return;
        };
        self.0.extend_from_slice(&length.to_be_bytes());
        self.0.extend_from_slice(&bytes);
    }

    /// An `int` length, then the bytes.
    pub fn bytes(&mut self, value: &[u8]) {
        self.count(value.len());
        self.0.extend_from_slice(value);
    }
}

const fn low_byte(value: u16) -> u8 {
    value.to_be_bytes()[1]
}

/// Reads a packet.
pub struct In<'data> {
    data: &'data [u8],
}

impl<'data> In<'data> {
    pub const fn new(data: &'data [u8]) -> Self {
        Self { data }
    }

    fn take(&mut self, length: usize) -> Result<&'data [u8], WireError> {
        let (head, rest) = self.data.split_at_checked(length).ok_or(WireError)?;
        self.data = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], WireError> {
        self.take(N)?.try_into().map_err(|_| WireError)
    }

    pub fn byte(&mut self) -> Result<u8, WireError> {
        Ok(u8::from_be_bytes(self.array()?))
    }

    pub fn bool(&mut self) -> Result<bool, WireError> {
        Ok(self.byte()? != 0)
    }

    pub fn short(&mut self) -> Result<i16, WireError> {
        Ok(i16::from_be_bytes(self.array()?))
    }

    pub fn int(&mut self) -> Result<i32, WireError> {
        Ok(i32::from_be_bytes(self.array()?))
    }

    /// A count from `min` to `max`.
    pub fn count(&mut self, min: usize, max: usize) -> Result<usize, WireError> {
        let value = usize::try_from(self.int()?).map_err(|_| WireError)?;
        if (min..=max).contains(&value) {
            Ok(value)
        } else {
            Err(WireError)
        }
    }

    pub fn long(&mut self) -> Result<i64, WireError> {
        Ok(i64::from_be_bytes(self.array()?))
    }

    pub fn uuid(&mut self) -> Result<Uuid, WireError> {
        Ok(Uuid::from_bytes(self.array()?))
    }

    /// `readUTF` of at most `max` characters.
    pub fn utf(&mut self, max: usize) -> Result<String, WireError> {
        let length = usize::from(u16::from_be_bytes(self.array()?));
        let mut bytes = self.take(length)?.iter().copied();
        let mut units = Vec::with_capacity(length);
        while let Some(first) = bytes.next() {
            let mut next = || {
                bytes
                    .next()
                    .map(|byte| u16::from(byte & 0x3F))
                    .ok_or(WireError)
            };
            let unit = match first {
                0x00..=0x7F => u16::from(first),
                0xC0..=0xDF => (u16::from(first & 0x1F) << 6) | next()?,
                0xE0..=0xEF => (u16::from(first & 0x0F) << 12) | (next()? << 6) | next()?,
                _ => return Err(WireError),
            };
            units.push(unit);
        }
        if units.len() > max {
            return Err(WireError);
        }
        Ok(String::from_utf16_lossy(&units))
    }

    /// An `int` length from 1 to `max`, then the bytes.
    pub fn bytes(&mut self, max: usize) -> Result<&'data [u8], WireError> {
        let length = self.count(1, max)?;
        self.take(length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf_is_java_modified_utf8() {
        let mut out = Out::default();
        out.utf("aя\u{0}€😀");
        // 'a', 'я' (2 bytes), NUL (2 bytes), '€' (3 bytes), a surrogate pair (3 + 3 bytes).
        assert_eq!(
            out.0,
            [
                0, 14, b'a', 0xD1, 0x8F, 0xC0, 0x80, 0xE2, 0x82, 0xAC, 0xED, 0xA0, 0xBD, 0xED,
                0xB8, 0x80
            ]
        );
        assert_eq!(In::new(&out.0).utf(16), Ok("aя\u{0}€😀".to_owned()));
        assert_eq!(In::new(&out.0).utf(3), Err(WireError));
    }

    #[test]
    fn numbers_are_big_endian() {
        let mut out = Out::default();
        out.short(-2);
        out.int(0x0102_0304);
        out.long(5);
        out.bool(true);
        out.uuid(Uuid::from_u128(0x0011_2233_4455_6677_8899_AABB_CCDD_EEFF));
        out.bytes(&[9, 8]);
        let mut input = In::new(&out.0);
        assert_eq!(input.short(), Ok(-2));
        assert_eq!(input.int(), Ok(0x0102_0304));
        assert_eq!(input.long(), Ok(5));
        assert_eq!(input.bool(), Ok(true));
        assert_eq!(
            input.uuid(),
            Ok(Uuid::from_u128(0x0011_2233_4455_6677_8899_AABB_CCDD_EEFF))
        );
        assert_eq!(input.bytes(2048), Ok(&[9, 8][..]));
        assert_eq!(input.byte(), Err(WireError));
    }

    #[test]
    fn counts_are_bounded() {
        let mut out = Out::default();
        out.int(-1);
        out.int(5000);
        let mut input = In::new(&out.0);
        assert_eq!(input.count(0, 10), Err(WireError));
        assert_eq!(input.bytes(2048), Err(WireError));
    }
}
