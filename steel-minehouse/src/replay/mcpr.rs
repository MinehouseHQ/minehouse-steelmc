//! The file format of a replay: the `.mcpr` of the Replay Mod.
//!
//! An `.mcpr` is a zip of `recording.tmcpr` and `metaData.json`. The first is the packets
//! the client was sent, one after another: when it came, in milliseconds since the start
//! (a big-endian `i32`), how long it is (the same), and the packet as it is on the wire
//! before framing and compression, so its id and its fields.

use std::{
    fs::File,
    io::{self, BufReader, BufWriter, Read, Write},
    path::Path,
};

use flate2::{Compression, Crc, read::ZlibDecoder, write::DeflateEncoder};
use serde_json::json;
use uuid::Uuid;

/// The name of the packets in the zip.
pub const RECORDING: &str = "recording.tmcpr";
/// The name of the description in the zip.
pub const META: &str = "metaData.json";
/// The version of the format the Replay Mod writes for the current game.
const FILE_FORMAT_VERSION: u32 = 14;

/// Reads a `VarInt` from the front of `bytes`: the value and the rest.
fn var_int(bytes: &[u8]) -> Option<(usize, &[u8])> {
    let mut value: u32 = 0;
    for (index, byte) in bytes.iter().enumerate().take(5) {
        value |= u32::from(byte & 0x7F) << (7 * index);
        if byte & 0x80 == 0 {
            return Some((usize::try_from(value).ok()?, &bytes[index + 1..]));
        }
    }
    None
}

/// The id an unframed packet begins with.
#[must_use]
pub fn id(packet: &[u8]) -> Option<i32> {
    var_int(packet).and_then(|(id, _)| i32::try_from(id).ok())
}

/// The packet inside a frame the server queued for a client: its id and fields. `compressed`
/// is whether the connection compresses, so whether the frame says how long the packet is
/// when it is not deflated. `None` if the frame is broken.
#[must_use]
pub fn unframed(frame: &[u8], compressed: bool) -> Option<Vec<u8>> {
    let (length, rest) = var_int(frame)?;
    let body = rest.get(..length)?;
    if !compressed {
        return Some(body.to_vec());
    }
    let (inflated, body) = var_int(body)?;
    if inflated == 0 {
        return Some(body.to_vec());
    }
    let mut packet = Vec::with_capacity(inflated);
    ZlibDecoder::new(body).read_to_end(&mut packet).ok()?;
    (packet.len() == inflated).then_some(packet)
}

/// Writes one packet of a recording.
pub fn write_packet(out: &mut impl Write, millis: u32, packet: &[u8]) -> io::Result<()> {
    let length = i32::try_from(packet.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the packet is too long"))?;
    // The Replay Mod reads the time as an `i32`: 24 days is all a recording can be.
    out.write_all(&millis.min(i32::MAX.cast_unsigned()).to_be_bytes())?;
    out.write_all(&length.to_be_bytes())?;
    out.write_all(packet)
}

/// The time of the last whole packet in a recording, for one a stopped server left without
/// its description. Zero for an empty one.
pub fn duration(recording: &mut impl Read) -> u32 {
    let mut last = 0;
    let mut header = [0_u8; 8];
    while recording.read_exact(&mut header).is_ok() {
        let [time @ .., l0, l1, l2, l3] = header;
        let length = u64::from(u32::from_be_bytes([l0, l1, l2, l3]));
        match io::copy(&mut recording.by_ref().take(length), &mut io::sink()) {
            Ok(skipped) if skipped == length => last = u32::from_be_bytes(time),
            _ => break,
        }
    }
    last
}

/// What `metaData.json` says about a recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    /// The name of the server.
    pub server: String,
    /// How long the recording is, milliseconds.
    pub duration: u32,
    /// When it began, milliseconds since the Unix epoch.
    pub started: u64,
    /// The game version.
    pub version: String,
    /// The protocol version.
    pub protocol: i32,
    /// The player it is a recording of.
    pub player: Uuid,
}

impl Meta {
    fn json(&self) -> String {
        json!({
            "singleplayer": false,
            "serverName": self.server,
            "duration": self.duration,
            "date": self.started,
            "mcversion": self.version,
            "fileFormat": "MCPR",
            "fileFormatVersion": FILE_FORMAT_VERSION,
            "protocol": self.protocol,
            "generator": "MineHouse",
            "selfId": -1,
            "players": [self.player.to_string()],
        })
        .to_string()
    }
}

/// One file written into a zip: where it is and what the directory says of it.
struct Entry {
    name: &'static str,
    offset: u32,
    crc: u32,
    packed: u32,
    size: u32,
}

/// Counts and checksums what goes through to `inner`.
struct Counted<W> {
    inner: W,
    written: u64,
}

impl<W: Write> Write for Counted<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(bytes)?;
        self.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn too_big() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "the recording is over 4 GiB")
}

/// Deflates `data` into `out` as the zip file `name`. Sizes and the checksum are not known
/// until the end, so they follow the data and the header says so.
fn add<W: Write>(
    out: &mut Counted<W>,
    name: &'static str,
    data: &mut impl Read,
) -> io::Result<Entry> {
    let offset = u32::try_from(out.written).map_err(|_| too_big())?;
    let name_length = u16::try_from(name.len()).map_err(|_| too_big())?;
    out.write_all(&0x0403_4b50_u32.to_le_bytes())?;
    // Version 2.0, sizes after the data, deflate, no time, and no sizes yet.
    out.write_all(&[20, 0, 8, 0, 8, 0, 0, 0, 0, 0])?;
    out.write_all(&[0; 12])?;
    out.write_all(&name_length.to_le_bytes())?;
    out.write_all(&[0, 0])?;
    out.write_all(name.as_bytes())?;

    let begins = out.written;
    let mut crc = Crc::new();
    let mut size: u64 = 0;
    let mut deflate = DeflateEncoder::new(&mut *out, Compression::default());
    let mut buffer = vec![0_u8; 1 << 16];
    loop {
        let read = data.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        crc.update(&buffer[..read]);
        size += read as u64;
        deflate.write_all(&buffer[..read])?;
    }
    deflate.finish()?;
    let entry = Entry {
        name,
        offset,
        crc: crc.sum(),
        packed: u32::try_from(out.written - begins).map_err(|_| too_big())?,
        size: u32::try_from(size).map_err(|_| too_big())?,
    };
    out.write_all(&0x0807_4b50_u32.to_le_bytes())?;
    out.write_all(&entry.crc.to_le_bytes())?;
    out.write_all(&entry.packed.to_le_bytes())?;
    out.write_all(&entry.size.to_le_bytes())?;
    Ok(entry)
}

fn finish<W: Write>(out: &mut Counted<W>, entries: &[Entry]) -> io::Result<()> {
    let directory = u32::try_from(out.written).map_err(|_| too_big())?;
    for entry in entries {
        let name_length = u16::try_from(entry.name.len()).map_err(|_| too_big())?;
        out.write_all(&0x0201_4b50_u32.to_le_bytes())?;
        out.write_all(&[20, 0, 20, 0, 8, 0, 8, 0, 0, 0, 0, 0])?;
        out.write_all(&entry.crc.to_le_bytes())?;
        out.write_all(&entry.packed.to_le_bytes())?;
        out.write_all(&entry.size.to_le_bytes())?;
        out.write_all(&name_length.to_le_bytes())?;
        // No extra field, comment, disk or attributes.
        out.write_all(&[0; 12])?;
        out.write_all(&entry.offset.to_le_bytes())?;
        out.write_all(entry.name.as_bytes())?;
    }
    let length = u32::try_from(out.written).map_err(|_| too_big())? - directory;
    let count = u16::try_from(entries.len()).map_err(|_| too_big())?;
    out.write_all(&0x0605_4b50_u32.to_le_bytes())?;
    out.write_all(&[0; 4])?;
    out.write_all(&count.to_le_bytes())?;
    out.write_all(&count.to_le_bytes())?;
    out.write_all(&length.to_le_bytes())?;
    out.write_all(&directory.to_le_bytes())?;
    out.write_all(&[0, 0])?;
    out.flush()
}

/// Packs the packets in the file `recording` and their description into the replay `to`.
pub fn pack(recording: &Path, meta: &Meta, to: &Path) -> io::Result<()> {
    let mut packets = BufReader::new(File::open(recording)?);
    let mut out = Counted {
        inner: BufWriter::new(File::create(to)?),
        written: 0,
    };
    let entries = [
        add(&mut out, RECORDING, &mut packets)?,
        add(&mut out, META, &mut meta.json().as_bytes())?,
    ];
    finish(&mut out, &entries)
}

#[cfg(test)]
mod tests {
    use std::{env, fs, io::Cursor, process};

    use flate2::{read::DeflateDecoder, write::ZlibEncoder};

    use super::*;

    #[test]
    fn frames_come_apart_with_and_without_compression() {
        // Without compression: the length and the packet.
        assert_eq!(unframed(&[3, 0x2A, 1, 2], false), Some(vec![0x2A, 1, 2]));
        // With it, a short packet says 0 and is not deflated.
        assert_eq!(unframed(&[4, 0, 0x2A, 1, 2], true), Some(vec![0x2A, 1, 2]));
        // A long one says its length and is.
        let packet: Vec<u8> = (0..600_u32).map(|n| (n % 7) as u8).collect();
        let mut deflated = ZlibEncoder::new(Vec::new(), Compression::fast());
        deflated.write_all(&packet).expect("deflates");
        let deflated = deflated.finish().expect("deflates");
        // 600 as a VarInt is 0xD8 0x04.
        let mut frame = vec![u8::try_from(deflated.len() + 2).expect("a short frame")];
        frame.extend([0xD8, 0x04]);
        frame.extend(&deflated);
        assert_eq!(unframed(&frame, true), Some(packet));
        // Cut short or lying about its length.
        assert_eq!(unframed(&[9, 0x2A], false), None);
        frame[1] = 0xD9;
        assert_eq!(unframed(&frame, true), None);
    }

    #[test]
    fn packets_are_written_with_their_time_and_length() {
        let mut recording = Vec::new();
        write_packet(&mut recording, 0, &[2, 9]).expect("writes");
        write_packet(&mut recording, 1500, &[7]).expect("writes");
        assert_eq!(
            recording,
            [0, 0, 0, 0, 0, 0, 0, 2, 2, 9, 0, 0, 5, 0xDC, 0, 0, 0, 1, 7]
        );
        assert_eq!(duration(&mut Cursor::new(&recording)), 1500);
        // A packet the server did not finish writing does not count.
        recording.extend([0, 0, 9, 0, 0, 0, 0, 5, 1]);
        assert_eq!(duration(&mut Cursor::new(&recording)), 1500);
        assert_eq!(duration(&mut Cursor::new(Vec::new())), 0);
    }

    /// Reads the zip back by its directory, as a zip reader does.
    fn unpacked(zip: &[u8]) -> Vec<(String, Vec<u8>)> {
        let le32 = |at: usize| {
            u32::from_le_bytes([zip[at], zip[at + 1], zip[at + 2], zip[at + 3]]) as usize
        };
        let le16 = |at: usize| u16::from_le_bytes([zip[at], zip[at + 1]]) as usize;
        let end = zip.len() - 22;
        assert_eq!(le32(end), 0x0605_4b50);
        let mut at = le32(end + 16);
        (0..le16(end + 10))
            .map(|_| {
                assert_eq!(le32(at), 0x0201_4b50);
                let (packed, size, name_length) = (le32(at + 20), le32(at + 24), le16(at + 28));
                let local = le32(at + 42);
                let name =
                    String::from_utf8_lossy(&zip[at + 46..at + 46 + name_length]).into_owned();
                assert_eq!(le32(local), 0x0403_4b50);
                let data = local + 30 + le16(local + 26) + le16(local + 28);
                let mut file = Vec::new();
                DeflateDecoder::new(&zip[data..data + packed])
                    .read_to_end(&mut file)
                    .expect("inflates");
                assert_eq!(file.len(), size);
                let mut crc = Crc::new();
                crc.update(&file);
                assert_eq!(crc.sum() as usize, le32(at + 16));
                at += 46 + name_length;
                (name, file)
            })
            .collect()
    }

    #[test]
    fn a_replay_is_a_zip_of_the_packets_and_their_description() {
        let folder = env::temp_dir().join(format!("minehouse-mcpr-{}", process::id()));
        fs::create_dir_all(&folder).expect("a folder");
        let recording = folder.join("a.tmcpr");
        let mut packets = Vec::new();
        for millis in 0..2000 {
            write_packet(&mut packets, millis, &[1, 2, 3, 4]).expect("writes");
        }
        fs::write(&recording, &packets).expect("writes");
        let meta = Meta {
            server: "MineHouse".to_owned(),
            duration: 1999,
            started: 1_795_000_000_000,
            version: "26.3".to_owned(),
            protocol: 777,
            player: Uuid::from_u128(7),
        };
        let replay = folder.join("a.mcpr");
        pack(&recording, &meta, &replay).expect("packs");

        let files = unpacked(&fs::read(&replay).expect("reads"));
        fs::remove_dir_all(&folder).expect("removes");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0], (RECORDING.to_owned(), packets));
        assert_eq!(files[1].0, META);
        let description: serde_json::Value =
            serde_json::from_slice(&files[1].1).expect("the description is JSON");
        assert_eq!(description["duration"], 1999);
        assert_eq!(description["fileFormat"], "MCPR");
        assert_eq!(description["protocol"], 777);
        assert_eq!(
            description["players"][0],
            "00000000-0000-0000-0000-000000000007"
        );
    }
}
